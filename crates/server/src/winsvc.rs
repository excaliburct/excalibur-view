//! Being a proper Windows service.
//!
//! The customer's whole experience of this is one double-click on the office
//! server and one "Yes" when Windows asks for permission. Everything in here is
//! what happens behind that: the program copies itself somewhere permanent,
//! registers itself with the service manager so it starts at boot and runs
//! with nobody signed in, opens the firewall for the office network only, and
//! starts.
//!
//! It is a real service, answering the service manager, not a console program
//! registered with `sc` and hoped for. Windows kills a program that is started
//! as a service and does not answer within thirty seconds.

#![cfg(windows)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

/// What the service manager knows it as.
pub const NAME: &str = "ExcaliburHyperviewServer";
/// What a person sees in Services.
pub const DISPLAY: &str = "Excalibur Hyperview Server";
/// What the program is called once it is installed.
pub const PROGRAM: &str = "Hyperview-Server.exe";

const FIREWALL_TCP: &str = "Excalibur Hyperview Server (drawings)";
const FIREWALL_UDP: &str = "Excalibur Hyperview Server (being found)";

/// Where the installed server and the company's drawings live.
///
/// ProgramData, because it is the machine's rather than anybody's: a person's
/// account being removed must not take the drawings with it.
pub fn home() -> PathBuf {
    let base = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    base.join("Excalibur Hyperview").join("Server")
}

pub fn installed_program() -> PathBuf {
    home().join(PROGRAM)
}

pub fn data_folder() -> PathBuf {
    home().join("hyperview-data")
}

/// Where the elevated half leaves its answer for the half the person is
/// looking at. Readable by everybody, writable by an administrator.
pub fn report_file() -> PathBuf {
    home().join("last-install.txt")
}

/// Whether it is installed, and whether it is running.
#[derive(Debug, PartialEq)]
pub enum Standing {
    NotInstalled,
    Running,
    Stopped,
    Starting,
}

pub fn standing() -> Standing {
    let Ok(manager) = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
    else {
        return Standing::NotInstalled;
    };
    let Ok(service) = manager.open_service(NAME, ServiceAccess::QUERY_STATUS) else {
        return Standing::NotInstalled;
    };
    match service.query_status().map(|s| s.current_state) {
        Ok(ServiceState::Running) => Standing::Running,
        Ok(ServiceState::StartPending) | Ok(ServiceState::ContinuePending) => Standing::Starting,
        _ => Standing::Stopped,
    }
}

// ---- installing: the elevated half ----------------------------------------

/// Installs or upgrades this program as the office's drawing server and starts
/// it. Needs administrator rights; the ordinary double-click starts a second
/// copy with them to do this.
pub fn install() -> Result<(), String> {
    let home = home();
    let parent = home.parent().ok_or("the service's folder has no folder above it")?.to_path_buf(); // WHAT: ProgramData\Excalibur Hyperview. WHY: locked first
    std::fs::create_dir_all(&parent).map_err(|e| format!("could not make {}: {e}", parent.display()))?;
    crate::lockdown::lock_parent(&parent); // WHAT: the folder above made the system's and administrators' alone. WHY: whoever made it first could otherwise swap the service's folder (audit, 2026-11-11)
    if std::fs::symlink_metadata(&home).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!("{} is a link to somewhere else, not a folder; it was left as it is and nothing was installed. Remove it, then install again.", home.display())); // WHAT: refused. WHY: a link put there would point the system's program somewhere an account controls
    }
    std::fs::create_dir_all(&home).map_err(|e| format!("could not make {}: {e}", home.display()))?;
    crate::lockdown::lock_down(&home); // WHAT: the folder made the system's and administrators' only, and any DLL beside the program taken away, before the program is put there or started. WHY: the service loads a DLL beside it before its own code runs, so doing this only when it starts was too late for one left there already.

    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(|e| format!("Windows would not let this set up a service: {e}"))?;
    let access = ServiceAccess::QUERY_STATUS
        | ServiceAccess::START
        | ServiceAccess::STOP
        | ServiceAccess::CHANGE_CONFIG;

    // An upgrade: stop the old one so its file can be replaced.
    let existing = manager.open_service(NAME, access).ok();
    if let Some(service) = existing.as_ref() {
        stop_and_wait(service);
    }

    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let target = installed_program();
    if !same_file(&me, &target) {
        let bytes = std::fs::read(&me).map_err(|e| format!("could not read this program: {e}"))?;
        if target.exists() {
            hub::update::swap_in(&target, &bytes)
        } else {
            std::fs::write(&target, &bytes)
        }
        .map_err(|e| format!("could not put the server in {}: {e}", home.display()))?;
    }

    // The Citadel cell is never installed from here: it isn't in View's public
    // release. An office licensed for it installs it afterwards with
    // `--install-citadel-cell` and the zip from its private IT kit; an
    // installed cell is left as it is (Fleet delivers it with each update).

    // Somebody may have run the plain program first and already set up an
    // office on it. Those drawings come along rather than being left behind
    // beside a copy of the program nobody will look at again.
    if let Some(beside) = me.parent().map(|d| d.join("hyperview-data")) {
        let data = data_folder();
        if beside != data
            && beside.join("hyperview.sqlite").exists()
            && !data.join("hyperview.sqlite").exists()
        {
            copy_folder(&beside, &data)
                .map_err(|e| format!("could not bring the existing drawings across: {e}"))?;
        }
    }

    let info = ServiceInfo {
        name: OsString::from(NAME),
        display_name: OsString::from(DISPLAY),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: target.clone(),
        launch_arguments: vec![OsString::from("--service")],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };
    let service = match existing {
        Some(service) => {
            service
                .change_config(&info)
                .map_err(|e| format!("could not update the service: {e}"))?;
            service
        }
        None => manager
            .create_service(&info, access)
            .map_err(|e| format!("could not register the service: {e}"))?,
    };
    let _ = service.set_description(
        "Holds this company's drawings, markups and takeoffs for Excalibur Hyperview, and keeps \
         every seat in the office up to date.",
    );
    // Restart on failure — which is also how an update takes effect: the
    // server puts the new version in place and exits, and this brings it up.
    let _ = service.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 60 * 60)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![
            ServiceAction {
                action_type: ServiceActionType::Restart,
                delay: Duration::from_secs(5),
            },
            ServiceAction {
                action_type: ServiceActionType::Restart,
                delay: Duration::from_secs(5),
            },
            ServiceAction {
                action_type: ServiceActionType::Restart,
                delay: Duration::from_secs(60),
            },
        ]),
    });
    let _ = service.set_failure_actions_on_non_crash_failures(true);

    open_firewall()?;

    match service.start::<&str>(&[]) {
        Ok(()) => {}
        // Already running is not a failure.
        Err(e) if standing() == Standing::Running => {
            let _ = e;
        }
        Err(e) => return Err(format!("the service would not start: {e}")),
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if standing() == Standing::Running {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    Err("the service was installed but did not report that it had started".into())
}

/// Takes the service and its firewall rules away. The drawings stay exactly
/// where they are: turning a computer off as the server is not asking for the
/// company's drawings to be deleted.
pub fn uninstall() -> Result<(), String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| e.to_string())?;
    if let Ok(service) = manager.open_service(
        NAME,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
    ) {
        stop_and_wait(&service);
        service.delete().map_err(|e| format!("could not remove the service: {e}"))?;
    }
    for rule in [FIREWALL_TCP, FIREWALL_UDP, crate::cell::FIREWALL_RULE] {
        let _ = netsh(&["advfirewall", "firewall", "delete", "rule", &format!("name={rule}")]); // WHAT: each of this server's rules, the cell's too. WHY: nothing left open
    }
    Ok(())
}

/// `hyperview-server --roll-back`: stops the service, puts the version
/// before the last update back (the server and its Citadel cell, together;
/// `crate::kept::roll_back`), and starts it again. Needs administrator
/// rights. What was done, in words.
///
/// # Errors
///
/// When the service can't be stopped, nothing is kept, or the rollback is
/// refused; the service is started again whatever happened.
pub fn roll_back() -> Result<String, String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).map_err(|e| format!("Windows would not let this reach the service manager ({e}); run it as an administrator"))?; // WHAT: the service manager. WHY: the service is stopped first
    let service = manager.open_service(NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::START).map_err(|e| format!("the Office Server's service couldn't be opened ({e}); run this as an administrator"))?; // WHAT: the service. WHY: stopped and started
    stop_and_wait(&service); // WHAT: stopped. WHY: its program and the cell's are swapped
    let config = crate::Config { data: data_folder(), ..crate::Config::default() }; // WHAT: the installed service's data. WHY: the rollback is written in its store
    let done = crate::Store::open(&config.database(), &config.blobs()).map_err(|e| e.to_string()).and_then(|store| crate::kept::roll_back(&store, &installed_program())); // WHAT: put back. WHY: crate::kept
    let started = service.start::<&str>(&[]); // WHAT: started again, as whichever version is now in place. WHY: the office is never left without its server
    match (done, started) {
        (Ok(said), Ok(())) => Ok(said), // WHAT: done and running. WHY: said
        (Ok(said), Err(e)) => Err(format!("{said}, but the service didn't start again ({e}); start \"{DISPLAY}\" in Services")), // WHAT: said. WHY: someone must act
        (Err(why), _) => Err(why), // WHAT: refused or failed; the service started again as it was. WHY: said
    }
}

/// `hyperview-server --install-citadel-cell "<zip>"`: an administrator
/// installs the Citadel cell from the office's private IT kit. Refused unless
/// the Office Server is installed, its licence lists `citadel`, and the zip is
/// the one this build pins; then the pinned files are checked, left waiting
/// as `citadel.new`, and the service is restarted, which checks the cell once
/// more, puts it in place and starts it. What was done, in words.
///
/// # Errors
///
/// When any of those isn't so; nothing is installed then.
pub fn install_citadel_cell(zip: &Path) -> Result<String, String> {
    if !installed_program().is_file() {
        return Err("the Office Server isn't installed here yet; install it first, then the Citadel cell".to_string()); // WHAT: refused. WHY: the cell runs inside the installed service
    }
    let mut config = crate::Config::from_environment(); // WHAT: the server's settings. WHY: its licence keys and data
    config.data = data_folder(); // WHAT: the installed service's data. WHY: its licence is there
    config.program_folder = Some(home()); // WHAT: the service's folder. WHY: the cell goes beside it
    let store = crate::Store::open(&config.database(), &config.blobs()).map_err(|e| format!("the Office Server's data couldn't be opened ({e})"))?; // WHAT: its store. WHY: the licence it holds
    let server = crate::api::Server::new(store, config); // WHAT: the server's view of its licence, the data folder's licence file taken as at a start. WHY: the same rule the service applies
    if !crate::office::licensed(&server).contains(&hub::license::Module::Citadel) {
        return Err(format!("{}; nothing was installed. Put the office's licence (listing citadel) in {} and run this again.", crate::cellbundle::NOT_LICENSED, data_folder().display())); // WHAT: refused. WHY: Creede, 2026-11-13: only licensed offices hold the cell
    }
    let size = std::fs::metadata(zip).map_err(|e| format!("{} couldn't be read ({e})", zip.display()))?.len(); // WHAT: its size. WHY: checked before it is read
    if size > crate::cellbundle::ZIP_MOST {
        return Err(format!("{} is too big to be the Citadel cell's zip", zip.display())); // WHAT: refused. WHY: never read whole
    }
    let bytes = std::fs::read(zip).map_err(|e| format!("{} couldn't be read ({e})", zip.display()))?; // WHAT: the zip. WHY: checked against the pin
    let pin = crate::cellbundle::pinned()?; // WHAT: the cell this build pins. WHY: only that one
    crate::cellbundle::stage_zip(&bytes, &home(), &pin)?; // WHAT: checked and waiting. WHY: put in place by the service's next start
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).map_err(|e| format!("the cell waits, but Windows would not let this reach the service manager ({e}); restart \"{DISPLAY}\" in Services"))?; // WHAT: the service manager. WHY: restarted next
    let service = manager.open_service(NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::START).map_err(|e| format!("the cell waits, but the service couldn't be opened ({e}); restart \"{DISPLAY}\" in Services"))?; // WHAT: the service. WHY: likewise
    stop_and_wait(&service); // WHAT: stopped. WHY: the cell's folder is swapped at the next start
    service.start::<&str>(&[]).map_err(|e| format!("the cell waits, but the service didn't start again ({e}); start \"{DISPLAY}\" in Services"))?; // WHAT: started. WHY: it puts the cell in place and starts it
    Ok(format!("The Citadel cell {} was checked against this Office Server's pin and installed. The service restarted, and starts the cell now.", pin.version)) // WHAT: said. WHY: a person reads it
}

fn stop_and_wait(service: &windows_service::service::Service) {
    let _ = service.stop();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        match service.query_status().map(|s| s.current_state) {
            Ok(ServiceState::Stopped) | Err(_) => return,
            _ => std::thread::sleep(Duration::from_millis(300)),
        }
    }
}

/// Lets the office network in, and nobody else.
///
/// Two rules, because a seat finds the server on one port and talks to it on
/// another. Both are for the local subnet only — a shop's drawings have no
/// business being reachable from outside the building — but for every network
/// profile: Windows calls a brand-new network "Public" until somebody tells it
/// otherwise, and a rule for "Private" only is how a server nobody can find
/// happens.
fn open_firewall() -> Result<(), String> {
    let port = crate::config::DEFAULT_PORT.to_string();
    let finding = hub::discover::PORT.to_string();
    for (name, protocol, number) in [
        (FIREWALL_TCP, "TCP", port.as_str()),
        (FIREWALL_UDP, "UDP", finding.as_str()),
    ] {
        let _ = netsh(&["advfirewall", "firewall", "delete", "rule", &format!("name={name}")]);
        let added = netsh(&[
            "advfirewall",
            "firewall",
            "add",
            "rule",
            &format!("name={name}"),
            "dir=in",
            "action=allow",
            &format!("protocol={protocol}"),
            &format!("localport={number}"),
            "profile=any",
            "remoteip=localsubnet",
        ])?;
        if !added {
            return Err(format!("the firewall would not let {protocol} {number} in"));
        }
    }
    let cell = crate::cellbundle::program_in(&home()); // WHAT: the cell's program, where it is installed. WHY: its rule is for it alone
    let _ = netsh(&["advfirewall", "firewall", "delete", "rule", &format!("name={}", crate::cell::FIREWALL_RULE)]); // WHAT: an older copy of the rule gone. WHY: added fresh, as the two above
    let rule = crate::cell::firewall_rule(&cell); // WHAT: TCP 47110 for the cell's program, office network only. WHY: critical 2: nothing opened 47110 for the cell
    if !netsh(&rule.iter().map(String::as_str).collect::<Vec<_>>())? {
        return Err(format!("the firewall would not let TCP {} in for the Citadel cell", crate::cell::PORT)); // WHAT: said. WHY: estimating couldn't reach the cell
    }
    Ok(())
}

/// Adds the cell's firewall rule when it is missing, at the cell's first
/// start: an office installed before the rule existed gets it from its
/// update.
pub fn cell_firewall(program: &Path) {
    let shown = crate::cell::firewall_rule_shown(); // WHAT: asked for by name. WHY: added only when missing
    if matches!(netsh(&shown.iter().map(String::as_str).collect::<Vec<_>>()), Ok(true)) {
        return; // WHAT: already there. WHY: nothing to do
    }
    let rule = crate::cell::firewall_rule(program); // WHAT: the rule. WHY: critical 2
    match netsh(&rule.iter().map(String::as_str).collect::<Vec<_>>()) {
        Ok(true) => tracing::warn!("the firewall now lets the office network reach the Citadel cell on TCP {}", crate::cell::PORT), // WHAT: said. WHY: whoever looks after the server can see it
        _ => tracing::error!("the firewall would not let TCP {} in for the Citadel cell; estimating computers can't reach it", crate::cell::PORT), // WHAT: the alarm. WHY: someone must act
    }
}

/// Runs a program from `System32` with arguments, without a window; whether
/// it succeeded.
pub fn system_tool(tool: &str, args: &[String]) -> bool {
    use std::os::windows::process::CommandExt; // WHAT: Windows-only process options. WHY: no console window
    const NO_WINDOW: u32 = 0x0800_0000; // WHAT: CREATE_NO_WINDOW. WHY: a service shows no windows
    let system = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from); // WHAT: Windows' folder. WHY: the tool by its full path
    std::process::Command::new(system.join("System32").join(tool)).args(args).creation_flags(NO_WINDOW).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success()) // WHAT: run and waited for. WHY: never one found on the PATH
}

fn netsh(args: &[&str]) -> Result<bool, String> {
    use std::os::windows::process::CommandExt;
    const NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("netsh")
        .args(args)
        .creation_flags(NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .map_err(|e| format!("netsh: {e}"))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn copy_folder(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let there = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_folder(&entry.path(), &there)?;
        } else {
            std::fs::copy(entry.path(), there)?;
        }
    }
    Ok(())
}

// ---- running: what the service manager starts ------------------------------

define_windows_service!(ffi_service_main, service_main);

/// Hands this process to the service manager. Returns when the service stops.
pub fn run() -> Result<(), String> {
    service_dispatcher::start(NAME, ffi_service_main).map_err(|e| e.to_string())
}

fn service_main(_arguments: Vec<OsString>) {
    if let Err(e) = serve_as_service() {
        tracing::error!("the service stopped: {e}");
    }
}

fn serve_as_service() -> Result<(), String> {
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let stop = Mutex::new(Some(stop));
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            if let Some(stop) = stop.lock().ok().and_then(|mut s| s.take()) {
                let _ = stop.send(());
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let status = service_control_handler::register(NAME, handler).map_err(|e| e.to_string())?;
    let report = |state: ServiceState, code: u32| {
        let _ = status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code: ServiceExitCode::Win32(code),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        });
    };
    report(ServiceState::Running, 0);

    let _ = std::fs::remove_file(installed_program().with_extension("new")); // WHAT: a half-written update gone. WHY: the version before (.old) is kept a day for a rollback (crate::kept), never deleted here
    let mut config = crate::Config::from_environment();
    config.data = data_folder();
    config.check_for_updates = true;
    config.replace_self = true;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let served = runtime.block_on(crate::run::serve(
        config,
        async move {
            let _ = stopped.await;
        },
        |serving| tracing::info!("{} is serving at {}", serving.name, serving.url()),
    ));
    let code = match &served {
        Ok(()) => 0,
        Err(e) => {
            tracing::error!("{e:#}");
            1
        }
    };
    report(ServiceState::Stopped, code);
    served.map_err(|e| e.to_string())
}
