//! The Windows half of `crate::cellrelay`: the listener on 47110, run on a
//! small runtime of its own, and the way each connection reaches the cell:
//! its relay pipe, opened as the system, and checked to be served by the
//! cell process this server started before a byte is sent.

use std::os::windows::io::AsRawHandle as _;
use std::sync::{Arc, Mutex, OnceLock};

use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;

use crate::cellrelay::{CellStream, Connect, Current, Target};

/// The relay's state, one for the process.
pub fn current() -> Arc<Current> {
    static CURRENT: OnceLock<Arc<Current>> = OnceLock::new(); // WHAT: made once. WHY: the listener and the supervisor share it
    CURRENT.get_or_init(|| Arc::new(Current::default())).clone()
}

/// The relay's own runtime: two threads, apart from the API's.
fn runtime() -> Option<&'static tokio::runtime::Runtime> {
    static RUNTIME: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new(); // WHAT: made once. WHY: lives with the process
    RUNTIME
        .get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("citadel-relay").enable_all().build().inspect_err(|e| tracing::error!("ALARM: the Citadel cell's relay couldn't start its runtime: {e}")).ok()) // WHAT: the runtime, or the alarm. WHY: the relay never borrows the API's threads
        .as_ref()
}

/// Starts listening on 47110 (IPv4, and IPv6 where there is one), once for
/// the process; later calls do nothing once it listens.
///
/// # Errors
///
/// When 47110 can't be taken on IPv4 (another program holds it); tried again at the cell's next start.
pub fn listen() -> std::io::Result<()> {
    static LISTENING: Mutex<bool> = Mutex::new(false); // WHAT: whether it listens. WHY: bound once
    let mut listening = LISTENING.lock().unwrap_or_else(std::sync::PoisonError::into_inner); // WHAT: held while binding. WHY: two starts never bind twice
    if *listening {
        return Ok(()); // WHAT: already listening. WHY: nothing to do
    }
    let runtime = runtime().ok_or_else(|| std::io::Error::other("the relay's runtime didn't start"))?; // WHAT: the runtime. WHY: below
    let caps = crate::cellrelay::Caps::default(); // WHAT: one set of caps. WHY: 500 in all and 32 per machine hold across IPv4 and IPv6 together (security review, 2026-11-16)
    let v4 = runtime.block_on(async { crate::listen::exclusive((std::net::Ipv4Addr::UNSPECIFIED, crate::cell::PORT).into()) })?; // WHAT: 47110 on IPv4, held exclusively. WHY: the office network reaches the cell there; no other program can share the port
    runtime.spawn(crate::cellrelay::serve(v4, current(), connector(), caps.clone())); // WHAT: served. WHY: TLS answered and relayed
    match runtime.block_on(async { crate::listen::exclusive((std::net::Ipv6Addr::UNSPECIFIED, crate::cell::PORT).into()) }) {
        Ok(v6) => {
            runtime.spawn(crate::cellrelay::serve(v6, current(), connector(), caps)); // WHAT: served on IPv6 too, with the same caps. WHY: Citadel listened on both
        }
        Err(e) => tracing::info!("the Citadel cell's port isn't served on IPv6 ({e}); IPv4 is"), // WHAT: said. WHY: a machine without IPv6 still serves the office
    }
    *listening = true; // WHAT: noted. WHY: bound once
    tracing::info!("the Office Server answers TLS for the Citadel cell on TCP {}", crate::cell::PORT); // WHAT: said. WHY: the log shows who holds the port
    Ok(())
}

/// How a connection reaches the cell: its relay pipe, opened as the system
/// (waiting while every instance is busy), then checked: the process serving
/// the pipe must be the cell this server started. A pipe served by any other
/// process gets nothing and is an alarm.
fn connector() -> Connect {
    Arc::new(|target: Target| {
        Box::pin(async move {
            let name = format!(r"\\.\pipe\{}", target.pipe); // WHAT: the pipe's full name. WHY: as Windows takes it
            let pipe = loop {
                match tokio::net::windows::named_pipe::ClientOptions::new().open(&name) {
                    Ok(pipe) => break pipe, // WHAT: connected. WHY: checked next
                    Err(e) if e.raw_os_error() == Some(231) => tokio::time::sleep(std::time::Duration::from_millis(20)).await, // WHAT: ERROR_PIPE_BUSY: every instance in use, waited for. WHY: the cell makes the next; the caller's own limit ends the wait
                    Err(e) => return Err(e), // WHAT: no pipe. WHY: the connection is closed
                }
            };
            let mut server = 0u32; // WHAT: filled by Windows. WHY: the serving process's id
            // SAFETY: the handle is the pipe just opened, alive for the call; the out pointer is a live local.
            let asked = unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut server) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: who serves the relay pipe
            if asked == 0 || server != target.pid {
                tracing::error!("ALARM: the Citadel cell's relay pipe is served by process {server}, not the cell ({}); nothing was sent to it", target.pid); // WHAT: the alarm. WHY: something else took the pipe's name
                return Err(std::io::Error::other("the relay pipe isn't the cell's")); // WHAT: refused. WHY: not a byte to anyone but the cell
            }
            Ok(Box::new(pipe) as CellStream) // WHAT: the checked pipe. WHY: relayed over
        })
    })
}
