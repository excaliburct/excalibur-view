//! TLS on 47110 for the Citadel cell, ended here and relayed to the cell
//! (docs/OFFICE-ENROLLMENT.md §6b).
//!
//! Windows' TLS (`SChannel`) gives no credentials at all to a process whose
//! token has restricting SIDs, which the cell's has (the Windows end-to-end
//! test, try 21: `AcquireCredentialsHandle` returned `0x8009030E` even with
//! no certificate). So the Office Server, as the system, answers 47110 itself
//! with rustls, signing through the cell's non-exportable key in Windows' key
//! store (only the system may use it); the certificate is the one the
//! computers pinned. Each connection, once its handshake is done, is relayed
//! as plain bytes to the cell over a named pipe the cell serves, which only
//! the system and the cell may open, after the Office Server has checked
//! that the pipe's server process is the cell it started. The first bytes on
//! every relayed connection are a PROXY protocol v2 header with the caller's
//! real address, which the cell takes from this channel alone.
//!
//! Decided here, and tested on any machine: the limits, the PROXY header, the
//! per-machine counts, the TLS settings, and the relay itself (over any
//! stream). `cell::relaywin` (Windows) supplies the key's signer and the
//! pipe.

use std::collections::HashMap; // WHAT: counts by machine. WHY: the per-machine cap
use std::net::{IpAddr, SocketAddr}; // WHAT: addresses. WHY: the caller's, written into the PROXY header
use std::sync::{Arc, Mutex}; // WHAT: shared state. WHY: every connection reads the current cell and counts itself
use std::time::Duration; // WHAT: time limits. WHY: below

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _}; // WHAT: async byte streams. WHY: the relay

/// The most connections open at once.
pub const MOST_CONNECTIONS: usize = 500; // WHAT: 500. WHY: Citadel's own cap (ServerHost.MaxConnections), so nothing changes for an office
/// The most from one machine (an IPv6 /64 counts as one).
pub const MOST_FROM_ONE: usize = 32; // WHAT: 32. WHY: Citadel's own (MaxConnectionsPerAddress): one computer can't take every slot
/// How long a TLS handshake may take.
pub const HANDSHAKE: Duration = Duration::from_secs(10); // WHAT: ten seconds. WHY: Citadel's own HandshakeTimeout; a stalled handshake gives its slot back
/// How long a connection may carry nothing either way.
pub const IDLE: Duration = Duration::from_secs(90); // WHAT: ninety seconds. WHY: longer than the cell's 60-second keep-alive and 25-second long-poll, so the cell closes an idle connection first; a held-open socket still gives its slot back
/// How long the cell's pipe may stay busy before a connection gives up.
pub const PIPE_WAIT: Duration = Duration::from_secs(5); // WHAT: five seconds. WHY: the cell makes a new instance after each connection; a cell that never does fails the connection, not the server
/// The ALPN protocol offered.
pub const ALPN: &[u8] = b"http/1.1"; // WHAT: HTTP/1.1. WHY: Citadel's Kestrel served HTTP/1.1 only (ServerHost: Protocols = Http1)
/// The most bytes read from one side before they are written to the other.
const CHUNK: usize = 16 * 1024; // WHAT: 16 KiB. WHY: a write waits for the other side, so neither side is read faster than the other takes it (backpressure)

/// The PROXY protocol v2 signature.
pub const PROXY_SIGNATURE: [u8; 12] = [0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A]; // WHAT: "\r\n\r\n\0\r\nQUIT\n". WHY: the protocol's fixed start

/// The PROXY protocol v2 header for one TCP connection: version 2, command
/// PROXY, the caller's address and port, and ours. An IPv4 caller seen on an
/// IPv6 socket is written as IPv4.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn proxy_v2(remote: SocketAddr, local: SocketAddr) -> Vec<u8> {
    let canon = |a: SocketAddr| SocketAddr::new(a.ip().to_canonical(), a.port()); // WHAT: IPv4-mapped written as IPv4. WHY: the cell keys its limits on the real address
    let (remote, local) = (canon(remote), canon(local)); // WHAT: both. WHY: below
    let mut out = PROXY_SIGNATURE.to_vec(); // WHAT: the signature. WHY: first
    out.push(0x21); // WHAT: version 2, command PROXY. WHY: a relayed connection
    match (remote.ip(), local.ip()) {
        (IpAddr::V4(r), IpAddr::V4(l)) => {
            out.push(0x11); // WHAT: TCP over IPv4. WHY: both addresses are IPv4
            out.extend_from_slice(&12u16.to_be_bytes()); // WHAT: 12 bytes follow. WHY: two addresses and two ports
            out.extend_from_slice(&r.octets()); // WHAT: the caller's address. WHY: the cell's limits and logs
            out.extend_from_slice(&l.octets()); // WHAT: ours. WHY: the protocol has both
        }
        (r, l) => {
            let six = |ip: IpAddr| match ip {
                IpAddr::V4(v4) => v4.to_ipv6_mapped(),
                IpAddr::V6(v6) => v6,
            }; // WHAT: an address as IPv6. WHY: one family for both
            out.push(0x21); // WHAT: TCP over IPv6. WHY: at least one address is IPv6
            out.extend_from_slice(&36u16.to_be_bytes()); // WHAT: 36 bytes follow. WHY: two IPv6 addresses and two ports
            out.extend_from_slice(&six(r).octets()); // WHAT: the caller's address. WHY: as above
            out.extend_from_slice(&six(l).octets()); // WHAT: ours. WHY: as above
        }
    }
    out.extend_from_slice(&remote.port().to_be_bytes()); // WHAT: the caller's port. WHY: as above
    out.extend_from_slice(&local.port().to_be_bytes()); // WHAT: ours. WHY: as above
    out
}

/// Connections counted per machine, given back when each closes.
#[derive(Default, Clone)]
pub struct Counts(Arc<Mutex<HashMap<IpAddr, usize>>>); // WHAT: open connections by machine. WHY: the per-machine cap

/// One connection counted against its machine.
pub struct Counted {
    counts: Counts, // WHAT: where it is counted. WHY: given back when dropped
    machine: IpAddr, // WHAT: the machine. WHY: likewise
}

impl Counts {
    /// One more connection from `ip`'s machine, or none when it has its share.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it gives the slot back at once
    pub fn take(&self, ip: IpAddr) -> Option<Counted> {
        let machine = crate::tls::machine(ip); // WHAT: the machine (an IPv6 /64, an IPv4 address). WHY: the same rule as 8714
        let mut map = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner); // WHAT: the counts. WHY: a poisoned lock still counts
        let n = map.entry(machine).or_insert(0); // WHAT: its count. WHY: below
        if *n >= MOST_FROM_ONE {
            return None; // WHAT: its share taken. WHY: one machine can't take every slot
        }
        *n += 1; // WHAT: counted. WHY: given back on drop
        Some(Counted { counts: self.clone(), machine })
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        let mut map = self.counts.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner); // WHAT: the counts. WHY: below
        if let Some(n) = map.get_mut(&self.machine) {
            *n -= 1; // WHAT: given back. WHY: the connection closed
            if *n == 0 {
                map.remove(&self.machine); // WHAT: forgotten at zero. WHY: the map never grows with every address ever seen
            }
        }
    }
}

/// What signs with the cell's key: SHA-256 then ECDSA P-256, the signature
/// in DER. On Windows it is the CNG key the Office Server holds.
pub trait Sign: Send + Sync + std::fmt::Debug {
    /// The DER ECDSA signature of SHA-256(`message`), or none.
    fn sign(&self, message: &[u8]) -> Option<Vec<u8>>;
}

/// The cell's key as rustls signs with it.
#[derive(Debug)]
struct Key(Arc<dyn Sign>); // WHAT: the signer. WHY: shared by every handshake

/// One handshake's signer.
#[derive(Debug)]
struct Signing(Arc<dyn Sign>); // WHAT: the signer. WHY: below

impl rustls::sign::SigningKey for Key {
    fn choose_scheme(&self, offered: &[rustls::SignatureScheme]) -> Option<Box<dyn rustls::sign::Signer>> {
        offered.contains(&rustls::SignatureScheme::ECDSA_NISTP256_SHA256).then(|| Box::new(Signing(self.0.clone())) as Box<dyn rustls::sign::Signer>) // WHAT: ECDSA P-256 with SHA-256, when the client takes it. WHY: the only thing this key does
    }

    fn algorithm(&self) -> rustls::SignatureAlgorithm {
        rustls::SignatureAlgorithm::ECDSA // WHAT: ECDSA. WHY: a P-256 key
    }
}

impl rustls::sign::Signer for Signing {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, rustls::Error> {
        self.0.sign(message).ok_or_else(|| rustls::Error::General("the Citadel cell's key would not sign".into())) // WHAT: signed through the key, or the handshake fails. WHY: never a handshake without the pinned key
    }

    fn scheme(&self) -> rustls::SignatureScheme {
        rustls::SignatureScheme::ECDSA_NISTP256_SHA256 // WHAT: the scheme. WHY: as chosen
    }
}

/// The one certificate, always.
#[derive(Debug)]
struct One(Arc<rustls::sign::CertifiedKey>); // WHAT: the certificate and its key. WHY: every caller gets the pinned one

impl rustls::server::ResolvesServerCert for One {
    fn resolve(&self, _hello: rustls::server::ClientHello<'_>) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(self.0.clone()) // WHAT: the pinned certificate. WHY: one office, one certificate
    }
}

/// The TLS settings for 47110: TLS 1.2 and 1.3 only (Citadel's own), the
/// pinned certificate, signed through `key`, HTTP/1.1 by ALPN.
///
/// # Errors
///
/// When rustls refuses the settings.
pub fn server_config(certificate: Vec<u8>, key: Arc<dyn Sign>) -> Result<Arc<rustls::ServerConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider()); // WHAT: rustls' ring suites. WHY: as 8714; the handshake's own ECDHE and ciphers
    let certified = rustls::sign::CertifiedKey::new(vec![rustls::pki_types::CertificateDer::from(certificate)], Arc::new(Key(key))); // WHAT: the pinned certificate and the key held in Windows. WHY: the computers' pins match
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12]) // WHAT: TLS 1.3 and 1.2 only. WHY: Citadel's own SslProtocols
        .map_err(|e| format!("the cell's TLS settings: {e}"))?
        .with_no_client_auth() // WHAT: no client certificates. WHY: Citadel's computers sign their requests instead
        .with_cert_resolver(Arc::new(One(Arc::new(certified)))); // WHAT: the one certificate. WHY: as above
    config.alpn_protocols = vec![ALPN.to_vec()]; // WHAT: HTTP/1.1. WHY: what Citadel served
    Ok(Arc::new(config))
}

/// Where a relayed connection goes, and the TLS it is answered with: set at
/// each start of the cell, cleared when it stops.
#[derive(Default)]
pub struct Current {
    config: Mutex<Option<Arc<rustls::ServerConfig>>>, // WHAT: the TLS settings with the key. WHY: none until the key is ready
    target: Mutex<Option<Target>>, // WHAT: the running cell's pipe and process. WHY: none while it isn't running
}

/// The running cell, as the relay reaches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub pipe: String, // WHAT: its relay pipe's name, new at each start. WHY: unguessable, never the control pipe
    pub pid: u32, // WHAT: its process id. WHY: the pipe's server must be this process
}

impl Current {
    /// The TLS settings to answer with from now on.
    pub fn set_config(&self, config: Arc<rustls::ServerConfig>) {
        *self.config.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(config); // WHAT: kept. WHY: each new connection reads it
    }

    /// The cell now running, or none.
    pub fn set_target(&self, target: Option<Target>) {
        *self.target.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = target; // WHAT: kept. WHY: each new connection reads it
    }

    fn now(&self) -> (Option<Arc<rustls::ServerConfig>>, Option<Target>) {
        (self.config.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone(), self.target.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()) // WHAT: both, as they are. WHY: one connection's view
    }
}

/// A stream to the cell.
pub type CellStream = Box<dyn Stream>;
/// What a relayed connection is carried over.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// How a connection reaches the cell: given the target, a stream whose far
/// end has been checked to be that cell.
pub type Connect = Arc<dyn Fn(Target) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<CellStream>> + Send>> + Send + Sync>;

/// Answers TLS on `listener` and relays each connection to the cell, for as
/// long as the listener lives.
pub async fn serve(listener: tokio::net::TcpListener, current: Arc<Current>, connect: Connect) {
    let slots = Arc::new(tokio::sync::Semaphore::new(MOST_CONNECTIONS)); // WHAT: the open-connection slots. WHY: the cap
    let counts = Counts::default(); // WHAT: connections per machine. WHY: the per-machine cap
    let local = listener.local_addr().ok(); // WHAT: our own address. WHY: the PROXY header's destination
    loop {
        let Ok((tcp, peer)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(50)).await; // WHAT: a pause after a failed accept. WHY: a broken listener never spins
            continue; // WHAT: carried on. WHY: one bad connection never stops the server
        };
        let Some(counted) = counts.take(peer.ip()) else {
            continue; // WHAT: closed at once (dropped). WHY: past its machine's share
        };
        let Ok(slot) = slots.clone().try_acquire_owned() else {
            continue; // WHAT: closed at once. WHY: every slot taken; a flood is turned away rather than queued
        };
        let (current, connect) = (current.clone(), connect.clone()); // WHAT: for the task. WHY: below
        let ours = tcp.local_addr().ok().or(local).unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], crate::cell::PORT))); // WHAT: the address the caller reached. WHY: the PROXY header's destination
        tokio::spawn(async move {
            let (_slot, _counted) = (slot, counted); // WHAT: held for the connection's life. WHY: given back when it closes
            if let Err(why) = one(tcp, peer, ours, &current, &connect).await {
                tracing::debug!("a connection to the Citadel cell from {peer} ended: {why}"); // WHAT: noted. WHY: a stranger's dropped connection is no alarm
            }
        });
    }
}

/// One connection: the handshake within its limit, then the cell reached and
/// checked, the PROXY header sent, and the bytes relayed both ways.
async fn one(tcp: tokio::net::TcpStream, peer: SocketAddr, ours: SocketAddr, current: &Current, connect: &Connect) -> Result<(), String> {
    let (config, target) = current.now(); // WHAT: the TLS settings and the cell, now. WHY: below
    let (Some(config), Some(target)) = (config, target) else {
        return Err("the cell isn't running".into()); // WHAT: closed. WHY: nothing to relay to; the computers retry
    };
    let _ = tcp.set_nodelay(true); // WHAT: small writes sent at once. WHY: request and answer lines aren't held back
    let tls = tokio::time::timeout(HANDSHAKE, tokio_rustls::TlsAcceptor::from(config).accept(tcp)).await.map_err(|_| "the handshake took too long".to_string())?.map_err(|e| format!("the handshake failed: {e}"))?; // WHAT: TLS, within ten seconds. WHY: a stalled handshake gives its slot back
    let mut cell = tokio::time::timeout(PIPE_WAIT, connect(target)).await.map_err(|_| "the cell's pipe stayed busy".to_string())?.map_err(|e| format!("the cell couldn't be reached: {e}"))?; // WHAT: the cell, checked by the connector. WHY: only the cell this server started gets a byte
    cell.write_all(&proxy_v2(peer, ours)).await.map_err(|e| e.to_string())?; // WHAT: the caller's real address first. WHY: the cell's limits and logs keep working
    relay(tls, cell, IDLE).await.map_err(|e| e.to_string()) // WHAT: the bytes, both ways. WHY: the cell answers as before
}

/// Copies bytes both ways between `a` and `b` until both have ended, or
/// nothing moves either way for `idle`; each write waits for the other side
/// (backpressure). When one side ends, the other is told (its write half is
/// shut) and the rest is still carried; both are closed together at the end.
///
/// # Errors
///
/// When either side fails.
pub async fn relay<A, B>(a: A, b: B, idle: Duration) -> std::io::Result<()>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let (mut a_read, mut a_write) = tokio::io::split(a); // WHAT: the caller's halves. WHY: read and written at once
    let (mut b_read, mut b_write) = tokio::io::split(b); // WHAT: the cell's halves. WHY: likewise
    let (mut from_a, mut from_b) = (vec![0u8; CHUNK], vec![0u8; CHUNK]); // WHAT: one buffer each way. WHY: bounded memory per connection
    let (mut a_open, mut b_open) = (true, true); // WHAT: whether each side still sends. WHY: carried until both have ended
    while a_open || b_open {
        tokio::select! {
            read = a_read.read(&mut from_a), if a_open => match read {
                Ok(0) => {
                    a_open = false; // WHAT: the caller has finished sending. WHY: below
                    let _ = b_write.shutdown().await; // WHAT: the cell told. WHY: it sees the end of the request
                }
                Ok(n) => tokio::time::timeout(idle, b_write.write_all(&from_a[..n])).await.map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??, // WHAT: passed to the cell, waiting for it to take it. WHY: backpressure; a cell that takes nothing for the idle time ends the connection
                Err(e) => return Err(e), // WHAT: the caller's side failed. WHY: both closed
            },
            read = b_read.read(&mut from_b), if b_open => match read {
                Ok(0) => {
                    b_open = false; // WHAT: the cell has finished sending. WHY: below
                    let _ = a_write.shutdown().await; // WHAT: the caller told (TLS close). WHY: it sees the end of the answer
                }
                Ok(n) => tokio::time::timeout(idle, a_write.write_all(&from_b[..n])).await.map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??, // WHAT: passed to the caller, waiting for it to take it. WHY: backpressure
                Err(e) => return Err(e), // WHAT: the cell's side failed. WHY: both closed
            },
            () = tokio::time::sleep(idle) => return Err(std::io::ErrorKind::TimedOut.into()), // WHAT: nothing either way for the idle time. WHY: a held-open connection gives its slot back
        }
    }
    let _ = a_write.shutdown().await; // WHAT: the caller's side closed. WHY: both together
    let _ = b_write.shutdown().await; // WHAT: the cell's side closed. WHY: likewise
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_proxy_header_carries_the_caller_s_real_address() {
        let v4 = proxy_v2("192.168.1.20:51000".parse().unwrap(), "192.168.1.5:47110".parse().unwrap()); // WHAT: an IPv4 caller. WHY: the office network
        assert_eq!(&v4[..12], &PROXY_SIGNATURE, "the signature"); // WHAT: first. WHY: the protocol
        assert_eq!(&v4[12..16], &[0x21, 0x11, 0, 12], "version 2 PROXY, TCP over IPv4, 12 bytes"); // WHAT: the header's fixed part. WHY: as the cell reads it
        assert_eq!(&v4[16..20], &[192, 168, 1, 20], "the caller"); // WHAT: its address. WHY: the cell's limits key on it
        assert_eq!(&v4[24..26], &51000u16.to_be_bytes(), "its port"); // WHAT: its port. WHY: as above
        assert_eq!(v4.len(), 28, "nothing more"); // WHAT: the whole header. WHY: the request follows at once
        let mapped = proxy_v2("[::ffff:10.0.0.9]:5000".parse().unwrap(), "[::ffff:10.0.0.1]:47110".parse().unwrap()); // WHAT: IPv4 seen on an IPv6 socket. WHY: written as IPv4
        assert_eq!((mapped[13], &mapped[16..20]), (0x11, &[10, 0, 0, 9][..]), "an IPv4 caller is written as IPv4"); // WHAT: as IPv4. WHY: one machine, one key
        let v6 = proxy_v2("[fd00::20]:5000".parse().unwrap(), "[fd00::1]:47110".parse().unwrap()); // WHAT: an IPv6 caller. WHY: below
        assert_eq!((v6[13], u16::from_be_bytes([v6[14], v6[15]]), v6.len()), (0x21, 36, 52), "TCP over IPv6, 36 bytes"); // WHAT: the IPv6 form. WHY: as the cell reads it
    }

    #[test]
    fn one_machine_gets_its_share_of_connections_and_no_more() {
        let counts = Counts::default(); // WHAT: no connections yet. WHY: below
        let ip: IpAddr = "10.0.0.7".parse().unwrap(); // WHAT: one machine. WHY: below
        let held: Vec<Counted> = (0..MOST_FROM_ONE).map(|_| counts.take(ip).expect("within its share")).collect(); // WHAT: its whole share. WHY: below
        assert!(counts.take(ip).is_none(), "one more is refused"); // WHAT: refused. WHY: the per-machine cap
        assert!(counts.take("10.0.0.8".parse().unwrap()).is_some(), "another machine still gets in"); // WHAT: allowed. WHY: shares are per machine
        drop(held); // WHAT: all closed. WHY: below
        assert!(counts.take(ip).is_some(), "given back when they close"); // WHAT: allowed again. WHY: counted only while open
    }

    #[tokio::test]
    async fn the_relay_carries_both_ways_and_closes_both_together() {
        let (caller, near) = tokio::io::duplex(64); // WHAT: the caller's side. WHY: a small buffer shows backpressure works
        let (far, cell) = tokio::io::duplex(64); // WHAT: the cell's side. WHY: likewise
        let pump = tokio::spawn(relay(near, far, Duration::from_secs(5))); // WHAT: the relay. WHY: under test
        let (mut caller, mut cell) = (caller, cell); // WHAT: the two ends a test holds. WHY: below
        let request = vec![7u8; 100_000]; // WHAT: a request far bigger than the buffers. WHY: every byte must pass, in order
        let writer = tokio::spawn(async move {
            caller.write_all(&request).await.unwrap(); // WHAT: sent. WHY: below
            caller.shutdown().await.unwrap(); // WHAT: the caller has finished. WHY: the cell must see the end
            let mut answer = Vec::new(); // WHAT: what comes back. WHY: checked
            caller.read_to_end(&mut answer).await.unwrap(); // WHAT: until the cell closes. WHY: below
            answer
        });
        let mut got = Vec::new(); // WHAT: what reached the cell. WHY: checked
        cell.read_to_end(&mut got).await.unwrap(); // WHAT: until the caller's end. WHY: the end was passed on
        assert_eq!(got.len(), 100_000, "every byte reached the cell"); // WHAT: whole. WHY: nothing lost under backpressure
        cell.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap(); // WHAT: the cell's answer. WHY: after the caller has finished sending
        cell.shutdown().await.unwrap(); // WHAT: the cell has finished. WHY: both sides have now ended
        assert_eq!(writer.await.unwrap(), b"HTTP/1.1 200 OK\r\n\r\n", "the answer reached the caller after its own end"); // WHAT: carried. WHY: half-closed connections still answer
        pump.await.unwrap().unwrap(); // WHAT: the relay finished cleanly. WHY: both sides ended
    }

    #[tokio::test]
    async fn a_connection_that_carries_nothing_is_closed() {
        let (_caller, near) = tokio::io::duplex(64); // WHAT: a caller that sends nothing. WHY: a held-open socket
        let (far, _cell) = tokio::io::duplex(64); // WHAT: a cell that sends nothing. WHY: likewise
        let ended = relay(near, far, Duration::from_millis(100)).await; // WHAT: the relay with a short idle limit. WHY: under test
        assert_eq!(ended.unwrap_err().kind(), std::io::ErrorKind::TimedOut, "closed after the idle time"); // WHAT: ended. WHY: its slot given back
    }

    /// A signer over a ring key, standing in for the CNG one.
    #[derive(Debug)]
    struct Ring(ring::signature::EcdsaKeyPair);

    impl Sign for Ring {
        fn sign(&self, message: &[u8]) -> Option<Vec<u8>> {
            self.0.sign(&ring::rand::SystemRandom::new(), message).ok().map(|s| s.as_ref().to_vec()) // WHAT: ECDSA P-256 SHA-256, DER. WHY: what the CNG signer gives
        }
    }

    #[tokio::test]
    #[allow(clippy::items_after_statements)] // WHAT: the stand-in key type beside its use. WHY: it reads in order with the test
    async fn tls_on_the_port_is_answered_with_the_pinned_certificate_and_relayed_with_the_callers_address() {
        use ring::signature::{EcdsaKeyPair, KeyPair as _, ECDSA_P256_SHA256_ASN1_SIGNING};
        let rng = ring::rand::SystemRandom::new(); // WHAT: randomness. WHY: a key
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap(); // WHAT: a P-256 key. WHY: the cell's stand-in
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng).unwrap(); // WHAT: opened. WHY: below
        let point = key.public_key().as_ref().to_vec(); // WHAT: its point. WHY: the certificate holds it
        struct Held(EcdsaKeyPair, Vec<u8>); // WHAT: the key as rcgen signs with it. WHY: a certificate for it
        impl rcgen::RemoteKeyPair for Held {
            fn public_key(&self) -> &[u8] {
                &self.1 // WHAT: the point. WHY: rcgen writes it
            }
            fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
                self.0.sign(&ring::rand::SystemRandom::new(), msg).map(|s| s.as_ref().to_vec()).map_err(|_| rcgen::Error::RemoteKeyError) // WHAT: DER. WHY: as a certificate holds it
            }
            fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
                &rcgen::PKCS_ECDSA_P256_SHA256 // WHAT: ECDSA P-256 SHA-256. WHY: Citadel's
            }
        }
        let pkcs8_again = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng).unwrap(); // WHAT: the same key again. WHY: one for rcgen, one for the signer
        let pair = rcgen::KeyPair::from_remote(Box::new(Held(pkcs8_again, point))).unwrap(); // WHAT: the remote key. WHY: as on Windows
        let cert = crate::cellkey::certificate_params("TEST", time::OffsetDateTime::now_utc()).unwrap().self_signed(&pair).unwrap().der().to_vec(); // WHAT: Citadel's certificate shape. WHY: as a new office's
        let current = Arc::new(Current::default()); // WHAT: no cell yet. WHY: set below
        current.set_config(server_config(cert.clone(), Arc::new(Ring(key))).unwrap()); // WHAT: the TLS settings. WHY: as at the cell's start
        current.set_target(Some(Target { pipe: "test".into(), pid: 1 })); // WHAT: a running cell. WHY: below
        let (to_cell, from_relay) = tokio::sync::mpsc::channel::<tokio::io::DuplexStream>(1); // WHAT: the cell's ends of connections. WHY: the test plays the cell
        let connect: Connect = Arc::new(move |target: Target| {
            let to_cell = to_cell.clone(); // WHAT: for the future. WHY: below
            Box::pin(async move {
                assert_eq!(target.pid, 1, "the target set"); // WHAT: the cell asked for. WHY: as set
                let (ours, theirs) = tokio::io::duplex(4096); // WHAT: a stand-in pipe. WHY: no Windows here
                to_cell.send(theirs).await.map_err(|_| std::io::Error::other("no cell"))?; // WHAT: handed to the test's cell. WHY: below
                Ok(Box::new(ours) as CellStream)
            })
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); // WHAT: a free port. WHY: stands in for 47110
        let address = listener.local_addr().unwrap(); // WHAT: its address. WHY: the client connects to it
        tokio::spawn(serve(listener, current.clone(), connect)); // WHAT: the relay. WHY: under test
        let mut roots = rustls::RootCertStore::empty(); // WHAT: trust only the pinned certificate. WHY: as a pinning client
        roots.add(rustls::pki_types::CertificateDer::from(cert.clone())).unwrap(); // WHAT: added. WHY: below
        let mut client = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions().unwrap().with_root_certificates(roots).with_no_client_auth(); // WHAT: a TLS client. WHY: below
        client.alpn_protocols = vec![b"h2".to_vec(), ALPN.to_vec()]; // WHAT: what HttpClient offers. WHY: HTTP/1.1 must be chosen
        let tcp = tokio::net::TcpStream::connect(address).await.unwrap(); // WHAT: connected. WHY: below
        let mut tls = tokio_rustls::TlsConnector::from(Arc::new(client)).connect(rustls::pki_types::ServerName::try_from("localhost").unwrap(), tcp).await.expect("the handshake, with the pinned certificate"); // WHAT: TLS. WHY: under test
        assert_eq!(tls.get_ref().1.alpn_protocol(), Some(ALPN), "HTTP/1.1, as Citadel served"); // WHAT: the protocol. WHY: below
        assert_eq!(tls.get_ref().1.peer_certificates().unwrap()[0].as_ref(), cert.as_slice(), "the pinned certificate"); // WHAT: the same certificate. WHY: the computers' pins
        let mut from_relay = from_relay; // WHAT: the cell's side. WHY: below
        tls.write_all(b"GET /v1/hello HTTP/1.1\r\n\r\n").await.unwrap(); // WHAT: a request. WHY: relayed
        let mut cell = from_relay.recv().await.unwrap(); // WHAT: the cell's end of this connection. WHY: below
        let mut header = [0u8; 28]; // WHAT: the PROXY header. WHY: first
        cell.read_exact(&mut header).await.unwrap(); // WHAT: read. WHY: checked
        assert_eq!(header.to_vec(), proxy_v2(tls.get_ref().0.local_addr().unwrap(), address), "the caller's real address first"); // WHAT: as the caller is. WHY: the cell's limits
        let mut line = [0u8; 26]; // WHAT: the request. WHY: after the header
        cell.read_exact(&mut line).await.unwrap(); // WHAT: read. WHY: checked
        assert_eq!(&line, b"GET /v1/hello HTTP/1.1\r\n\r\n", "the request, as sent"); // WHAT: unchanged. WHY: plain bytes relayed
        cell.write_all(b"ok").await.unwrap(); // WHAT: the answer. WHY: back through TLS
        let mut back = [0u8; 2]; // WHAT: what the caller gets. WHY: checked
        tls.read_exact(&mut back).await.unwrap(); // WHAT: read. WHY: below
        assert_eq!(&back, b"ok", "the cell's answer, back over TLS"); // WHAT: carried. WHY: both ways
        current.set_target(None); // WHAT: the cell stopped. WHY: a new connection is closed
        let tcp = tokio::net::TcpStream::connect(address).await.unwrap(); // WHAT: another caller. WHY: below
        let mut closed = tcp; // WHAT: its socket. WHY: read below
        let mut nothing = [0u8; 1]; // WHAT: room for a byte. WHY: none comes
        assert_eq!(closed.read(&mut nothing).await.unwrap_or(0), 0, "with no cell running, a connection is closed at once"); // WHAT: closed. WHY: nothing to relay to
    }
}
