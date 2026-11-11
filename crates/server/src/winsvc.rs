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
    for rule in [FIREWALL_TCP, FIREWALL_UDP] {
        let _ = netsh(&["advfirewall", "firewall", "delete", "rule", &format!("name={rule}")]);
    }
    Ok(())
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
    Ok(())
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

    hub::update::tidy_after_update(&installed_program());
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
