//! Listening sockets nobody else can share.
//!
//! On Windows a socket bound with `SO_EXCLUSIVEADDRUSE` can't be taken over
//! or shared by another program binding the same port, even one running as
//! an administrator or with `SO_REUSEADDR`; without it, a program bound to a
//! specific address could be handed connections meant for ours. Used for the
//! Office Server's own port (8714) and for the Citadel cell's (47110).
//! Elsewhere the socket is bound as tokio binds it.

use std::net::SocketAddr; // WHAT: an address and port. WHY: what is bound

/// How many connections may wait to be accepted.
const BACKLOG: i32 = 1024; // WHAT: 1024. WHY: as tokio's own default

/// How long a port still held may be waited for.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(60); // WHAT: a minute. WHY: after a restart, Windows keeps an exclusive port from being bound again while the old process's connections are still closing

/// The first wait between tries. It doubles, up to `MOST_WAIT`.
const FIRST_WAIT: std::time::Duration = std::time::Duration::from_millis(250); // WHAT: a quarter second. WHY: a port that frees at once is taken at once

/// The longest wait between tries.
const MOST_WAIT: std::time::Duration = std::time::Duration::from_secs(5); // WHAT: five seconds. WHY: the port is taken soon after it frees

/// What an IPv6 listener on the unspecified address `[::]` also takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum V6 {
    /// IPv6 only: IPv4 is bound by a socket of its own. The Citadel cell's
    /// 47110, which binds `0.0.0.0` and `[::]` apart.
    Only,
    /// IPv6 and IPv4 both (dual-stack). The Office Server's own port, which
    /// binds one address: `[::]` there means every address of both kinds.
    Dual,
}

/// Binds `address` exclusively (Windows) and listens. While the port is
/// still held (`AddrInUse`), tries again with a growing wait for up to a
/// minute, logging each wait; any other failure is returned at once.
/// `v6` says what an IPv6 address also takes; it is ignored for IPv4.
///
/// # Errors
///
/// When the address can't be bound: another program still holds it after a
/// minute, or it isn't this machine's.
pub async fn exclusive(address: SocketAddr, v6: V6) -> std::io::Result<tokio::net::TcpListener> {
    patiently(address, v6, PATIENCE, FIRST_WAIT).await // WHAT: bound, waiting out a port still held. WHY: a restarted server must not fail to start because its last connections are closing
}

/// `exclusive`, with its patience and first wait given (shortened by tests).
async fn patiently(address: SocketAddr, v6: V6, patience: std::time::Duration, first_wait: std::time::Duration) -> std::io::Result<tokio::net::TcpListener> {
    let until = tokio::time::Instant::now() + patience; // WHAT: when to give up. WHY: a port held for good is reported, not waited on forever
    let mut wait = first_wait; // WHAT: the next wait. WHY: grows below
    loop {
        match bind(address, v6) {
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && tokio::time::Instant::now() + wait <= until => {
                tracing::warn!("{address} is still held ({e}); trying again in {} ms", wait.as_millis()); // WHAT: each wait, said. WHY: the log shows why the start took longer
                tokio::time::sleep(wait).await; // WHAT: a wait. WHY: the old connections close
                wait = (wait * 2).min(MOST_WAIT); // WHAT: the next wait longer. WHY: not tried hundreds of times
            }
            done => return done, // WHAT: bound, or a failure waiting won't change. WHY: said by the caller
        }
    }
}

/// One try at binding `address` exclusively (Windows) and listening.
fn bind(address: SocketAddr, v6: V6) -> std::io::Result<tokio::net::TcpListener> {
    let socket = configured(address, v6)?; // WHAT: the socket, its options set. WHY: bound below
    socket.bind(&address.into())?; // WHAT: bound. WHY: below
    socket.listen(BACKLOG)?; // WHAT: listening. WHY: connections wait to be accepted
    socket.set_nonblocking(true)?; // WHAT: non-blocking. WHY: tokio needs it
    tokio::net::TcpListener::from_std(socket.into()) // WHAT: handed to tokio. WHY: served asynchronously
}

/// A TCP socket for `address` with its options set, not yet bound.
fn configured(address: SocketAddr, v6: V6) -> std::io::Result<socket2::Socket> {
    let socket = socket2::Socket::new(socket2::Domain::for_address(address), socket2::Type::STREAM, Some(socket2::Protocol::TCP))?; // WHAT: a TCP socket of the address's family. WHY: bound below
    #[cfg(windows)]
    exclusive_use(&socket)?; // WHAT: SO_EXCLUSIVEADDRUSE, before binding. WHY: nobody else can share or take the port
    #[cfg(not(windows))]
    socket.set_reuse_address(true)?; // WHAT: SO_REUSEADDR, as tokio sets it. WHY: a restarted server binds again at once; on Unix this never lets another program share a listening port
    if address.is_ipv6() {
        socket.set_only_v6(v6 == V6::Only)?; // WHAT: IPv6 only, or both kinds, as asked. WHY: 47110 holds each family on its own socket; the Office Server's one address [::] means both
    }
    Ok(socket) // WHAT: ready to bind. WHY: below
}

/// Sets `SO_EXCLUSIVEADDRUSE` on a socket.
#[cfg(windows)]
fn exclusive_use(socket: &socket2::Socket) -> std::io::Result<()> {
    use std::os::windows::io::AsRawSocket as _; // WHAT: the raw socket. WHY: the option is set on it
    use windows_sys::Win32::Networking::WinSock::{setsockopt, SOCKET, SOL_SOCKET, SO_EXCLUSIVEADDRUSE};
    let on: i32 = 1; // WHAT: on. WHY: below
    // SAFETY: the socket is open and owned by `socket` for the call; the value is a live local of the size given.
    let done = unsafe { setsockopt(socket.as_raw_socket() as SOCKET, SOL_SOCKET, SO_EXCLUSIVEADDRUSE, (&raw const on).cast(), 4) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: one socket option on our own socket
    if done == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) } // WHAT: set, or why not. WHY: never a shared port by accident
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn a_listening_port_is_bound_and_answers() {
        let listener = super::exclusive("127.0.0.1:0".parse().unwrap(), super::V6::Only).await.unwrap(); // WHAT: bound. WHY: below
        let address = listener.local_addr().unwrap(); // WHAT: its port. WHY: connected to
        let (accepted, connected) = tokio::join!(listener.accept(), tokio::net::TcpStream::connect(address)); // WHAT: a connection. WHY: it listens
        assert!(accepted.is_ok() && connected.is_ok(), "accepted"); // WHAT: both ends. WHY: a working listener
        assert!(super::bind(address, super::V6::Only).is_err(), "the same port can't be bound twice"); // WHAT: refused. WHY: nobody shares it
    }

    #[tokio::test]
    async fn a_port_still_held_is_waited_for_and_taken_once_it_frees() {
        use std::time::Duration; // WHAT: durations. WHY: below
        let holder = super::bind("127.0.0.1:0".parse().unwrap(), super::V6::Only).unwrap(); // WHAT: the port held, as by the server before a restart. WHY: below
        let address = holder.local_addr().unwrap(); // WHAT: its address. WHY: bound again
        let started = std::time::Instant::now(); // WHAT: the start. WHY: it must have waited
        let refused = super::patiently(address, super::V6::Only, Duration::from_millis(400), Duration::from_millis(50)).await; // WHAT: held throughout. WHY: gives up after its patience
        assert_eq!(refused.unwrap_err().kind(), std::io::ErrorKind::AddrInUse); // WHAT: the failure reported. WHY: a port held for good isn't hidden
        assert!(started.elapsed() >= Duration::from_millis(300), "it waited before giving up"); // WHAT: it tried again first. WHY: the retry
        let freed = tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(500)).await; drop(holder) }); // WHAT: the port let go after a moment. WHY: as the old process's connections close
        let started = std::time::Instant::now(); // WHAT: the start. WHY: below
        let taken = super::patiently(address, super::V6::Only, Duration::from_secs(20), Duration::from_millis(50)).await; // WHAT: tried until it frees. WHY: the restart goes through
        freed.await.unwrap(); // WHAT: the holder gone. WHY: tidy
        assert!(taken.is_ok(), "bound once it freed: {taken:?}"); // WHAT: bound. WHY: the retry
        assert!(started.elapsed() >= Duration::from_millis(400), "it waited for the port"); // WHAT: it had to wait. WHY: the port was held at first
    }

    #[tokio::test]
    async fn any_other_failure_is_reported_at_once() {
        let started = std::time::Instant::now(); // WHAT: the start. WHY: below
        let refused = super::patiently("192.0.2.1:0".parse().unwrap(), super::V6::Only, std::time::Duration::from_secs(20), std::time::Duration::from_millis(50)).await; // WHAT: an address that isn't this machine's (TEST-NET-1). WHY: waiting can't change it
        assert!(refused.is_err_and(|e| e.kind() != std::io::ErrorKind::AddrInUse)); // WHAT: refused for its own reason. WHY: below
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "not waited on"); // WHAT: at once. WHY: only a held port is waited for
    }

    #[test]
    fn the_office_servers_ipv6_address_takes_ipv4_too_and_the_cells_does_not() {
        let any: std::net::SocketAddr = "[::]:0".parse().unwrap(); // WHAT: every IPv6 address. WHY: what [::] means for each; checked before binding, so nothing is opened to the network
        let Ok(dual) = super::configured(any, super::V6::Dual) else { return }; // WHAT: the Office Server's kind. WHY: a machine without IPv6 has nothing to show
        let only = super::configured(any, super::V6::Only).unwrap(); // WHAT: the cell's kind. WHY: below
        assert!(!dual.only_v6().unwrap(), "dual-stack"); // WHAT: both kinds. WHY: V6::Dual, the Office Server's [::]
        assert!(only.only_v6().unwrap(), "IPv6 only"); // WHAT: IPv6 only. WHY: V6::Only, 47110's [::] beside its own 0.0.0.0
    }
}
