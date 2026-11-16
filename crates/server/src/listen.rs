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

/// Binds `address` exclusively (Windows) and listens.
///
/// # Errors
///
/// When the address can't be bound: another program holds it, or it isn't this machine's.
pub fn exclusive(address: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    let socket = socket2::Socket::new(socket2::Domain::for_address(address), socket2::Type::STREAM, Some(socket2::Protocol::TCP))?; // WHAT: a TCP socket of the address's family. WHY: bound below
    #[cfg(windows)]
    exclusive_use(&socket)?; // WHAT: SO_EXCLUSIVEADDRUSE, before binding. WHY: nobody else can share or take the port
    #[cfg(not(windows))]
    socket.set_reuse_address(true)?; // WHAT: SO_REUSEADDR, as tokio sets it. WHY: a restarted server binds again at once; on Unix this never lets another program share a listening port
    if address.is_ipv6() {
        socket.set_only_v6(true)?; // WHAT: IPv6 only. WHY: IPv4 is its own socket, so each family is held on its own
    }
    socket.bind(&address.into())?; // WHAT: bound. WHY: below
    socket.listen(BACKLOG)?; // WHAT: listening. WHY: connections wait to be accepted
    socket.set_nonblocking(true)?; // WHAT: non-blocking. WHY: tokio needs it
    tokio::net::TcpListener::from_std(socket.into()) // WHAT: handed to tokio. WHY: served asynchronously
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
        let listener = super::exclusive("127.0.0.1:0".parse().unwrap()).unwrap(); // WHAT: bound. WHY: below
        let address = listener.local_addr().unwrap(); // WHAT: its port. WHY: connected to
        let (accepted, connected) = tokio::join!(listener.accept(), tokio::net::TcpStream::connect(address)); // WHAT: a connection. WHY: it listens
        assert!(accepted.is_ok() && connected.is_ok(), "accepted"); // WHAT: both ends. WHY: a working listener
        assert!(super::exclusive(address).is_err(), "the same port can't be bound twice"); // WHAT: refused. WHY: nobody shares it
    }
}
