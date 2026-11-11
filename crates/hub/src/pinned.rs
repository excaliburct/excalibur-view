//! HTTPS to a server whose certificate is pinned, not trusted.
//!
//! Excalibur's own servers (Citadel Server, CAD's server, and in time the
//! Office Server itself) make their own certificate. No certificate authority
//! vouches for it, so it is checked the way Citadel's office computers check
//! it: by the SHA-256 of its public key (the SPKI), kept from the first time
//! and compared on every connection after. No names checked, no authority, and
//! no "accept anyway". The handshake's signature is still checked, so only the
//! holder of that key's private half can answer.
//!
//! The pin is 64 lowercase hex digits, the same text Citadel's `Pinning.Of`
//! makes, so a pin read off either side compares as is.

use std::sync::{Arc, Mutex};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

/// The pin of a certificate: the SHA-256 of its SPKI, as 64 lowercase hex
/// digits. `None` for something that doesn't read as a certificate.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn pin_of(certificate: &[u8]) -> Option<String> {
    let der = CertificateDer::from(certificate); // WHAT: the certificate's bytes, borrowed. WHY: rustls reads it from these
    let parsed = rustls::server::ParsedCertificate::try_from(&der).ok()?; // WHAT: read as an X.509 certificate. WHY: the SPKI is inside it
    Some(crate::update::hex(&Sha256::digest(parsed.subject_public_key_info().as_ref()))) // WHAT: SHA-256 of the SPKI, in hex. WHY: Citadel's pin, exactly
}

/// The pin in short, for a person to compare by eye: the first 16 hex digits
/// in groups of four ("3F2A 91C0 …"), as Citadel shows it.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn short(pin: &str) -> String {
    pin.chars().take(16).collect::<Vec<_>>().chunks(4).map(|c| c.iter().collect::<String>().to_uppercase()).collect::<Vec<_>>().join(" ") // WHAT: four groups of four, upper case. WHY: Citadel's Pinning.Short, so both read the same
}

/// Whether `pin` reads as a pin: 64 lowercase hex digits.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn is_pin(pin: &str) -> bool {
    pin.len() == 64 && pin.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) // WHAT: the shape. WHY: anything else can never match, and is refused before use
}

/// Compared to the end whichever way it goes, so how long it takes says
/// nothing about how much of the pin was right.
fn same(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false; // WHAT: different lengths never match. WHY: a pin's length is public
    }
    a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0 // WHAT: every byte compared. WHY: fixed time
}

/// Checks the certificate against a pin, or, with no pin yet, notes the one it
/// saw so the caller can keep it once the answer proves who it was.
#[derive(Debug)]
pub struct Pinned {
    pin: Option<String>, // WHAT: the pin kept from before, or none on the first connection. WHY: the one rule this check applies
    seen: Mutex<Option<String>>, // WHAT: the pin of the certificate this connection was given. WHY: kept by the caller on a first connection, shown on a mismatch
    algorithms: WebPkiSupportedAlgorithms, // WHAT: the signature algorithms the handshake may use. WHY: the provider's own, so the signature check is rustls' real one
}

impl Pinned {
    /// The pin the server showed, once a connection has been tried.
    pub fn seen(&self) -> Option<String> {
        self.seen.lock().ok().and_then(|s| s.clone()) // WHAT: a copy of the pin seen. WHY: read after the request
    }
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let Some(seen) = pin_of(end_entity.as_ref()) else {
            return Err(rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)); // WHAT: not a certificate. WHY: nothing to pin
        };
        if let Ok(mut slot) = self.seen.lock() {
            *slot = Some(seen.clone()); // WHAT: noted. WHY: the caller keeps it on a first connection, or reports it on a mismatch
        }
        match &self.pin {
            Some(pin) if same(pin, &seen) => Ok(ServerCertVerified::assertion()), // WHAT: the pinned key. WHY: the one rule
            Some(_) => Err(rustls::Error::InvalidCertificate(rustls::CertificateError::ApplicationVerificationFailure)), // WHAT: another key. WHY: a different server, or someone in between: never "accept anyway"
            None => Ok(ServerCertVerified::assertion()), // WHAT: no pin yet: let the first answer through. WHY: the caller keeps the pin only if that answer proves who it was
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algorithms) // WHAT: rustls' own check. WHY: proves the server holds the key it showed
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms) // WHAT: rustls' own check. WHY: the same
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes() // WHAT: what this check can verify. WHY: offered in the handshake
    }
}

/// A client that reaches one server by its pin, and the check it uses, to read
/// the pin it saw afterwards. A `pin` that isn't one is refused here.
///
/// # Errors
///
/// Refused, in words a person can act on, when it can't be done.
pub fn agent(pin: Option<&str>) -> Result<(ureq::AgentBuilder, Arc<Pinned>), String> {
    if let Some(p) = pin {
        if !is_pin(p) {
            return Err("that pin is not 64 lowercase hex digits".into()); // WHAT: refused. WHY: a malformed pin would never match, and must never mean "none"
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider()); // WHAT: the same crypto the program's other connections use. WHY: one implementation to trust
    let check = Arc::new(Pinned {
        pin: pin.map(str::to_string),
        seen: Mutex::new(None),
        algorithms: provider.signature_verification_algorithms,
    }); // WHAT: the pin check. WHY: shared with the caller, who reads what was seen
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("no TLS versions: {e}"))?
        .dangerous() // WHAT: rustls' name for "my own certificate check". WHY: the check is the pin, which is stricter than an authority, not looser
        .with_custom_certificate_verifier(check.clone())
        .with_no_client_auth(); // WHAT: no client certificate. WHY: these reads are open on the server's side
    Ok((ureq::AgentBuilder::new().tls_config(Arc::new(config)).redirects(0), check)) // WHAT: no redirects. WHY: a pinned connection answers from the pinned server or not at all; a redirect would put another host's certificate in `seen`
}

/// HTTPS servers on this machine with made-up certificates, for tests here
/// and in the crates that use this one (the `testing` feature). Never in a
/// build that ships.
#[cfg(any(test, feature = "testing"))]
#[allow(clippy::missing_panics_doc, clippy::cast_possible_truncation)] // WHAT: test servers that stop on a broken setup, and DER lengths written as bytes. WHY: test helpers only; every length here is under 64 KiB, checked as it is written
pub mod testing {
    use std::sync::Arc;

    use super::pin_of;

    /// DER: a tag, its length, its content.
    fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![tag]; // WHAT: the tag. WHY: DER's first byte
        match content.len() {
            n if n < 0x80 => out.push(n as u8), // WHAT: a short length. WHY: one byte below 128
            n if n < 0x100 => out.extend([0x81, n as u8]), // WHAT: a one-byte long length. WHY: up to 255
            n => out.extend([0x82, (n >> 8) as u8, n as u8]), // WHAT: a two-byte long length. WHY: these certificates never pass 64 KiB
        }
        out.extend_from_slice(content); // WHAT: the content. WHY: after its length
        out
    }

    /// A self-made Ed25519 certificate and its PKCS#8 key, from a made-up
    /// seed: no key file is kept anywhere, and each seed is another server.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn certificate(seed: u8) -> (Vec<u8>, Vec<u8>) {
        use ed25519_dalek::Signer;
        let key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]); // WHAT: a key from the seed. WHY: the same every run, and never a real one
        let ed25519 = tlv(0x30, &tlv(0x06, &[0x2b, 0x65, 0x70])); // WHAT: the Ed25519 algorithm (1.3.101.112). WHY: named in three places
        let name = tlv(0x30, &tlv(0x31, &tlv(0x30, &[tlv(0x06, &[0x55, 0x04, 0x03]), tlv(0x0c, b"citadel-test")].concat()))); // WHAT: CN=citadel-test. WHY: a name is required, though nothing checks it
        let validity = tlv(0x30, &[tlv(0x17, b"250101000000Z"), tlv(0x17, b"491231000000Z")].concat()); // WHAT: 2025 to 2049. WHY: the dates are not what's checked
        let mut bits = vec![0u8]; // WHAT: no unused bits. WHY: a BIT STRING's first byte
        bits.extend(key.verifying_key().to_bytes()); // WHAT: the public key. WHY: inside the SPKI
        let spki = tlv(0x30, &[ed25519.clone(), tlv(0x03, &bits)].concat()); // WHAT: the SubjectPublicKeyInfo. WHY: what the pin is of
        let tbs = tlv(
            0x30,
            &[tlv(0xa0, &tlv(0x02, &[2])), tlv(0x02, &[1]), ed25519.clone(), name.clone(), validity, name, spki].concat(),
        ); // WHAT: version 3, serial 1, the algorithm, issuer, dates, subject, key. WHY: the signed part
        let mut signature = vec![0u8]; // WHAT: no unused bits. WHY: the same
        signature.extend(key.sign(&tbs).to_bytes()); // WHAT: signed by itself. WHY: a self-made certificate
        let cert = tlv(0x30, &[tbs, ed25519, tlv(0x03, &signature)].concat()); // WHAT: the certificate. WHY: what the server shows
        let mut pkcs8 = vec![0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20]; // WHAT: PKCS#8's fixed Ed25519 opening. WHY: rustls takes the key this way
        pkcs8.extend([seed; 32]); // WHAT: the seed. WHY: the private key itself
        (cert, pkcs8)
    }

    /// An HTTPS server on this machine that answers one request with `body`,
    /// showing the certificate made from `seed`. Its address, and its pin.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn serve_once(seed: u8, body: &'static str) -> (String, String) {
        let (cert, pkcs8) = certificate(seed);
        let pin = pin_of(&cert).expect("the made-up certificate reads"); // WHAT: its pin. WHY: what the tests compare
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.into()], rustls::pki_types::PrivateKeyDer::Pkcs8(pkcs8.into()))
            .expect("the made-up key matches its certificate"); // WHAT: the server's TLS. WHY: a real handshake to check against
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); // WHAT: any free port. WHY: tests run side by side
        let address = format!("https://127.0.0.1:{}/api/version", listener.local_addr().unwrap().port());
        std::thread::spawn(move || {
            let Ok((tcp, _)) = listener.accept() else { return };
            let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(Arc::new(config)).unwrap(), tcp); // WHAT: the connection, over TLS. WHY: the client's check runs in this handshake
            answer(&mut tls, body); // WHAT: one request answered. WHY: what a Citadel Server's /api/version does
        });
        (address, pin)
    }

    /// Reads one request to its end and answers it with `body`.
    fn answer(tls: &mut rustls::StreamOwned<rustls::ServerConnection, std::net::TcpStream>, body: &str) {
        use std::io::{Read, Write};
        let mut asked = Vec::new();
        let mut byte = [0u8; 1];
        while !asked.ends_with(b"\r\n\r\n") && tls.read(&mut byte).is_ok_and(|n| n == 1) {
            asked.push(byte[0]); // WHAT: the request, to its end. WHY: answered only after it has been read
        }
        let _ = write!(tls, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()); // WHAT: the answer. WHY: a well-formed HTTP reply
        tls.conn.send_close_notify(); // WHAT: a clean end. WHY: the client reads to it
        let _ = tls.flush();
    }

    /// Shows one certificate and signs with another key: someone who copied
    /// the real server's certificate, which is public, but not its key.
    #[derive(Debug)]
    struct Copied(Arc<rustls::sign::CertifiedKey>);

    impl rustls::server::ResolvesServerCert for Copied {
        fn resolve(&self, _: rustls::server::ClientHello<'_>) -> Option<Arc<rustls::sign::CertifiedKey>> {
            Some(self.0.clone()) // WHAT: always the mismatched pair. WHY: the attack under test
        }
    }

    /// A server showing the certificate made from `shown` while holding only
    /// the key made from `held`. Its address.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn impostor(shown: u8, held: u8) -> String {
        let (cert, _) = certificate(shown); // WHAT: the real server's certificate. WHY: copied, as anyone can
        let (_, pkcs8) = certificate(held); // WHAT: the impostor's own key. WHY: it doesn't have the real one
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let key = provider.key_provider.load_private_key(rustls::pki_types::PrivateKeyDer::Pkcs8(pkcs8.into())).unwrap(); // WHAT: the impostor's signing key. WHY: it signs the handshake
        let pair = Arc::new(rustls::sign::CertifiedKey::new(vec![cert.into()], key)); // WHAT: put together without the match check. WHY: as an attacker would
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(Copied(pair))); // WHAT: always the copied certificate. WHY: the same
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("https://127.0.0.1:{}/api/version", listener.local_addr().unwrap().port());
        std::thread::spawn(move || {
            let Ok((tcp, _)) = listener.accept() else { return };
            let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(Arc::new(config)).unwrap(), tcp);
            answer(&mut tls, r#"{"version":"6.6.6"}"#); // WHAT: answers like the real one. WHY: so only the handshake's signature can stop it
        });
        address
    }


}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn a_pin_is_sixty_four_lowercase_hex_digits_and_nothing_else() {
        assert!(is_pin(&"a".repeat(64)));
        assert!(!is_pin(&"A".repeat(64)), "upper case is not the form Citadel writes");
        assert!(!is_pin(&"a".repeat(63)));
        assert!(!is_pin(&format!("{}g", "a".repeat(63))));
        assert!(!is_pin(""));
        assert!(agent(Some("not a pin")).is_err(), "a malformed pin never means 'no pin'");
        assert!(agent(Some(&"0".repeat(64))).is_ok());
        assert!(agent(None).is_ok());
    }

    #[test]
    fn a_copied_certificate_without_its_key_is_refused() {
        let (_, pin) = serve_once(5, "{}"); // WHAT: the real server's pin. WHY: what the client holds
        let address = impostor(5, 6); // WHAT: its certificate, someone else's key. WHY: the pin alone matches
        let (answer, seen) = ask(&address, Some(&pin));
        assert_eq!(seen.as_deref(), Some(pin.as_str()), "the pin matched, so only the signature can refuse it");
        assert!(answer.is_err(), "the handshake signature is checked: {answer:?}");
    }

    fn ask(address: &str, pin: Option<&str>) -> (Result<String, String>, Option<String>) {
        let (agent, check) = agent(pin).unwrap();
        let answer = agent
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .get(address)
            .call()
            .map_err(|e| e.to_string())
            .and_then(|r| r.into_string().map_err(|e| e.to_string())); // WHAT: the body, or why not. WHY: each test reads one
        (answer, check.seen())
    }

    #[test]
    fn the_pinned_server_answers() {
        let (address, pin) = serve_once(1, r#"{"version":"1.0.2"}"#);
        let (answer, seen) = ask(&address, Some(&pin));
        assert_eq!(answer.as_deref(), Ok(r#"{"version":"1.0.2"}"#));
        assert_eq!(seen.as_deref(), Some(pin.as_str()));
    }

    #[test]
    fn another_certificate_is_refused_and_its_pin_is_reported() {
        let (_, pinned) = serve_once(2, "{}"); // WHAT: the server that was paired. WHY: its pin is what's kept
        let (address, other) = serve_once(3, r#"{"version":"9.9.9"}"#); // WHAT: a different server on the address. WHY: an impostor, or a reinstall
        let (answer, seen) = ask(&address, Some(&pinned));
        assert!(answer.is_err(), "a certificate with another key never answers: {answer:?}");
        assert_eq!(seen.as_deref(), Some(other.as_str()), "the pin it showed is reported, for a person to compare");
    }

    #[test]
    fn with_no_pin_yet_the_first_answer_comes_through_and_shows_its_pin() {
        let (address, pin) = serve_once(4, r#"{"version":"1.0.2"}"#);
        let (answer, seen) = ask(&address, None);
        assert!(answer.is_ok(), "{answer:?}");
        assert_eq!(seen.as_deref(), Some(pin.as_str()), "the caller keeps this pin");
    }

    #[test]
    fn pins_compare_whole() {
        let a = "0".repeat(64);
        assert!(same(&a, &a));
        assert!(!same(&a, &format!("{}1", "0".repeat(63))), "the last digit counts");
        assert!(!same(&a, &format!("1{}", "0".repeat(63))), "the first digit counts");
        assert!(!same(&a, &"0".repeat(63)));
        assert!(pin_of(b"not a certificate").is_none());
    }
}
