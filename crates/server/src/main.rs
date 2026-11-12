//! Hyperview-Server.exe.
//!
//! One of the two files a company is given. Double-clicked on the office
//! server, it explains in three lines what it is about to do, Windows asks
//! for permission once, and it installs itself as a proper Windows service:
//! copied somewhere permanent, started at boot with nobody signed in, the
//! firewall opened for the office network only, and keeping itself and every
//! seat up to date from then on. Double-clicked again later, it says it is
//! already running and where.
//!
//! Nobody is shown a password and nobody types an address. The first person
//! to open Hyperview on any computer in the office finds this server and sets
//! it up, becoming its administrator.
//!
//! Elsewhere — a Linux box, or anybody who wants it in a window — it runs in
//! the console exactly as it always has: `hyperview-server --console`.
//!
//! The one thing a console program must never do is vanish. A window that
//! disappears half a second after a double-click, taking the reason with it,
//! is the worst failure this program could have on the one machine nobody is
//! sitting in front of. So every path out of the double-click prints what
//! happened and waits for Enter.

use anyhow::Result;
use hyperview_server::{run, Config, Store, VERSION};

/// Keeps the version readable off the file itself, so a newer copy
/// double-clicked on the server can tell it is newer than the one installed.
static BUILD_MARK: &str = hub::build_mark!();

fn main() {
    std::hint::black_box(BUILD_MARK);
    let given: Vec<String> = std::env::args().take(6).collect(); // WHAT: the program's name and its first five words, read once. WHY: the command, and the device code --make-admin takes, which may be typed as three groups
    let first = given.get(1).cloned().unwrap_or_default();
    match first.as_str() {
        #[cfg(windows)]
        "--service" => {
            log_to_file(&hyperview_server::winsvc::data_folder());
            if let Err(e) = hyperview_server::winsvc::run() {
                tracing::error!("could not run as a service: {e}");
                std::process::exit(1);
            }
        }
        #[cfg(windows)]
        "--install" => {
            let result = hyperview_server::winsvc::install();
            let report = match &result {
                Ok(()) => "ok\n".to_string(),
                Err(why) => format!("error\n{why}\n"),
            };
            let _ = std::fs::create_dir_all(hyperview_server::winsvc::home());
            let _ = std::fs::write(hyperview_server::winsvc::report_file(), report);
            std::process::exit(if result.is_ok() { 0 } else { 1 });
        }
        #[cfg(windows)]
        "--uninstall" => match hyperview_server::winsvc::uninstall() {
            Ok(()) => println!(
                "  Removed. The drawings are still in {}.",
                hyperview_server::winsvc::data_folder().display()
            ),
            Err(why) => {
                eprintln!("  {why}");
                std::process::exit(1);
            }
        },
        "--offsite-backup" | "--offsite-drill" => { // WHAT: a backup now, or the restore drill (README.md, Off-site backups). WHY: the server runs the backup each night itself; these are for a person.
            let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect(); // WHAT: the words after the program, never the program's own name. WHY: a folder name that isn't valid Unicode reaches the command (as close as it can be written) instead of stopping the program.
            #[allow(unused_mut)] // WHAT: only Windows changes it. WHY: no warning elsewhere.
            let mut config = Config::from_environment(); // WHAT: the server's own settings. WHY: the same target, work folder and data as the night's.
            #[cfg(windows)]
            if std::env::var_os("HYPERVIEW_DATA").is_none() && hyperview_server::winsvc::data_folder().exists() { config.data = hyperview_server::winsvc::data_folder(); } // WHAT: the installed service's drawings. WHY: the service keeps them in ProgramData, not beside this program.
            std::process::exit(hyperview_server::offsite::main_from_args(&hyperview_server::offsite::VIEW, &config, VERSION, &args)); // WHAT: run it and end with its code. WHY: 0 good, 5 good with a warning, anything else says why.
        }
        "--move-cad" => { // WHAT: Excalibur CAD's server's drawings moved into this server's cad part. WHY: one server for the office (docs/OFFICE-SERVER.md)
            let Some(from) = std::env::args_os().nth(2) else {
                eprintln!("  Say which folder: hyperview-server --move-cad \"C:\\ProgramData\\Excalibur CAD Server\\data\""); // WHAT: how to use it. WHY: a person types it
                std::process::exit(2);
            };
            #[allow(unused_mut)] // WHAT: only Windows changes it. WHY: no warning elsewhere
            let mut config = Config::from_environment(); // WHAT: this server's settings. WHY: its data folder
            #[cfg(windows)]
            if std::env::var_os("HYPERVIEW_DATA").is_none() && hyperview_server::winsvc::data_folder().exists() { config.data = hyperview_server::winsvc::data_folder(); } // WHAT: the installed service's data. WHY: the same folder the service serves
            match hyperview_server::cad::move_from(std::path::Path::new(&from), &config.database(), &config.blobs()) {
                Ok(m) => println!("  Moved {} drawings ({} versions, {} files). {} were checked out there and are checked in here. The old CAD server's folder was not changed.", m.drawings, m.versions, m.files, m.released), // WHAT: what came. WHY: a person checks it against the old server
                Err(why) => {
                    eprintln!("  {why}");
                    std::process::exit(1);
                }
            }
        }
        "--move-archetype" => { // WHAT: Archetype's server's jobs moved into this server's archetype part. WHY: one server for the office (docs/OFFICE-SERVER.md)
            let Some(from) = std::env::args_os().nth(2) else {
                eprintln!("  Say which folder: hyperview-server --move-archetype \"C:\\ProgramData\\Excalibur Archetype\\Server\\data\""); // WHAT: how to use it. WHY: a person types it
                std::process::exit(2);
            };
            #[allow(unused_mut)] // WHAT: only Windows changes it. WHY: no warning elsewhere
            let mut config = Config::from_environment(); // WHAT: this server's settings. WHY: its data folder
            #[cfg(windows)]
            if std::env::var_os("HYPERVIEW_DATA").is_none() && hyperview_server::winsvc::data_folder().exists() { config.data = hyperview_server::winsvc::data_folder(); } // WHAT: the installed service's data. WHY: the same folder the service serves
            match hyperview_server::archetype::move_from(std::path::Path::new(&from), &config.database(), &config.blobs()) {
                Ok(m) => println!("  Moved {} jobs ({} check-ins, {} files). {} were checked out there and are given back here. The old Archetype server's folder was not changed.", m.jobs, m.checkins, m.files, m.released), // WHAT: what came. WHY: a person checks it against the old server
                Err(why) => {
                    eprintln!("  {why}");
                    std::process::exit(1);
                }
            }
        }
        "--reopen-claim" | "--make-admin" | "--reset-admins" | "--retry-citadel-move" => device_console(&first, Some(given.get(2..).map(|rest| rest.join(" ")).unwrap_or_default().as_str()).filter(|code| !code.is_empty())), // WHAT: the device list's console commands. WHY: §3 and the security review's M3; on the server itself
        "--console" | "--serve" => console(),
        #[cfg(windows)]
        "" => front_door(),
        _ => console(),
    }
}

/// The device list's commands, on the server itself: `--reopen-claim`,
/// `--make-admin <device code or id>` and `--reset-admins`. Each needs the server's
/// own data folder, so it runs only on the server, and each is written in
/// the audit log.
fn device_console(command: &str, id: Option<&str>) {
    #[allow(unused_mut)] // WHAT: only Windows changes it. WHY: no warning elsewhere
    let mut config = Config::from_environment(); // WHAT: this server's settings. WHY: its data folder
    #[cfg(windows)]
    if std::env::var_os("HYPERVIEW_DATA").is_none() && hyperview_server::winsvc::data_folder().exists() { config.data = hyperview_server::winsvc::data_folder(); } // WHAT: the installed service's data. WHY: the same folder the service serves
    let done = Store::open(&config.database(), &config.blobs()).and_then(|store| match command {
        "--reopen-claim" => hyperview_server::devices::reopen_claim(&store).map(|until| format!("  The first computer to ask, until {until}, becomes the office's admin.")), // WHAT: claiming open for a day. WHY: only while the office has no admin
        "--retry-citadel-move" => hyperview_server::cellmove::retry(&store).map(|()| "  The failed Citadel move is cleared. Restart the Office Server service and it tries once more.".to_string()), // WHAT: a failure cleared. WHY: the console way to try the move again
        "--make-admin" => hyperview_server::devices::make_admin_here(&store, id.unwrap_or_default()).map(|id| format!("  The computer {id} is approved, and an admin.")), // WHAT: one computer made an admin. WHY: the way back when every admin computer is lost
        _ => hyperview_server::devices::reset_admins_here(&store).map(|until| format!("  No computer is an admin now. Until {until}, the next NEW computer to ask from the office network becomes the admin, whoever's it is.\n  To pick a computer you know instead, use: hyperview-server --make-admin <device code>, with the 12-digit code its Excalibur app shows (for example 29AC 6B9D EFA9).")), // WHAT: every admin unmade, claiming open, said plainly. WHY: audit: the next new computer to ask takes the office, so the safer way is named
    });
    match done {
        Ok(said) => println!("{said}"), // WHAT: what was done. WHY: a person reads it
        Err(why) => {
            eprintln!("  {why}");
            std::process::exit(1);
        }
    }
}

fn console() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hyperview_server=info,tower_http=warn".into()),
        )
        .init();
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("  Could not start: {e}");
            std::process::exit(1);
        }
    };
    if let Err(trouble) = runtime.block_on(serve()) {
        eprintln!();
        eprintln!("  The server could not start.");
        eprintln!();
        eprintln!("      {trouble}");
        eprintln!();
        eprintln!("  {}", advice_about(&trouble));
        eprintln!();
        hold_the_window_open();
        std::process::exit(1);
    }
}

async fn serve() -> Result<()> {
    let mut config = Config::from_environment();
    // A console server still mirrors new versions for its seats. It does not
    // replace itself: there is no service manager to start it again.
    config.check_for_updates = true;
    std::fs::create_dir_all(&config.data).map_err(|e| {
        anyhow::anyhow!(
            "the folder for the drawings ({}) could not be made: {e}",
            config.data.display()
        )
    })?;

    // Nobody is made an administrator here by default. The first person to
    // open Hyperview and find this server sets it up with their own details
    // and gets the join code for everybody else. A headless box that will
    // never have a person near a copy of Hyperview can still ask for the old
    // behaviour: HYPERVIEW_FIRST_ADMIN=1 prints a password once.
    if std::env::var("HYPERVIEW_FIRST_ADMIN").is_ok() {
        let store = Store::open(&config.database(), &config.blobs())?;
        if store.is_empty()? {
            let password = run::first_administrator(&store)?;
            println!();
            println!("  A first administrator has been created.");
            println!();
            println!("      email     admin@localhost");
            println!("      password  {password}");
            println!();
            println!("  Write that down. It is not stored anywhere it can be read back,");
            println!("  and it will not be printed again.");
            println!();
        }
    }

    let where_things_live = config.data.clone();
    run::serve(config, run::on_a_signal(), move |serving| {
        println!();
        println!("  {} {VERSION} is serving at {}", serving.name, serving.url());
        println!("  the drawings live in {}", where_things_live.display());
        println!(
            "  the description of the API is at {}/api/v1/openapi.json",
            serving.url()
        );
        println!(
            "  an assistant can ask about the quantities at {}/mcp",
            serving.url()
        );
        println!();
        if !serving.claimed {
            println!("  Nobody has set this server up yet. Open Excalibur View on any machine");
            println!("  in the building and it will offer it — no address to type.");
            println!();
        }
        println!("  Leave this window open. Closing it stops the server.");
        println!();
    })
    .await?;

    println!();
    println!("  Stopped. Nothing was lost; the drawings are still where they were.");
    println!();
    Ok(())
}

// ---- the double-click, on Windows ------------------------------------------

#[cfg(windows)]
fn front_door() {
    use hyperview_server::winsvc::{self, Standing};

    println!();
    println!("  EXCALIBUR VIEW SERVER  {VERSION}");
    println!();

    let installed = hub::update::version_in_file(&winsvc::installed_program());
    match (winsvc::standing(), installed) {
        (Standing::NotInstalled, _) => {
            println!("  This makes this computer the company's drawing server.");
            println!();
            println!("  It installs itself as a Windows service, so it keeps serving after");
            println!("  a restart and with nobody signed in, and it keeps itself and every");
            println!("  copy of Excalibur View in the office up to date.");
            println!();
            println!("  Windows will ask for permission once.");
            println!();
            elevate_and_report();
        }
        (standing, Some(running)) if hub::update::newer(VERSION, &running) => {
            let _ = standing;
            println!("  Version {running} is installed here. Updating it to {VERSION}.");
            println!();
            println!("  Windows will ask for permission once.");
            println!();
            elevate_and_report();
        }
        (Standing::Running, running) | (Standing::Starting, running) => {
            let running = running.unwrap_or_else(|| "?".into());
            println!("  It is already running on this computer (version {running}).");
            println!();
            println!("      {}", address());
            println!();
            println!("  It starts by itself with Windows and updates itself. There is");
            println!("  nothing to do here.");
            println!();
        }
        (Standing::Stopped, _) => {
            println!("  It is installed here but stopped. Starting it again.");
            println!();
            println!("  Windows will ask for permission once.");
            println!();
            elevate_and_report();
        }
    }
    hold_the_window_open();
}

/// Starts a second copy of this program with administrator rights to do the
/// installing, waits for it, and says what it came to.
#[cfg(windows)]
fn elevate_and_report() {
    use hyperview_server::winsvc;
    let report = winsvc::report_file();
    let _ = std::fs::remove_file(&report);
    let me = match std::env::current_exe() {
        Ok(me) => me,
        Err(e) => {
            println!("  Could not find this program's own file: {e}");
            return;
        }
    };
    println!("  Working…");
    let code = run_as_administrator(&me, "--install");
    println!();
    if code == 1223 {
        println!("  Windows did not get permission, so nothing was changed.");
        println!("  Double-click this again and choose Yes when Windows asks.");
        println!();
        return;
    }
    let said = std::fs::read_to_string(&report).unwrap_or_default();
    let mut lines = said.lines();
    match lines.next() {
        Some("ok") => {
            println!("  Done. This computer is now the company's drawing server.");
            println!();
            println!("      {}", address());
            println!();
            println!("  It starts by itself whenever this computer does, and keeps itself");
            println!("  and every copy of Excalibur View in the office up to date. You can");
            println!("  close this window.");
            println!();
            println!("  Next: open Excalibur View on your own computer. It finds this server by");
            println!("  itself and asks you to set it up. You become its administrator and");
            println!("  get a join code for everybody else.");
            println!();
        }
        _ => {
            let why: Vec<&str> = lines.collect();
            println!("  That did not work, and nothing is half done.");
            println!();
            if why.is_empty() {
                println!("  The installer stopped without saying why (code {code}).");
            } else {
                for line in why {
                    println!("      {line}");
                }
            }
            println!();
        }
    }
}

/// Starts this program again with administrator rights and waits for it.
///
/// Asked of Windows directly — the same "runas" a right-click → Run as
/// administrator uses — rather than through PowerShell, which a hardened
/// server may not let anybody start. The second copy runs with no window of
/// its own; this one reports what it did. 1223 means somebody said no.
#[cfg(windows)]
fn run_as_administrator(program: &std::path::Path, arguments: &str) -> u32 {
    use std::ffi::c_void;

    #[repr(C)]
    struct ShellExecuteInfo {
        size: u32,
        mask: u32,
        window: isize,
        verb: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
        instance: isize,
        id_list: *mut c_void,
        class: *const u16,
        class_key: isize,
        hot_key: u32,
        icon_or_monitor: isize,
        process: isize,
    }
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteExW(info: *mut ShellExecuteInfo) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn WaitForSingleObject(handle: isize, millis: u32) -> u32;
        fn GetExitCodeProcess(handle: isize, code: *mut u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetLastError() -> u32;
    }
    const SEE_MASK_NOCLOSEPROCESS: u32 = 0x40;
    const SEE_MASK_NOASYNC: u32 = 0x100;
    const SW_HIDE: i32 = 0;
    const INFINITE: u32 = 0xFFFF_FFFF;

    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let verb = wide("runas");
    let file = wide(&program.display().to_string());
    let parameters = wide(arguments);
    let mut info = ShellExecuteInfo {
        size: std::mem::size_of::<ShellExecuteInfo>() as u32,
        mask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        window: 0,
        verb: verb.as_ptr(),
        file: file.as_ptr(),
        parameters: parameters.as_ptr(),
        directory: std::ptr::null(),
        show: SW_HIDE,
        instance: 0,
        id_list: std::ptr::null_mut(),
        class: std::ptr::null(),
        class_key: 0,
        hot_key: 0,
        icon_or_monitor: 0,
        process: 0,
    };
    unsafe {
        if ShellExecuteExW(&mut info) == 0 {
            return GetLastError();
        }
        if info.process == 0 {
            return 1;
        }
        WaitForSingleObject(info.process, INFINITE);
        let mut code = 1u32;
        GetExitCodeProcess(info.process, &mut code);
        CloseHandle(info.process);
        code
    }
}

#[cfg(windows)]
fn address() -> String {
    match run::local_address() {
        Some(ip) => format!("http://{ip}:{}", hyperview_server::config::DEFAULT_PORT),
        None => format!("http://localhost:{}", hyperview_server::config::DEFAULT_PORT),
    }
}

/// A service has no console, so it writes down what it does next to the
/// drawings, where the Fleet's log view also reads it.
#[cfg(windows)]
fn log_to_file(folder: &std::path::Path) {
    let _ = std::fs::create_dir_all(folder);
    let path = folder.join("hyperview-server.log");
    // Kept short: one previous log, and a fresh one once it passes 10 MB.
    if std::fs::metadata(&path).map(|m| m.len() > 10 * 1024 * 1024).unwrap_or(false) {
        let _ = std::fs::rename(&path, folder.join("hyperview-server.log.1"));
    }
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let path = path.clone(); // WHAT: kept for moving the file aside. WHY: `Rotating` reopens it
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hyperview_server=info,tower_http=warn".into()),
        )
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(Rotating { file, path, written: 0 }))
        .init();
}

/// WHAT: the service's log file, moved aside to `.1` once it passes 10 MB while
/// the server runs. WHY: it was only moved at start, so a flood of refusals
/// between restarts could fill the disk (audit, 2026-11-11).
#[cfg(windows)]
struct Rotating {
    file: std::fs::File,   // WHAT: the file written now. WHY: replaced when moved aside
    path: std::path::PathBuf, // WHAT: where it lives. WHY: reopened there
    written: u64,          // WHAT: bytes since the size was last looked at. WHY: looked at once a megabyte, not every line
}

#[cfg(windows)]
impl std::io::Write for Rotating {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        use std::io::Write as _; // WHAT: the file's own write. WHY: called below
        let n = self.file.write(bytes)?; // WHAT: written. WHY: the log line
        self.written += n as u64; // WHAT: counted. WHY: as above
        if self.written > 1024 * 1024 {
            self.written = 0; // WHAT: counted afresh. WHY: the next look in a megabyte
            if self.file.metadata().is_ok_and(|m| m.len() > 10 * 1024 * 1024) {
                let _ = std::fs::rename(&self.path, self.path.with_extension("log.1")); // WHAT: moved aside, over the older one. WHY: one previous log kept, as at start
                if let Ok(fresh) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
                    self.file = fresh; // WHAT: a new file. WHY: the log goes on
                }
            }
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        use std::io::Write as _; // WHAT: as above. WHY: as above
        self.file.flush() // WHAT: passed on. WHY: as the file does
    }
}

/// What to try, in the cases a person can actually do something about.
fn advice_about(trouble: &anyhow::Error) -> String {
    let said = format!("{trouble:#}").to_lowercase();
    if said.contains("address in use") || said.contains("addrinuse") {
        return format!(
            "Something else on this computer is already using port {}. Either that \
             is another copy of this server already running — in which case there \
             is nothing to do — or you can move this one with \
             HYPERVIEW_LISTEN=0.0.0.0:<another port>.",
            hyperview_server::config::DEFAULT_PORT
        );
    }
    if said.contains("permission") || said.contains("denied") || said.contains("access") {
        return "Windows would not let this program listen for other computers, or \
                would not let it write where it wanted to. Allow it through the \
                firewall when it asks, and try putting it in a folder you own — \
                Documents, say, rather than Program Files."
            .into();
    }
    if said.contains("could not be made") {
        return "Put this program somewhere it is allowed to write, or set \
                HYPERVIEW_DATA to a folder it can use."
            .into();
    }
    "If this keeps happening, the whole of the message above is the useful part \
     of it."
        .into()
}

/// Keeps a double-clicked console window on screen long enough to be read.
///
/// Only when there is nobody redirecting the output: a server run from a
/// script, a service or a pipe must exit rather than sit waiting for a key
/// that is never coming.
fn hold_the_window_open() {
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return;
    }
    println!("  Press Enter to close this window.");
    let mut nothing = String::new();
    let _ = std::io::stdin().read_line(&mut nothing);
}
