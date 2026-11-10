//! The Office Server's own certificate, so sign-ins and drawings travel
//! encrypted, and HTTPS served with it beside the plain-HTTP API.
//!
//! Made the first time the server starts and kept beside the data: the
//! certificate (public) and its private key (readable by the server alone).
//! No certificate authority is involved. A seat keeps the certificate's pin
//! the first time it connects and refuses a server that shows another, the way
//! Citadel's office computers pin Citadel Server: the pin is the SHA-256 of
//! the certificate's public key (`hub::pinned`), so one rule covers every
//! Excalibur server. The pin is in the log as the server starts and in the
//! Manager, so a person can compare it.
//!
//! Ported from Excalibur CAD's server, which serves its HTTPS the same way.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Connections served at once (an office has a handful of computers),
/// and from any one address (so one machine can't take them all).
const MOST_CONNECTIONS: usize = 512; // WHAT: the cap on open connections. WHY: a flood waits instead of exhausting the machine
const MOST_FROM_ONE: usize = 16; // WHAT: the cap from one address. WHY: one machine can't take every slot
/// The longest one connection is kept, however it's used.
const CONNECTION: Duration = Duration::from_secs(3600); // WHAT: an hour. WHY: a 512 MB drawing set over a slow tunnel fits; a held-open socket doesn't stay forever
/// How long a connection may take to finish its encryption handshake.
const HANDSHAKE: Duration = Duration::from_secs(10); // WHAT: ten seconds. WHY: a stalled handshake gives its slot back

/// The certificate's file, in the data folder.
pub const CERT: &str = "tls-cert.pem"; // WHAT: the public certificate. WHY: kept with the data, backed up with it
/// The private key's file, in the data folder.
pub const KEY: &str = "tls-key.pem"; // WHAT: the private key. WHY: readable by the server alone

/// The encryption settings for the certificate and key (PEM).
pub fn server_config(cert_pem: &str, key_pem: &str) -> Result<Arc<rustls::ServerConfig>, String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let cert = CertificateDer::from_pem_slice(cert_pem.as_bytes()).map_err(|_| "the certificate file is damaged".to_string())?; // WHAT: the certificate read. WHY: shown in every handshake
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(|_| "the key file is damaged".to_string())?; // WHAT: the key read. WHY: signs every handshake
    let provider = Arc::new(rustls::crypto::ring::default_provider()); // WHAT: the same crypto the client uses. WHY: one implementation to trust
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(|e| format!("the server's certificate: {e}"))?; // WHAT: checks the key matches the certificate. WHY: a mismatched pair is refused at start, not at the first seat
    config.alpn_protocols = vec![b"http/1.1".to_vec()]; // WHAT: HTTP/1.1 only. WHY: what the seats and Fleet speak
    Ok(Arc::new(config))
}

/// The pin of a PEM certificate: the SHA-256 of its public key, as the seats
/// keep it.
pub fn pin(cert_pem: &str) -> Result<String, String> {
    use rustls::pki_types::pem::PemObject;
    let der = rustls::pki_types::CertificateDer::from_pem_slice(cert_pem.as_bytes()).map_err(|_| "the certificate file is damaged".to_string())?; // WHAT: the certificate's DER. WHY: the pin is read from it
    hub::pinned::pin_of(der.as_ref()).ok_or_else(|| "the certificate file is damaged".to_string()) // WHAT: SHA-256 of the SPKI. WHY: the same rule as Citadel's pin
}

/// Answers `router` over HTTPS on `listener` until `shutdown` finishes: each
/// connection encrypted, its address handed to the handlers (sign-ins are
/// limited per address, and presence says where a seat is), a stalled
/// handshake dropped.
pub async fn serve(
    listener: tokio::net::TcpListener,
    config: Arc<rustls::ServerConfig>,
    router: axum::Router,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) {
    use tower::ServiceExt;
    let acceptor = tokio_rustls::TlsAcceptor::from(config); // WHAT: the TLS side of each connection. WHY: rustls, as everywhere else
    let slots = Arc::new(tokio::sync::Semaphore::new(MOST_CONNECTIONS)); // WHAT: the open-connection slots. WHY: the cap above
    let per_address: Shares = Default::default(); // WHAT: connections per address. WHY: the per-machine cap
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        let accepted = tokio::select! {
            _ = &mut shutdown => return, // WHAT: stop accepting. WHY: the server is stopping
            a = listener.accept() => a,
        };
        let Ok((tcp, peer)) = accepted else {
            continue; // WHAT: a failed accept is skipped. WHY: one bad connection never stops the server
        };
        let Some(share) = Share::take(&per_address, peer.ip()) else {
            continue; // WHAT: past its address's share, closed straight away. WHY: one machine can't take every slot
        };
        let Ok(slot) = slots.clone().acquire_owned().await else {
            return; // WHAT: the slots are gone. WHY: only when stopping
        };
        let (acceptor, router) = (acceptor.clone(), router.clone());
        tokio::spawn(async move {
            let (_slot, _share) = (slot, share); // WHAT: held for the connection's life. WHY: given back when it closes
            let Ok(Ok(tls)) = tokio::time::timeout(HANDSHAKE, acceptor.accept(tcp)).await else {
                return; // WHAT: a failed or stalled handshake. WHY: dropped, its slot given back
            };
            let service = hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
                req.extensions_mut().insert(axum::extract::ConnectInfo::<SocketAddr>(peer)); // WHAT: the caller's address. WHY: the same as the plain-HTTP API hands the handlers
                router.clone().oneshot(req)
            });
            let connection = hyper::server::conn::http1::Builder::new()
                .timer(hyper_util::rt::TokioTimer::new())
                .header_read_timeout(Duration::from_secs(30)) // WHAT: headers within 30 seconds. WHY: a request trickled in to hold a slot is dropped
                .serve_connection(hyper_util::rt::TokioIo::new(tls), service);
            let _ = tokio::time::timeout(CONNECTION, connection).await; // WHAT: the connection, up to its limit. WHY: none is kept forever
        });
    }
}

type Shares = Arc<std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, usize>>>;

/// One connection counted against its address, given back when it closes.
struct Share {
    counts: Shares,
    ip: std::net::IpAddr,
}

impl Share {
    fn take(counts: &Shares, ip: std::net::IpAddr) -> Option<Share> {
        let mut map = counts.lock().unwrap_or_else(|e| e.into_inner()); // WHAT: the counts. WHY: a poisoned lock still counts
        let n = map.entry(ip).or_insert(0);
        if *n >= MOST_FROM_ONE {
            return None; // WHAT: this address has its share. WHY: the per-machine cap
        }
        *n += 1;
        Some(Share { counts: counts.clone(), ip })
    }
}

impl Drop for Share {
    fn drop(&mut self) {
        let mut map = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = map.get_mut(&self.ip) {
            *n -= 1; // WHAT: given back. WHY: the connection closed
            if *n == 0 {
                map.remove(&self.ip); // WHAT: forgotten at zero. WHY: the map never grows with every address ever seen
            }
        }
    }
}

/// The certificate and key (PEM), made and saved if there aren't any yet.
pub fn load_or_make(data: &Path) -> Result<(String, String), String> {
    let (cert_path, key_path) = (data.join(CERT), data.join(KEY));
    use std::io::ErrorKind::NotFound;
    let (cert, key) = (std::fs::read_to_string(&cert_path), std::fs::read_to_string(&key_path));
    match (&cert, &key) {
        (Ok(c), Ok(k)) => return Ok((c.clone(), k.clone())), // WHAT: both there. WHY: the server's identity, kept
        (Err(a), Err(b)) if a.kind() == NotFound && b.kind() == NotFound => {} // WHAT: neither yet. WHY: made below
        (Err(a), Ok(_)) if a.kind() == NotFound => remove(&key_path)?, // WHAT: a key without its certificate (a start cut short). WHY: both made again
        (Ok(_), Err(b)) if b.kind() == NotFound => remove(&cert_path)?, // WHAT: a certificate without its key. WHY: the same
        (Err(e), _) | (_, Err(e)) => {
            return Err(format!("couldn't read the server's certificate or key in {}: {e}", data.display())); // WHAT: anything else stops here. WHY: the server's identity never changes by accident
        }
    }
    let made = rcgen::generate_simple_self_signed(vec!["excalibur-office-server".to_string()]).map_err(|e| format!("making the server's certificate: {e}"))?; // WHAT: a fresh key and certificate. WHY: no authority is involved; the seats pin it
    let (cert, key) = (made.cert.pem(), made.key_pair.serialize_pem());
    write_private(&key_path, &key)?; // WHAT: the key first, private from the moment it exists. WHY: never readable by anyone else, even briefly
    std::fs::write(&cert_path, &cert).map_err(|e| format!("saving the certificate: {e}"))?; // WHAT: the certificate. WHY: public, kept beside it
    Ok((cert, key))
}

fn remove(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| format!("clearing {}: {e}", path.display())) // WHAT: a half-made pair's leftover. WHY: made again whole
}

/// The private key, readable by the server alone from the moment it exists
/// (on Windows the data folder's own permissions do the same).
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true); // WHAT: a new file only. WHY: never writes into one somebody put there
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(0o600); // WHAT: the owner only. WHY: private from creation
    }
    open.open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| format!("saving the server's key: {e}"))?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x0800_0000; // WHAT: no console window. WHY: a service shows none
        let system = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
        let icacls = |service: bool| {
            let mut c = std::process::Command::new(system.join("System32").join("icacls.exe")); // WHAT: Windows' own tool, by its full path. WHY: never one found on PATH
            c.arg(path).args(["/inheritance:r", "/grant:r", "*S-1-3-4:F", "*S-1-5-18:F", "*S-1-5-32-544:F"]); // WHAT: its owner, SYSTEM and Administrators. WHY: nobody else reads the key
            if service {
                c.arg(format!(r"NT SERVICE\{}:R", SERVICE)); // WHAT: the service's own account, read only. WHY: it runs the server
            }
            c.arg("/Q").creation_flags(NO_WINDOW).status().map(|s| s.success()).unwrap_or(false)
        };
        if !(icacls(true) || icacls(false)) {
            let _ = std::fs::remove_file(path); // WHAT: a key that couldn't be made private is taken away. WHY: never left readable
            return Err("Windows wouldn't make the server's key private.".into());
        }
    }
    Ok(())
}

/// The Windows service's name, whose account reads the key.
#[cfg(windows)]
const SERVICE: &str = "ExcaliburHyperviewServer"; // WHAT: the same as winsvc::NAME. WHY: the account Windows makes for the service

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_address_gets_its_share_of_connections_and_no_more() {
        let counts: Shares = Default::default();
        let ip: std::net::IpAddr = [10, 0, 0, 9].into();
        let held: Vec<Share> = (0..MOST_FROM_ONE).map(|_| Share::take(&counts, ip).unwrap()).collect();
        assert!(Share::take(&counts, ip).is_none(), "the 17th waits its turn");
        assert!(Share::take(&counts, [10, 0, 0, 10].into()).is_some(), "another machine isn't held up");
        drop(held);
        assert!(Share::take(&counts, ip).is_some(), "closed ones are given back");
    }

    fn a_folder() -> std::path::PathBuf {
        let n: u64 = rand::random();
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("hv-tls-{n:016x}")); // WHAT: under target, as tunnel's tests do. WHY: a key a test writes is never somewhere another account shares
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_certificate_is_made_once_and_kept() {
        let data = a_folder();
        let (cert, key) = load_or_make(&data).unwrap();
        assert!(server_config(&cert, &key).is_ok(), "the pair made is a pair that serves");
        let again = load_or_make(&data).unwrap();
        assert_eq!(again, (cert.clone(), key), "the second start keeps the first's identity");
        assert!(hub::pinned::is_pin(&pin(&cert).unwrap()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(data.join(KEY)).unwrap().permissions().mode() & 0o777, 0o600, "the key is the owner's alone");
        }
    }

    #[test]
    fn half_a_pair_is_made_whole_and_a_damaged_one_is_refused() {
        let data = a_folder();
        let (cert, _) = load_or_make(&data).unwrap();
        std::fs::remove_file(data.join(KEY)).unwrap(); // WHAT: the key gone. WHY: a start cut short
        let (fresh, _) = load_or_make(&data).unwrap();
        assert_ne!(pin(&fresh).unwrap(), pin(&cert).unwrap(), "a new pair, not a certificate without its key");
        assert!(server_config("not a certificate", "not a key").is_err());
        let (_, other_key) = load_or_make(&a_folder()).unwrap();
        assert!(server_config(&fresh, &other_key).is_err(), "a key that isn't the certificate's is refused at start");
    }
}
