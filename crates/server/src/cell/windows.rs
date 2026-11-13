//! The Windows half of the Citadel cell: the folders locked, the pipe made
//! with its permissions, and the cell started under a restricted token.
//!
//! Everything decided is decided in `super`: the command line, the SIDs, the
//! SDDL, the icacls steps, the restart rule and every answer. This file only
//! hands those to Windows.

use std::ffi::OsStr;
use std::io::BufReader;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::time::Instant;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, LocalFree, SetHandleInformation, ERROR_PIPE_CONNECTED, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, PSID, SECURITY_ATTRIBUTES, SID_AND_ATTRIBUTES, TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, CreatePipe, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};
use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE};
use windows_sys::Win32::System::Threading::{CreateProcessAsUserW, DeleteProcThreadAttributeList, GetCurrentProcess, GetExitCodeProcess, InitializeProcThreadAttributeList, OpenProcessToken, ResumeThread, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, INFINITE, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW};

use crate::api::Shared;

/// A string as Windows wants it: UTF-16, ended by a zero.
fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect() // WHAT: UTF-16 and its end. WHY: every W call takes this
}

/// A handle that may cross threads. Windows handles are process-wide.
#[derive(Clone, Copy)]
struct Raw(usize); // WHAT: the handle as a number. WHY: a raw pointer can't be sent between threads, a handle can

impl Raw {
    fn handle(self) -> HANDLE {
        self.0 as HANDLE // WHAT: back to a handle. WHY: for the call
    }
}

/// Starts supervising the cell, on a thread of its own, when the licence
/// lists `citadel` and `citadel\Citadel Server.exe` is beside this program.
pub fn start(server: Shared) {
    let installed = crate::winsvc::installed_program(); // WHAT: where the service's program is installed. WHY: its own known place, never a path the running program reports about itself
    let Some(folder) = server.config.program_folder.clone().or_else(|| installed.parent().map(Path::to_path_buf)) else { return }; // WHAT: the program's folder. WHY: the cell is beside it, in `citadel\`
    let install = crate::api::installation_id(&server); // WHAT: this installation's id. WHY: the pipe's name and the cell's SID
    let program_data = std::env::var_os("ProgramData").map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from); // WHAT: ProgramData. WHY: the cell's data lives there
    let data = super::data_folder(&program_data); // WHAT: `ProgramData\Excalibur Office\citadel`. WHY: §6
    let (program, args) = super::command(&folder, &install, &data); // WHAT: what to start. WHY: §6a
    let pin = crate::cellbundle::pinned(); // WHAT: the cell this build was made with. WHY: only that program is ever run
    if let Ok(pin) = &pin {
        let until = Instant::now() + std::time::Duration::from_secs(10); // WHAT: up to ten seconds. WHY: review low 1: the last server's cell may still be ending (its job is killed when that server exits)
        loop {
            match crate::cellbundle::swap_staged(&folder, pin) {
                Ok(true) => tracing::warn!("the Citadel cell {} that came with this update was checked and put in place", pin.version), // WHAT: said. WHY: the cell updates with the server
                Ok(false) => {} // WHAT: nothing waiting. WHY: the ordinary start
                Err(why) if why.starts_with(crate::cellbundle::BUSY) && Instant::now() < until => {
                    std::thread::sleep(std::time::Duration::from_millis(500)); // WHAT: a moment. WHY: a running program's folder can't be renamed until it ends
                    continue; // WHAT: tried again. WHY: within the ten seconds
                }
                Err(why) => tracing::error!("ALARM: {why}"), // WHAT: the alarm; nothing was half swapped. WHY: the cell before stays whole, but an admin should know
            }
            break; // WHAT: done. WHY: put in place, nothing waiting, or the alarm
        }
    }
    if !super::should_start(&crate::office::licensed(&server), program.is_file()) {
        return; // WHAT: no licence for it, or no program. WHY: §6: the cell runs only then
    }
    let pin = match pin.and_then(|pin| crate::cellbundle::check_program(&program, &pin).map(|()| pin)) {
        Ok(pin) => pin, // WHAT: the pinned program is there. WHY: it may run
        Err(why) => {
            tracing::error!("ALARM: the Citadel cell was not started: {why}"); // WHAT: the alarm. WHY: risk 8: never run a cell that didn't come from the signed bundle
            return;
        }
    };
    let spawned = std::thread::Builder::new().name("citadel-cell".into()).spawn(move || {
        let sid = super::cell_sid(&install); // WHAT: the cell's own SID. WHY: the pipe, its data and its program grant it
        if let Some(office) = data.parent() {
            let _ = std::fs::create_dir_all(office); // WHAT: `Excalibur Office`. WHY: made before it is locked
            crate::lockdown::lock_parent(office); // WHAT: the system's and administrators' alone. WHY: an account that made it first could swap the folder below
        }
        let _ = std::fs::create_dir_all(&data); // WHAT: the cell's data folder. WHY: made once
        let old = crate::cellmove::old_folder(&program_data); // WHAT: Citadel Server 1.0's folder. WHY: moved in before the cell's first start
        if let Err(why) = crate::cellbundle::check_program(&program, &pin) {
            tracing::error!("ALARM: the Citadel move was not run: {why}"); // WHAT: checked again just before it runs as the system. WHY: risk 8
            return;
        }
        if !crate::cellmove::before_first_start(&server.store, &OldCitadel, &crate::cellmove::Run, &program, &old, &data) {
            return; // WHAT: the move failed: the cell is not started. WHY: a cell started empty would make a new certificate; /api/v1/office shows the alarm
        }
        allow(&data, &super::data_sddl(&sid), true); // WHAT: the system and the cell only. WHY: §6
        icacls(&super::data_steps(&data)); // WHAT: the moved files inherit it, the system owns them. WHY: §6
        if let Some(own) = program.parent() {
            allow(own, &super::program_sddl(&sid), false); // WHAT: the cell may read and run its program. WHY: the service's folder is locked to the system and administrators
        }
        crate::winsvc::cell_firewall(&program); // WHAT: TCP 47110 open for the cell's program, when it isn't yet. WHY: critical 2: an office installed before the rule existed gets it here
        crate::cellmove::tidy_old_task(&server.store, &OldTask); // WHAT: 1.0's update task removed, or the alarm kept and tried again. WHY: review low 2
        let unpack = super::extract_folder(&folder, true); // WHAT: the cell's own unpacking folder. WHY: review low 3: never C:\Windows\Temp
        let _ = std::fs::create_dir_all(&unpack); // WHAT: made; it inherits the locked service folder's ACL. WHY: the system and administrators only
        allow(&unpack, &super::extract_sddl(&sid), false); // WHAT: the cell may use it too. WHY: a restricted token unpacks there
        let environment = super::environment_block(std::env::vars_os(), super::EXTRACT_VARIABLE, unpack.as_os_str()); // WHAT: this server's environment, with .NET's unpacking folder set. WHY: review low 3
        let mut launcher = Cell { server: server.clone(), program, home: folder.clone(), args, pipe: format!(r"\\.\pipe\{}", super::pipe_name(&install)), sid, process: None, serving: None, next: None, pin, data, environment }; // WHAT: the cell, ready to start. WHY: its command line is made at each start, with its key
        super::supervise(&mut launcher, &server.cell, &std::thread::sleep, &Instant::now, usize::MAX); // WHAT: run and restarted. WHY: §6a
    });
    if let Err(e) = spawned {
        tracing::error!("the Citadel cell's supervisor could not start: {e}"); // WHAT: said. WHY: the part then reports what its port shows
    }
}

/// Runs each icacls step, saying in the log, with icacls' own words, when
/// one fails.
fn icacls(steps: &[Vec<std::ffi::OsString>]) {
    use std::os::windows::process::CommandExt; // WHAT: Windows-only process options. WHY: no console window
    let system = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from); // WHAT: Windows' folder. WHY: icacls by its full path
    let program = system.join("System32").join("icacls.exe"); // WHAT: the program. WHY: never one found on the PATH
    for args in steps {
        match std::process::Command::new(&program).args(args).creation_flags(CREATE_NO_WINDOW).stdin(std::process::Stdio::null()).output() {
            Ok(done) if done.status.success() => {} // WHAT: done. WHY: nothing to say
            Ok(done) => tracing::warn!("could not set who may use {:?}: icacls said {}", args.first(), String::from_utf8_lossy(&done.stdout).trim().chars().take(300).collect::<String>()), // WHAT: said, with icacls' reason. WHY: whoever looks after the server can see why
            Err(e) => tracing::warn!("could not set who may use {:?}: icacls didn't run ({e})", args.first()), // WHAT: said. WHY: likewise
        }
    }
}

/// Sets a folder's own entries from SDDL with Windows' own call, saying in
/// the log when it can't. `protected`: nothing inherited from above; else
/// the entries are added to what the folder inherits.
fn allow(folder: &Path, sddl: &str, protected: bool) {
    if let Err(e) = set_acl(folder, sddl, protected) {
        tracing::warn!("could not set who may use {}: {e}", folder.display()); // WHAT: said. WHY: whoever looks after the server can see it
    }
}

/// The work of [`allow`]: the SDDL made into a security descriptor (a SID is
/// read as written, so the cell's SID needs no account behind it), its DACL
/// set on the folder; Windows passes the inheritable entries on to what is
/// inside.
fn set_acl(folder: &Path, sddl: &str, protected: bool) -> std::io::Result<()> {
    use windows_sys::Win32::Security::Authorization::{SetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{GetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, UNPROTECTED_DACL_SECURITY_INFORMATION};
    let text = wide(OsStr::new(sddl)); // WHAT: the SDDL. WHY: as Windows takes it
    let path = wide(folder.as_os_str()); // WHAT: the folder. WHY: likewise
    // SAFETY: every pointer passed is to a live local for the length of the call; the descriptor Windows allocates is
    // freed once, after SetNamedSecurityInfoW has copied the DACL it points into.
    unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: a folder's DACL set from SDDL
        let mut descriptor: *mut core::ffi::c_void = std::ptr::null_mut(); // WHAT: filled by Windows. WHY: the parsed SDDL
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(text.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) == 0 {
            return Err(std::io::Error::last_os_error()); // WHAT: refused. WHY: SDDL that doesn't read
        }
        let (mut present, mut defaulted, mut dacl) = (0, 0, std::ptr::null_mut::<ACL>()); // WHAT: filled by Windows. WHY: the DACL inside
        let found = GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted); // WHAT: the DACL. WHY: set next
        let done = if found == 0 || present == 0 {
            Err(std::io::Error::other("the SDDL has no DACL")) // WHAT: refused. WHY: never an empty grant
        } else {
            let how = DACL_SECURITY_INFORMATION | if protected { PROTECTED_DACL_SECURITY_INFORMATION } else { UNPROTECTED_DACL_SECURITY_INFORMATION }; // WHAT: the DACL, inheriting or not. WHY: as asked
            match SetNamedSecurityInfoW(path.as_ptr(), SE_FILE_OBJECT, how, std::ptr::null_mut(), std::ptr::null_mut(), dacl, std::ptr::null()) {
                0 => Ok(()), // WHAT: set. WHY: done
                code => Err(std::io::Error::from_raw_os_error(i32::try_from(code).unwrap_or(i32::MAX))), // WHAT: Windows' reason. WHY: said
            }
        };
        LocalFree(descriptor); // WHAT: let go. WHY: the folder holds its own copy
        done
    }
}

/// The cell, as the supervisor starts and waits for it.
struct Cell {
    server: Shared, // WHAT: the Office Server. WHY: the pipe's answers
    program: PathBuf, // WHAT: `citadel\Citadel Server.exe`. WHY: started
    home: PathBuf, // WHAT: the Office Server's own folder. WHY: the cell's key's name and certificate are kept there (crate::cellkey)
    args: Vec<String>, // WHAT: its arguments. WHY: the line is made from them
    pipe: String, // WHAT: `\\.\pipe\excalibur-office-<install id>`. WHY: §6a
    sid: String, // WHAT: the cell's SID. WHY: the pipe and the token
    process: Option<Raw>, // WHAT: the running cell. WHY: waited for
    serving: Option<std::thread::JoinHandle<(std::fs::File, Option<std::fs::File>)>>, // WHAT: the thread answering its pipe, which hands the pipe back when done, with the next instance it made once the cell connected. WHY: the old instance is held until the next exists
    next: Option<std::fs::File>, // WHAT: the next pipe instance, made before the old one closed. WHY: security review L4: the name is never free during the restart wait
    pin: crate::cellbundle::Pin, // WHAT: the cell this build was made with. WHY: checked before every start
    data: PathBuf, // WHAT: the cell's data folder. WHY: its settings get the Office Server's maintenance key before every start
    environment: Vec<std::ffi::OsString>, // WHAT: the cell's environment. WHY: .NET's unpacking folder set in it
}

impl super::Launcher for Cell {
    fn start(&mut self) -> std::io::Result<()> {
        crate::cellbundle::check_program(&self.program, &self.pin).map_err(std::io::Error::other)?; // WHAT: the pinned program, or not started. WHY: risk 8: checked before every start, a restart too
        super::give_the_cell_the_office_key(&self.data, self.server.config.maintenance_key.as_deref()).inspect_err(|e| tracing::error!("ALARM: the Citadel cell was not started: {e}"))?; // WHAT: only the Office Server's maintenance key opens the cell's admin routes; a link on the way stops the start. WHY: a key set on 1.0 moved in with its settings; review medium: never written through a link
        let tls = super::tlskey::ready(&self.home, &self.data, &self.sid).map_err(|e| {
            tracing::error!("ALARM: the Citadel cell was not started: its TLS key isn't ready: {e}"); // WHAT: the alarm. WHY: a cell without its key can't serve, and a key others could use is never handed over
            std::io::Error::other(e)
        })?; // WHAT: the cell's key made, imported or opened, owned by the system, locked to the cell's SID, and its certificate. WHY: §6b: the cell's restricted token can't keep a key Windows' TLS can use
        let mut args = self.args.clone(); // WHAT: the fixed arguments. WHY: the key's two are added
        args.extend(crate::cellkey::cell_args(&tls.name, &tls.certificate).map_err(std::io::Error::other)?); // WHAT: --tls-key and --tls-cert. WHY: the cell opens its key by name and serves that certificate
        let line = super::command_line(&self.program, &args); // WHAT: the whole command line. WHY: made at each start, so a key set back after tampering is the one used
        let pipe = match self.next.take() {
            Some(pipe) => pipe, // WHAT: the instance made while the last one was still held. WHY: the name was never free
            None => make_pipe(&self.pipe, &self.sid, true)?, // WHAT: the first instance, made only if nobody made it first. WHY: a name somebody else holds is an alarm (AddrInUse), not a restart
        };
        let (server, name, sid) = (self.server.clone(), self.pipe.clone(), self.sid.clone()); // WHAT: the server, the pipe's name and the cell's SID, for the thread. WHY: its answers, and the next instance
        self.serving = Some(std::thread::Builder::new().name("citadel-pipe".into()).spawn(move || serve(pipe, &server, &name, &sid))?); // WHAT: the pipe answered on its own thread. WHY: the supervisor waits on the process meanwhile
        match restricted_process(&self.program, &line, &self.sid, &self.environment) {
            Ok(process) => {
                self.process = Some(process); // WHAT: kept. WHY: waited for
                Ok(())
            }
            Err(e) => {
                self.end_pipe(); // WHAT: the waiting pipe released, the next made first. WHY: its thread would wait for a cell that never comes
                Err(e)
            }
        }
    }

    fn wait(&mut self) {
        if let Some(process) = self.process.take() {
            // SAFETY: `process` is the process handle CreateProcessAsUserW gave us, owned here and closed once.
            unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: waiting on and closing our own process handle
                WaitForSingleObject(process.handle(), INFINITE); // WHAT: until the cell stops. WHY: then it is restarted
                let mut code = 0u32; // WHAT: filled by Windows. WHY: how it ended
                if GetExitCodeProcess(process.handle(), &mut code) != 0 {
                    tracing::warn!("the Citadel cell ended with exit code {code} (0x{code:08X})"); // WHAT: said. WHY: 4 is a refused start, others a crash; the log says which
                }
                CloseHandle(process.handle()); // WHAT: let go. WHY: no handle kept for a stopped process
            }
        }
        self.end_pipe(); // WHAT: the next instance made, then the old one's thread ended. WHY: the next start uses it
    }
}

impl Cell {
    /// Ends the old instance's thread (cancelling its wait if the cell never
    /// connected), keeps the next instance it made once the cell connected
    /// (or makes it now, while the old one is still held), and lets the old
    /// instance go.
    fn end_pipe(&mut self) {
        let Some(thread) = self.serving.take() else {
            if self.next.is_none() {
                self.next = self.make_next(); // WHAT: the next instance. WHY: the name is ours throughout
            }
            return;
        };
        {
            use std::os::windows::io::AsRawHandle;
            for _ in 0..50 {
                if thread.is_finished() {
                    break; // WHAT: it has handed the pipe back. WHY: nothing to cancel
                }
                // SAFETY: the handle is the live thread's own, borrowed from its JoinHandle for this call only.
                unsafe { CancelSynchronousIo(thread.as_raw_handle() as HANDLE) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: cancelling our own thread's wait for the cell
                std::thread::sleep(std::time::Duration::from_millis(20)); // WHAT: a moment. WHY: for the cancel to land
            }
            match thread.join() {
                Ok((old, made)) => {
                    self.next = made.or_else(|| self.make_next()); // WHAT: the next instance, made while the cell ran or now. WHY: the name is ours throughout
                    drop(old); // WHAT: the old instance let go. WHY: only once the next one exists
                }
                Err(_) => self.next = self.make_next(), // WHAT: the thread failed; a next instance made. WHY: as above
            }
        }
    }

    /// The next instance of the cell's pipe, or none, said in the log.
    fn make_next(&self) -> Option<std::fs::File> {
        make_pipe(&self.pipe, &self.sid, false).inspect_err(|e| tracing::error!("the Citadel cell's next pipe could not be made: {e}")).ok() // WHAT: made, or said. WHY: the next start makes it fresh, and refuses a name someone else took
    }
}

/// Makes a pipe instance, local clients only, with the system and the
/// cell's SID allowed and nobody else; `first` makes it only if nobody holds
/// the name. At most two instances: the one in use and the next.
fn make_pipe(name: &str, sid: &str, first: bool) -> std::io::Result<std::fs::File> {
    let sddl = wide(OsStr::new(&super::pipe_sddl(sid))); // WHAT: the permissions. WHY: §6a
    let path = wide(OsStr::new(name)); // WHAT: the pipe's name. WHY: §6a
    let out = super::LONGEST_LINE as u32 * 2; // WHAT: room for two lines each way. WHY: requests and answers are short
    let open = if first { PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE } else { PIPE_ACCESS_DUPLEX }; // WHAT: both ways; the first only if nobody made it. WHY: a pipe someone else made would be theirs
    // SAFETY: every pointer passed is to a live local (zero-ended UTF-16 strings, the attributes struct); the descriptor
    // Windows allocates is freed once, after the pipe has taken its own copy; the handle returned is owned by the File.
    unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the pipe made with its permissions
        let mut descriptor: *mut core::ffi::c_void = std::ptr::null_mut(); // WHAT: filled by Windows. WHY: the security descriptor
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) == 0 {
            return Err(std::io::Error::last_os_error()); // WHAT: refused. WHY: never a pipe without its permissions
        }
        let attributes = SECURITY_ATTRIBUTES { nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0), lpSecurityDescriptor: descriptor, bInheritHandle: 0 }; // WHAT: the descriptor, not inherited. WHY: the cell gets no handle of ours
        let handle = CreateNamedPipeW(
            path.as_ptr(),
            open,
            PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, // WHAT: bytes, blocking, this computer only. WHY: lines of JSON, never from another machine
            2, // WHAT: two instances at most. WHY: the one in use and the next, so the name is never free
            out,
            out,
            0, // WHAT: the default wait. WHY: unused
            &attributes,
        );
        let made = std::io::Error::last_os_error(); // WHAT: why, if it failed. WHY: read before LocalFree can change it
        LocalFree(descriptor); // WHAT: the descriptor let go. WHY: the pipe keeps its own copy
        if handle == INVALID_HANDLE_VALUE {
            let taken = first && matches!(made.raw_os_error(), Some(5 | 231)); // WHAT: access denied or all instances busy on a first instance: someone else holds the name. WHY: told apart from any other failure
            return Err(if taken { std::io::Error::new(std::io::ErrorKind::AddrInUse, format!("the pipe {name} is held by another program")) } else { made }); // WHAT: refused. WHY: a taken name is an alarm (security review L4)
        }
        Ok(std::fs::File::from_raw_handle(handle)) // WHAT: the pipe as a file. WHY: read and written with the standard calls, closed when dropped
    }
}

/// Waits for the cell to connect, then answers it until it closes; hands the
/// pipe back, still open.
fn serve(pipe: std::fs::File, server: &crate::api::Server, name: &str, sid: &str) -> (std::fs::File, Option<std::fs::File>) {
    use std::os::windows::io::AsRawHandle;
    // SAFETY: the handle is the pipe this File owns, alive for the whole call; no overlapped structure is used.
    let connected = unsafe { ConnectNamedPipe(pipe.as_raw_handle() as HANDLE, std::ptr::null_mut()) != 0 || GetLastError() == ERROR_PIPE_CONNECTED }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: waiting for the cell on our own pipe
    let mut next = None; // WHAT: the second instance, once the cell is connected. WHY: below
    if connected {
        next = make_pipe(name, sid, false).inspect_err(|e| tracing::error!("the Citadel cell's next pipe could not be made: {e}")).ok(); // WHAT: the second of the two instances, made as soon as the cell is connected to the first. WHY: both are the Office Server's while the cell runs, so the cell can never make one of its own (its access to the pipe includes FILE_CREATE_PIPE_INSTANCE, which .NET's read-write open asks for)
        let office = super::Answering::new(server); // WHAT: the answers for this connection. WHY: the pure part
        let mut reader = BufReader::new(&pipe); // WHAT: lines in. WHY: one JSON object each
        let mut writer = &pipe; // WHAT: lines out. WHY: one answer each
        if let Err(e) = super::serve_lines(&mut reader, &mut writer, &office) {
            tracing::debug!("the Citadel cell's pipe closed: {e}"); // WHAT: noted. WHY: a stopping cell closes it
        }
    }
    (pipe, next) // WHAT: handed back open, with the next instance. WHY: closed only once the next instance exists
}

/// Starts `program` under a restricted copy of this service's token: every
/// privilege dropped, Administrators deny-only, and a second check against
/// the cell's SID and the everyday groups (`super::restricting_sids`).
fn restricted_process(program: &Path, line: &str, sid: &str, environment: &[std::ffi::OsString]) -> std::io::Result<Raw> {
    let job = job()?; // WHAT: the job the cell runs in. WHY: review low 1: it dies with the Office Server, and is ended before an update swaps its folder
    let mut block: Vec<u16> = environment.iter().flat_map(|v| v.encode_wide().chain(std::iter::once(0))).collect(); // WHAT: each variable, ended by a zero. WHY: a Unicode environment block
    block.push(0); // WHAT: the block's end. WHY: as CreateProcess reads it
    let texts: Vec<Vec<u16>> = super::restricting_sids(sid).iter().map(String::as_str).chain(std::iter::once("S-1-5-32-544")).map(|t| wide(OsStr::new(t))).collect(); // WHAT: the restricting SIDs, then Administrators last. WHY: made into SIDs below
    let application = wide(program.as_os_str()); // WHAT: the program, by its full path. WHY: never looked for on the PATH
    let mut command = wide(OsStr::new(line)); // WHAT: the command line, writable. WHY: CreateProcess may write to it
    let folder = program.parent().map(|p| wide(p.as_os_str())); // WHAT: the program's folder. WHY: started there
    // SAFETY: every pointer passed is to a live local for the length of the call; every SID Windows allocates is freed
    // once with LocalFree after the token holds its own copies; the process token, the restricted token and the thread
    // handle are each closed once; the process handle is returned to be owned, waited for and closed by `wait`.
    unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the cell started under a restricted token
        let mut own: HANDLE = std::ptr::null_mut(); // WHAT: this process's token. WHY: the restricted one is made from it
        if OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT | TOKEN_ADJUST_SESSIONID, &mut own) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut sids: Vec<PSID> = Vec::new(); // WHAT: the SIDs, as Windows holds them. WHY: freed after
        let mut failed = None; // WHAT: why a SID couldn't be read. WHY: never a token missing one of its restrictions
        for text in &texts {
            let mut one: PSID = std::ptr::null_mut();
            if ConvertStringSidToSidW(text.as_ptr(), &mut one) == 0 {
                failed = Some(std::io::Error::last_os_error());
                break;
            }
            sids.push(one);
        }
        let mut token: HANDLE = std::ptr::null_mut(); // WHAT: the restricted token. WHY: the cell runs under it
        let made = match failed {
            Some(e) => Err(e),
            None => {
                let administrators = [SID_AND_ATTRIBUTES { Sid: sids[sids.len() - 1], Attributes: 0 }]; // WHAT: Administrators, made deny-only. WHY: the cell is never an administrator
                let restricting: Vec<SID_AND_ATTRIBUTES> = sids[..sids.len() - 1].iter().map(|s| SID_AND_ATTRIBUTES { Sid: *s, Attributes: 0 }).collect(); // WHAT: the second check's SIDs. WHY: only what grants these is reachable
                if CreateRestrictedToken(own, DISABLE_MAX_PRIVILEGE, 1, administrators.as_ptr(), 0, std::ptr::null(), u32::try_from(restricting.len()).unwrap_or(0), restricting.as_ptr(), &mut token) == 0 {
                    Err(std::io::Error::last_os_error()) // WHAT: refused. WHY: §6: only under a restricted token
                } else {
                    Ok(())
                }
            }
        };
        for s in sids {
            LocalFree(s); // WHAT: let go. WHY: the token holds its own copies
        }
        CloseHandle(own);
        made?;
        if let Err(e) = give_default_dacl(token, sid) {
            CloseHandle(token); // WHAT: let go. WHY: not started
            return Err(e); // WHAT: said. WHY: a cell that can't open its own process never starts
        }
        let (said, write) = match output_pipe() {
            Ok(pair) => pair, // WHAT: the pipe the cell's output comes back on. WHY: it says why it stopped, on its error output
            Err(e) => {
                CloseHandle(token); // WHAT: let go. WHY: not started
                return Err(e);
            }
        };
        let mut size = 0usize; // WHAT: the attribute list's size. WHY: asked of Windows first
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size); // WHAT: the size for one attribute. WHY: the list is made in our own memory
        let mut list = vec![0u8; size.max(64)]; // WHAT: room for it. WHY: Windows fills it
        let attributes = list.as_mut_ptr().cast::<core::ffi::c_void>(); // WHAT: the list. WHY: passed on
        let inherited = [write]; // WHAT: the only handle the cell inherits: its output's write end. WHY: it reaches us otherwise only through the pipe
        let listed = InitializeProcThreadAttributeList(attributes, 1, 0, &mut size) != 0
            && UpdateProcThreadAttribute(attributes, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, inherited.as_ptr().cast(), std::mem::size_of_val(&inherited), std::ptr::null_mut(), std::ptr::null()) != 0; // WHAT: that one handle named. WHY: no other handle of ours is ever inherited
        if !listed {
            let why = std::io::Error::last_os_error(); // WHAT: why. WHY: read first
            CloseHandle(write); // WHAT: let go. WHY: not started
            CloseHandle(token);
            return Err(why);
        }
        let mut desktop = match desktop(sid) {
            Ok(name) => name, // WHAT: the cell's own desktop. WHY: below
            Err(e) => {
                DeleteProcThreadAttributeList(attributes); // WHAT: let go. WHY: not started
                CloseHandle(write);
                CloseHandle(token);
                return Err(e);
            }
        };
        let mut startup: STARTUPINFOEXW = std::mem::zeroed(); // WHAT: no window; its output to our pipe. WHY: the defaults otherwise
        startup.StartupInfo.lpDesktop = desktop.as_mut_ptr(); // WHAT: the cell's own window station and desktop. WHY: its restricted token can't open the service's; Windows would end it at once (0xC0000142)
        startup.StartupInfo.cb = u32::try_from(std::mem::size_of::<STARTUPINFOEXW>()).unwrap_or(0); // WHAT: the extended form's size. WHY: the attribute list follows
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES; // WHAT: its standard handles given. WHY: below
        startup.StartupInfo.hStdOutput = write; // WHAT: its output. WHY: to our log
        startup.StartupInfo.hStdError = write; // WHAT: its errors. WHY: likewise
        startup.lpAttributeList = attributes; // WHAT: the one inherited handle. WHY: as above
        let mut info: PROCESS_INFORMATION = std::mem::zeroed(); // WHAT: filled by Windows. WHY: the process's handle
        let started = CreateProcessAsUserW(
            token,
            application.as_ptr(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // WHAT: inherit, but only the handle in the list. WHY: the cell's output comes back; nothing else of ours reaches it
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT, // WHAT: no window, held until it is in the job, the list read. WHY: §6a; nothing it starts escapes the job
            block.as_ptr().cast(), // WHAT: its environment, with .NET's unpacking folder. WHY: review low 3
            folder.as_ref().map_or(std::ptr::null(), |f| f.as_ptr()),
            (&raw const startup).cast::<STARTUPINFOW>(),
            &mut info,
        );
        let start_error = std::io::Error::last_os_error(); // WHAT: why, if it failed. WHY: read before CloseHandle can change it
        DeleteProcThreadAttributeList(attributes); // WHAT: the list let go. WHY: used only by the call
        CloseHandle(write); // WHAT: our copy of the write end closed. WHY: the read end ends when the cell does
        CloseHandle(token);
        if started == 0 {
            return Err(start_error);
        }
        let _ = std::thread::Builder::new().name("citadel-output".into()).spawn(move || log_output(said)); // WHAT: the cell's output read and logged. WHY: "The cell didn't start: ..." reaches the Office Server's log
        if AssignProcessToJobObject(job.handle(), info.hProcess) == 0 {
            let why = std::io::Error::last_os_error(); // WHAT: why. WHY: read before anything else changes it
            TerminateProcess(info.hProcess, 1); // WHAT: never run outside the job. WHY: it wouldn't die with the server
            CloseHandle(info.hThread);
            CloseHandle(info.hProcess);
            return Err(why);
        }
        ResumeThread(info.hThread); // WHAT: let it run, now inside the job. WHY: started suspended for that
        CloseHandle(info.hThread); // WHAT: the thread's handle let go. WHY: only the process is waited for
        Ok(Raw(info.hProcess as usize))
    }
}

/// Gives the restricted token a default DACL that its own SID passes
/// (`super::token_default_sddl`): what the cell makes without saying who may
/// use it (its own process and threads above all) is the system's and the
/// cell's. With the system's default alone, the token's second check refuses
/// the cell its own process, and Windows ends it as it starts (0xC0000142).
fn give_default_dacl(token: HANDLE, sid: &str) -> std::io::Result<()> {
    use windows_sys::Win32::Security::{GetSecurityDescriptorDacl, SetTokenInformation, TokenDefaultDacl, ACL, TOKEN_DEFAULT_DACL};
    let sddl = wide(OsStr::new(&super::token_default_sddl(sid))); // WHAT: the default DACL, as SDDL. WHY: read by Windows
    // SAFETY: every pointer passed is to a live local for the length of the call; the descriptor Windows allocates is
    // freed once, after SetTokenInformation has copied the DACL it points into.
    unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the cell's token's default DACL
        let mut descriptor: *mut core::ffi::c_void = std::ptr::null_mut(); // WHAT: filled by Windows. WHY: the parsed SDDL
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) == 0 {
            return Err(std::io::Error::last_os_error()); // WHAT: refused. WHY: said
        }
        let (mut present, mut defaulted, mut dacl) = (0, 0, std::ptr::null_mut::<ACL>()); // WHAT: filled by Windows. WHY: the DACL inside
        let done = if GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) == 0 || present == 0 {
            Err(std::io::Error::other("the default DACL's SDDL has no DACL")) // WHAT: refused. WHY: never an empty one
        } else {
            let value = TOKEN_DEFAULT_DACL { DefaultDacl: dacl }; // WHAT: the DACL, as the call takes it. WHY: below
            if SetTokenInformation(token, TokenDefaultDacl, (&raw const value).cast(), u32::try_from(std::mem::size_of::<TOKEN_DEFAULT_DACL>()).unwrap_or(0)) == 0 {
                Err(std::io::Error::last_os_error()) // WHAT: Windows' reason. WHY: said
            } else {
                Ok(()) // WHAT: set. WHY: the cell may open its own process
            }
        };
        LocalFree(descriptor); // WHAT: let go. WHY: the token holds its own copy
        done
    }
}

/// The cell's own window station and desktop, made once for this process,
/// the system's and the cell's alone (`super::desktop_sddl`); its name as
/// `lpDesktop` takes it. The handles are kept for the life of the process,
/// which keeps them there.
fn desktop(sid: &str) -> std::io::Result<Vec<u16>> {
    static MADE: std::sync::OnceLock<Result<String, i32>> = std::sync::OnceLock::new(); // WHAT: made once. WHY: every start of the cell uses the same one
    let name = MADE.get_or_init(|| make_desktop(sid)).clone().map_err(std::io::Error::from_raw_os_error)?; // WHAT: its name, or why not. WHY: said by the caller
    Ok(wide(OsStr::new(&name))) // WHAT: as Windows takes it. WHY: STARTUPINFO.lpDesktop
}

/// The work of [`desktop`].
fn make_desktop(sid: &str) -> Result<String, i32> {
    use windows_sys::Win32::Foundation::GENERIC_ALL;
    use windows_sys::Win32::System::StationsAndDesktops::{CloseWindowStation, CreateDesktopW, CreateWindowStationW, GetProcessWindowStation, SetProcessWindowStation};
    let (station, full) = super::desktop_name(std::process::id()); // WHAT: the names. WHY: one per Office Server process
    let sddl = wide(OsStr::new(&super::desktop_sddl(sid))); // WHAT: who may use them. WHY: the system and the cell only
    let (station_wide, desk_wide) = (wide(OsStr::new(&station)), wide(OsStr::new("Default"))); // WHAT: the names as Windows takes them. WHY: below
    let failed = || std::io::Error::last_os_error().raw_os_error().unwrap_or(0); // WHAT: Windows' reason. WHY: said
    // SAFETY: every pointer passed is to a live local for the length of the call; the descriptor Windows allocates is freed
    // once, after both objects hold their own copies; the window station is the process's own again before returning; the
    // two new handles are kept open on purpose for the life of the process.
    unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the cell's private window station and desktop
        let mut descriptor: *mut core::ffi::c_void = std::ptr::null_mut(); // WHAT: filled by Windows. WHY: the permissions
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) == 0 {
            return Err(failed()); // WHAT: refused. WHY: never without its permissions
        }
        let attributes = SECURITY_ATTRIBUTES { nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0), lpSecurityDescriptor: descriptor, bInheritHandle: 0 }; // WHAT: the permissions, not inherited. WHY: the cell gets no handle of ours
        let made_station = CreateWindowStationW(station_wide.as_ptr(), 0, GENERIC_ALL, &attributes); // WHAT: the window station. WHY: the desktop lives in it
        if made_station.is_null() {
            let why = failed(); // WHAT: why. WHY: read first
            LocalFree(descriptor);
            return Err(why);
        }
        let ours = GetProcessWindowStation(); // WHAT: this process's own. WHY: put back after
        if SetProcessWindowStation(made_station) == 0 {
            let why = failed(); // WHAT: why. WHY: read first
            CloseWindowStation(made_station);
            LocalFree(descriptor);
            return Err(why);
        }
        let made_desktop = CreateDesktopW(desk_wide.as_ptr(), std::ptr::null(), std::ptr::null(), 0, GENERIC_ALL, &attributes); // WHAT: the desktop, made in the station just set. WHY: CreateDesktopW makes it in the process's current station
        let why = failed(); // WHAT: why, if it failed. WHY: read before anything else changes it
        SetProcessWindowStation(ours); // WHAT: this process's own station back. WHY: it was changed only for the call above
        LocalFree(descriptor); // WHAT: let go. WHY: both hold their own copies
        if made_desktop.is_null() {
            CloseWindowStation(made_station); // WHAT: let go. WHY: no desktop, no use
            return Err(why);
        }
        Ok(full) // WHAT: station\desktop. WHY: both handles stay open with the process
    }
}

/// A pipe for the cell's output: the read end, ours, never inherited; the
/// write end, inheritable, for the cell.
fn output_pipe() -> std::io::Result<(std::fs::File, HANDLE)> {
    let attributes = SECURITY_ATTRIBUTES { nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0), lpSecurityDescriptor: std::ptr::null_mut(), bInheritHandle: 1 }; // WHAT: inheritable handles. WHY: the write end goes to the cell
    let (mut read, mut write): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut()); // WHAT: filled by Windows. WHY: the two ends
    // SAFETY: the pointers are to live locals; the read end is owned by the File returned, the write end by the caller.
    unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: an anonymous pipe for the cell's output
        if CreatePipe(&mut read, &mut write, &attributes, 0) == 0 {
            return Err(std::io::Error::last_os_error()); // WHAT: refused. WHY: said
        }
        SetHandleInformation(read, HANDLE_FLAG_INHERIT, 0); // WHAT: the read end never inherited. WHY: only the write end goes to the cell
        Ok((std::fs::File::from_raw_handle(read), write))
    }
}

/// Writes what the cell prints into the Office Server's log, a line at a
/// time, the first 50 lines of each start at most; the rest is read and let
/// go, so the cell never waits on a full pipe.
fn log_output(said: std::fs::File) {
    use std::io::BufRead as _; // WHAT: reading lines. WHY: one log line each
    let mut shown = 0; // WHAT: lines written so far. WHY: a cell printing without end can't flood the log
    for line in BufReader::new(said).split(b'\n') {
        let Ok(line) = line else { break }; // WHAT: the next line, or the end. WHY: read until the cell's output closes, so the cell never blocks writing
        let line = String::from_utf8_lossy(&line[..line.len().min(4096)]).into_owned(); // WHAT: as text, bounded. WHY: whatever the cell's console writes
        if shown < 50 && !line.trim().is_empty() {
            shown += 1; // WHAT: counted. WHY: as above
            tracing::warn!("the Citadel cell said: {}", line.chars().filter(|c| !c.is_control()).take(500).collect::<String>()); // WHAT: its words, plain and bounded. WHY: why it stopped, in the log
        }
    }
}

/// Citadel Server 1.0's Windows service, through the service manager.
struct OldCitadel;

impl OldCitadel {
    /// The 1.0 service, opened with `access`, under whichever of its names it
    /// has.
    fn open(access: windows_service::service::ServiceAccess) -> Option<windows_service::service::Service> {
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).ok()?; // WHAT: the service manager. WHY: as the system
        crate::cellmove::OLD_SERVICES.iter().find_map(|name| manager.open_service(name, access).ok()) // WHAT: the first name that opens. WHY: its display name or its SID's name
    }
}

impl OldCitadel {
    /// Runs `sc.exe config <name> start= <how>` on whichever name the 1.0
    /// service has; whether it worked.
    fn start_type(how: &str) -> bool {
        crate::cellmove::OLD_SERVICES.iter().any(|name| crate::winsvc::system_tool("sc.exe", &["config".into(), (*name).into(), "start=".into(), how.into()])) // WHAT: its start type changed. WHY: disabled for the move, automatic again after a failure
    }
}

impl crate::cellmove::OldService for OldCitadel {
    fn found(&self) -> Option<String> {
        let service = Self::open(windows_service::service::ServiceAccess::QUERY_CONFIG)?; // WHAT: the 1.0 service, if installed. WHY: its program's path is recorded
        let config = service.query_config().ok()?; // WHAT: its configuration. WHY: the path
        Some(config.executable_path.display().to_string()) // WHAT: its program, as the service manager has it. WHY: recorded as "1.0 service found at <path>"
    }

    fn stop_and_disable(&self) -> Result<(), String> {
        use windows_service::service::{ServiceAccess, ServiceState};
        let Some(service) = Self::open(ServiceAccess::QUERY_STATUS | ServiceAccess::STOP) else {
            return Ok(()); // WHAT: no longer installed. WHY: nothing running holds the port or the files
        };
        if !Self::start_type("disabled") {
            return Err("it couldn't be disabled".into()); // WHAT: refused. WHY: it could start again in the middle of the move
        }
        let _ = service.stop(); // WHAT: asked to stop. WHY: it holds port 47110 and its files
        let deadline = Instant::now() + std::time::Duration::from_secs(60); // WHAT: a minute. WHY: a service shutting down cleanly
        loop {
            match service.query_status().map(|s| s.current_state) {
                Ok(ServiceState::Stopped) => return Ok(()), // WHAT: stopped and disabled, still installed. WHY: critical 1: never uninstalled before the move
                _ if Instant::now() >= deadline => return Err("it didn't stop within a minute".into()), // WHAT: refused. WHY: the move never runs beside it
                _ => std::thread::sleep(std::time::Duration::from_millis(300)), // WHAT: waited. WHY: asked again shortly
            }
        }
    }

    fn restore(&self) -> Result<(), String> {
        use windows_service::service::ServiceAccess;
        let service = Self::open(ServiceAccess::QUERY_STATUS | ServiceAccess::START).ok_or("it is no longer installed")?; // WHAT: the 1.0 service. WHY: started again
        if !Self::start_type("auto") {
            return Err("it couldn't be set to start with Windows again".into()); // WHAT: said. WHY: someone must act
        }
        service.start::<&str>(&[]).map_err(|e| format!("it couldn't be started ({e})")) // WHAT: running again. WHY: critical 1: the office keeps estimating after a failed move
    }

    fn remove(&self) -> Result<String, String> {
        use windows_service::service::ServiceAccess;
        let mut said = Vec::new(); // WHAT: what was done. WHY: said in the log and on /office
        if let Some(service) = Self::open(ServiceAccess::QUERY_STATUS | ServiceAccess::DELETE) {
            service.delete().map_err(|e| format!("it couldn't be uninstalled ({e})"))?; // WHAT: uninstalled. WHY: step 5, only after the data moved
            said.push("Citadel Server 1.0's service was uninstalled"); // WHAT: noted. WHY: said
        }
        for (tool, args) in super::old_leftover_steps() {
            if crate::winsvc::system_tool(tool, &args) {
                said.push(if tool == "schtasks.exe" { "its 03:30 update task was removed" } else { "its firewall rule was removed" }); // WHAT: noted. WHY: said
            }
        }
        Ok(if said.is_empty() { "nothing of Citadel Server 1.0 was left to remove".to_string() } else { said.join(", ") }) // WHAT: what was done, in words. WHY: high 5
    }
}

/// The job object every cell runs in, made once: killed when the Office
/// Server's last handle to it closes, which is when the server exits.
fn job() -> std::io::Result<Raw> {
    static JOB: std::sync::OnceLock<Result<Raw, i32>> = std::sync::OnceLock::new(); // WHAT: made once. WHY: one job for every start of the cell
    JOB.get_or_init(|| {
        // SAFETY: no attributes and no name are passed; the limit structure is a zeroed local of the right size, read by
        // Windows during the call; the job's handle is kept for the life of the process, which is the point.
        unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the cell's job object
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null()); // WHAT: a new job. WHY: below
            if job.is_null() {
                return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)); // WHAT: refused. WHY: the cell never runs outside a job
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed(); // WHAT: the limits. WHY: one flag set
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE; // WHAT: everything in it dies when the job's last handle closes. WHY: the cell never outlives the server
            if SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const limits).cast(), u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0)) == 0 {
                let why = std::io::Error::last_os_error().raw_os_error().unwrap_or(0); // WHAT: why. WHY: said
                CloseHandle(job);
                return Err(why); // WHAT: refused. WHY: likewise
            }
            Ok(Raw(job as usize))
        }
    })
    .clone()
    .map_err(std::io::Error::from_raw_os_error)
}

/// Ends the cell before an update swaps its folder, waiting up to `within`
/// for it to be gone. Whether it is gone (true when it never ran).
pub fn end_the_cell(within: std::time::Duration) -> bool {
    let Ok(job) = job() else { return true }; // WHAT: the cell's job. WHY: no job, no cell
    let until = Instant::now() + within; // WHAT: the limit. WHY: review low 1: ten seconds
    // SAFETY: the job handle lives for the process; the accounting structure is a zeroed local of the right size.
    unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: ending the cell's job and counting what is left
        TerminateJobObject(job.handle(), 1); // WHAT: every process in the job ended. WHY: its folder is about to be replaced
        loop {
            let mut counted: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed(); // WHAT: the job's counts. WHY: how many still run
            let asked = QueryInformationJobObject(job.handle(), JobObjectBasicAccountingInformation, (&raw mut counted).cast(), u32::try_from(std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>()).unwrap_or(0), std::ptr::null_mut()); // WHAT: asked. WHY: below
            if asked != 0 && counted.ActiveProcesses == 0 {
                return true; // WHAT: gone. WHY: the folder may be swapped
            }
            if Instant::now() >= until {
                return false; // WHAT: still there. WHY: the caller raises the alarm
            }
            std::thread::sleep(std::time::Duration::from_millis(100)); // WHAT: a moment. WHY: asked again
        }
    }
}

/// Citadel Server 1.0's nightly update task, through `schtasks`.
struct OldTask;

impl crate::cellmove::OldTask for OldTask {
    fn there(&self) -> bool {
        crate::winsvc::system_tool("schtasks.exe", &["/Query".into(), "/TN".into(), super::OLD_UPDATE_TASK.into()]) // WHAT: asked by name. WHY: schtasks exits 1 when there is no such task
    }
    fn remove(&self) -> bool {
        let (tool, args) = &super::old_leftover_steps()[0]; // WHAT: the task's removal. WHY: the same step as after the move
        crate::winsvc::system_tool(tool, args) // WHAT: removed, or not. WHY: the alarm follows
    }
}
