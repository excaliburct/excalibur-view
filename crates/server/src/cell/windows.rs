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

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, LocalFree, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, PSID, SECURITY_ATTRIBUTES, SID_AND_ATTRIBUTES, TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};
use windows_sys::Win32::System::Threading::{CreateProcessAsUserW, GetCurrentProcess, OpenProcessToken, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, INFINITE, PROCESS_INFORMATION, STARTUPINFOW};

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
    if !super::should_start(&crate::office::licensed(&server), program.is_file()) {
        return; // WHAT: no licence for it, or no program. WHY: §6: the cell runs only then
    }
    let spawned = std::thread::Builder::new().name("citadel-cell".into()).spawn(move || {
        let sid = super::cell_sid(&install); // WHAT: the cell's own SID. WHY: the pipe, its data and its program grant it
        if let Some(office) = data.parent() {
            let _ = std::fs::create_dir_all(office); // WHAT: `Excalibur Office`. WHY: made before it is locked
            crate::lockdown::lock_parent(office); // WHAT: the system's and administrators' alone. WHY: an account that made it first could swap the folder below
        }
        let _ = std::fs::create_dir_all(&data); // WHAT: the cell's data folder. WHY: made once
        icacls(&super::data_steps(&data, &sid)); // WHAT: the system and the cell only. WHY: §6
        if let Some(own) = program.parent() {
            icacls(&super::program_steps(own, &sid)); // WHAT: the cell may read and run its program. WHY: the service's folder is locked to the system and administrators
        }
        let mut launcher = Cell { server: server.clone(), program, line: String::new(), args, pipe: format!(r"\\.\pipe\{}", super::pipe_name(&install)), sid, process: None, serving: None, next: None };
        launcher.line = super::command_line(&launcher.program, &launcher.args); // WHAT: the command line. WHY: written once
        super::supervise(&mut launcher, &server.cell, &std::thread::sleep, &Instant::now); // WHAT: run and restarted. WHY: §6a
    });
    if let Err(e) = spawned {
        tracing::error!("the Citadel cell's supervisor could not start: {e}"); // WHAT: said. WHY: the part then reports what its port shows
    }
}

/// Runs each icacls step, saying in the log when one fails.
fn icacls(steps: &[Vec<std::ffi::OsString>]) {
    use std::os::windows::process::CommandExt; // WHAT: Windows-only process options. WHY: no console window
    let system = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from); // WHAT: Windows' folder. WHY: icacls by its full path
    let program = system.join("System32").join("icacls.exe"); // WHAT: the program. WHY: never one found on the PATH
    for args in steps {
        let done = std::process::Command::new(&program).args(args).creation_flags(CREATE_NO_WINDOW).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status(); // WHAT: run and waited for. WHY: done before the cell starts
        if !matches!(done, Ok(status) if status.success()) {
            tracing::warn!("could not set who may use {:?}", args.first()); // WHAT: said. WHY: whoever looks after the server can see it
        }
    }
}

/// The cell, as the supervisor starts and waits for it.
struct Cell {
    server: Shared, // WHAT: the Office Server. WHY: the pipe's answers
    program: PathBuf, // WHAT: `citadel\Citadel Server.exe`. WHY: started
    line: String, // WHAT: its command line. WHY: started with it
    args: Vec<String>, // WHAT: its arguments. WHY: the line is made from them
    pipe: String, // WHAT: `\\.\pipe\excalibur-office-<install id>`. WHY: §6a
    sid: String, // WHAT: the cell's SID. WHY: the pipe and the token
    process: Option<Raw>, // WHAT: the running cell. WHY: waited for
    serving: Option<std::thread::JoinHandle<std::fs::File>>, // WHAT: the thread answering its pipe, which hands the pipe back when done. WHY: the old instance is held until the next exists
    next: Option<std::fs::File>, // WHAT: the next pipe instance, made before the old one closed. WHY: security review L4: the name is never free during the restart wait
}

impl super::Launcher for Cell {
    fn start(&mut self) -> std::io::Result<()> {
        let pipe = match self.next.take() {
            Some(pipe) => pipe, // WHAT: the instance made while the last one was still held. WHY: the name was never free
            None => make_pipe(&self.pipe, &self.sid, true)?, // WHAT: the first instance, made only if nobody made it first. WHY: a name somebody else holds is an alarm (AddrInUse), not a restart
        };
        let server = self.server.clone(); // WHAT: the server, for the thread. WHY: its answers
        self.serving = Some(std::thread::Builder::new().name("citadel-pipe".into()).spawn(move || serve(pipe, &server))?); // WHAT: the pipe answered on its own thread. WHY: the supervisor waits on the process meanwhile
        match restricted_process(&self.program, &self.line, &self.sid) {
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
                CloseHandle(process.handle()); // WHAT: let go. WHY: no handle kept for a stopped process
            }
        }
        self.end_pipe(); // WHAT: the next instance made, then the old one's thread ended. WHY: the next start uses it
    }
}

impl Cell {
    /// Makes the next pipe instance while the old one is still held, then
    /// ends the old one's thread (cancelling its wait if the cell never
    /// connected) and lets the old instance go.
    fn end_pipe(&mut self) {
        match make_pipe(&self.pipe, &self.sid, false) {
            Ok(next) => self.next = Some(next), // WHAT: held for the next start. WHY: the name is ours throughout
            Err(e) => tracing::error!("the Citadel cell's next pipe could not be made: {e}"), // WHAT: said. WHY: the next start makes it fresh, and refuses a name someone else took
        }
        if let Some(thread) = self.serving.take() {
            use std::os::windows::io::AsRawHandle;
            for _ in 0..50 {
                if thread.is_finished() {
                    break; // WHAT: it has handed the pipe back. WHY: nothing to cancel
                }
                // SAFETY: the handle is the live thread's own, borrowed from its JoinHandle for this call only.
                unsafe { CancelSynchronousIo(thread.as_raw_handle() as HANDLE) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: cancelling our own thread's wait for the cell
                std::thread::sleep(std::time::Duration::from_millis(20)); // WHAT: a moment. WHY: for the cancel to land
            }
            let _ = thread.join(); // WHAT: waited for; the old instance it hands back is dropped here. WHY: only once the next one exists
        }
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
fn serve(pipe: std::fs::File, server: &crate::api::Server) -> std::fs::File {
    use std::os::windows::io::AsRawHandle;
    // SAFETY: the handle is the pipe this File owns, alive for the whole call; no overlapped structure is used.
    let connected = unsafe { ConnectNamedPipe(pipe.as_raw_handle() as HANDLE, std::ptr::null_mut()) != 0 || GetLastError() == ERROR_PIPE_CONNECTED }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: waiting for the cell on our own pipe
    if connected {
        let office = super::Answering::new(server); // WHAT: the answers for this connection. WHY: the pure part
        let mut reader = BufReader::new(&pipe); // WHAT: lines in. WHY: one JSON object each
        let mut writer = &pipe; // WHAT: lines out. WHY: one answer each
        if let Err(e) = super::serve_lines(&mut reader, &mut writer, &office) {
            tracing::debug!("the Citadel cell's pipe closed: {e}"); // WHAT: noted. WHY: a stopping cell closes it
        }
    }
    pipe // WHAT: handed back open. WHY: closed only once the next instance exists
}

/// Starts `program` under a restricted copy of this service's token: every
/// privilege dropped, Administrators deny-only, and a second check against
/// the cell's SID and the everyday groups (`super::restricting_sids`).
fn restricted_process(program: &Path, line: &str, sid: &str) -> std::io::Result<Raw> {
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
        let mut startup: STARTUPINFOW = std::mem::zeroed(); // WHAT: no window, nothing special. WHY: the defaults
        startup.cb = u32::try_from(std::mem::size_of::<STARTUPINFOW>()).unwrap_or(0);
        let mut info: PROCESS_INFORMATION = std::mem::zeroed(); // WHAT: filled by Windows. WHY: the process's handle
        let started = CreateProcessAsUserW(
            token,
            application.as_ptr(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0, // WHAT: no handles inherited. WHY: the cell reaches us only through the pipe
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT, // WHAT: no window. WHY: §6a: no Manager window
            std::ptr::null(),
            folder.as_ref().map_or(std::ptr::null(), |f| f.as_ptr()),
            &startup,
            &mut info,
        );
        let start_error = std::io::Error::last_os_error(); // WHAT: why, if it failed. WHY: read before CloseHandle can change it
        CloseHandle(token);
        if started == 0 {
            return Err(start_error);
        }
        CloseHandle(info.hThread); // WHAT: the thread's handle let go. WHY: only the process is waited for
        Ok(Raw(info.hProcess as usize))
    }
}
