//! Moving a seat from an office server's plain HTTP to its HTTPS, and keeping
//! it there.
//!
//! An Office Server says in its health answer where its HTTPS is
//! (`secure_port`). The first time a seat connects there it keeps the
//! certificate's pin (`crate::pinned`); every connection after must show that
//! same certificate. Once a seat holds a pin for a server it never falls back
//! to plain HTTP: someone in between who blocks HTTPS can't push it back to an
//! unencrypted connection. A server that doesn't offer HTTPS (an older one),
//! and that this seat holds no pin for, is reached as before.
//!
//! The pin itself is never taken from the health answer, which came over
//! plain HTTP: only from the HTTPS handshake. Pins are kept per server as the
//! connection reads its address (scheme and host in lower case, the port
//! spelled out), so `HTTP://Office:8714/` and `http://office:8714` are one.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::client::{Client, Trouble};

/// The pins this seat keeps, one per office server, in a small file.
pub struct PinBook {
    path: PathBuf, // WHAT: the file. WHY: kept between runs, in the seat's own settings folder
}

/// The book's file is there but doesn't read: every server counts as pinned.
struct Damaged;

impl PinBook {
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn at(path: PathBuf) -> PinBook {
        PinBook { path }
    }

    fn all(&self) -> Result<BTreeMap<String, String>, Damaged> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).map_err(|_| Damaged), // WHAT: every kept pin, or damaged. WHY: a book that doesn't read must never mean "no pins", or every seat would fall back to plain HTTP
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()), // WHAT: no book yet. WHY: a seat that never pinned anything
            Err(_) => Err(Damaged), // WHAT: there, but unreadable. WHY: the same as damaged
        }
    }

    /// The pin kept for `server`, if any and if it reads as one.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn pin(&self, server: &str) -> Option<String> {
        self.all().ok()?.remove(server).filter(|p| crate::pinned::is_pin(p)) // WHAT: the pin, only if it reads as one. WHY: a damaged entry is never used as a pin (and `holds` still sees it)
    }

    /// Whether anything at all is kept for `server`, readable or not. A book
    /// that doesn't read holds every server.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn holds(&self, server: &str) -> bool {
        self.all().map_or(true, |all| all.contains_key(server)) // WHAT: an entry, even a damaged one; or a damaged book. WHY: either still forbids falling back to plain HTTP
    }

    fn keep(&self, server: &str, pin: &str) -> Result<(), String> {
        let mut all = self.all().map_err(|_| "this computer's record of server certificates doesn't read, so nothing new is written over it".to_string())?; // WHAT: never rewritten when damaged. WHY: the other servers' pins would be lost
        all.insert(server.to_string(), pin.to_string()); // WHAT: this server's pin. WHY: kept from its first HTTPS answer
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()); // WHAT: a fresh number. WHY: two windows keeping a pin at once never write into one file
        let part = self.path.with_extension(format!("part{n}"));
        let written = (|| {
            use std::io::Write;
            let mut f = std::fs::File::create(&part)?;
            f.write_all(&serde_json::to_vec_pretty(&all).unwrap_or_default())?;
            f.sync_all() // WHAT: on the disk before it is named. WHY: a power cut never leaves half a book
        })()
        .and_then(|()| std::fs::rename(&part, &self.path)); // WHAT: moved into place. WHY: all at once
        if let Err(e) = written {
            let _ = std::fs::remove_file(&part); // WHAT: the leftover. WHY: never a stray part file
            return Err(e.to_string());
        }
        Ok(())
    }
}

/// How a seat's address for a server reads.
#[derive(Debug, PartialEq)]
enum Address {
    /// `https://…`: a tunnel's address, checked by its certificate authority.
    Https,
    /// `http://host:port`, as (`host:port`, `host`): moved to HTTPS by its pin.
    Plain(String, String),
}

/// Reads `base` the way the connection will. Anything that isn't a plain
/// http or https address of a host is refused, never used as it is.
fn address(base: &str) -> Result<Address, Trouble> {
    let odd = || Trouble::Unreadable(format!("{base} isn't an office server's address.")); // WHAT: the refusal. WHY: an odd address is never guessed at
    let url = url::Url::parse(base.trim()).map_err(|_| odd())?; // WHAT: parsed as the connection parses it. WHY: one server, one key, however it was typed
    if !url.username().is_empty() || url.password().is_some() {
        return Err(odd()); // WHAT: a user name in the address. WHY: `office@evil` would reach evil
    }
    match url.scheme() {
        "https" => Ok(Address::Https),
        "http" => {
            let host = url.host_str().ok_or_else(odd)?.trim_end_matches('.').to_ascii_lowercase(); // WHAT: the host, without a trailing dot. WHY: `office.` and `office` are one machine
            if host.is_empty() {
                return Err(odd());
            }
            let port = url.port_or_known_default().ok_or_else(odd)?; // WHAT: the port, 80 when none is written. WHY: `office` and `office:80` are one server
            Ok(Address::Plain(format!("{host}:{port}"), host))
        }
        _ => Err(odd()), // WHAT: any other kind of address. WHY: an office server is http or https
    }
}

/// Whether this seat holds a pin for `base`'s server (a damaged book counts).
/// A seat that does never sends a sign-in anywhere else in its place.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn is_pinned(base: &str, book: &PinBook) -> bool {
    match address(base) {
        Ok(Address::Plain(server, _)) => book.holds(&server),
        _ => false, // WHAT: an https address, or one that doesn't read. WHY: the first is checked by its authority; the second is refused anyway
    }
}

/// Where else a seat looks, and what else it holds a server to: the file
/// every app on this computer shares (`crate::office`), and the office
/// certificate keys this build carries.
#[derive(Clone, Copy, Debug, Default)]
pub struct Also<'a> {
    pub shared: Option<&'a std::path::Path>, // WHAT: the shared office.json, when the app uses it. WHY: §2.3: the second app on a PC never asks again
    pub office_keys: &'a [(&'a str, &'a str)], // WHAT: the office certificate keys built in. WHY: §2.4; empty until certificates are signed
    pub allow_plain: bool, // WHAT: the person confirmed this server is an old, unencrypted one. WHY: review: never plain HTTP without that, and never for an Office Server
    pub replace_shared: bool, // WHAT: the person confirmed this is a different office server from the one the other apps know. WHY: review: the shared record is never swapped quietly
}

/// The code a refusal carries when the person must confirm an old,
/// unencrypted server before anything is sent to it.
pub const CONFIRM_UNENCRYPTED: &str = "confirm_unencrypted"; // WHAT: the refusal's name. WHY: the app shows the warning and a button

/// The code a refusal carries when the server isn't the one the other apps
/// on this computer know, and the person must confirm the change.
pub const CONFIRM_OTHER_SERVER: &str = "confirm_other_server"; // WHAT: the refusal's name. WHY: the app shows both short pins and a button

/// Every address `host` looks up to, once, and whether all are local.
fn looked_up(host: &str) -> (Vec<std::net::IpAddr>, bool) {
    let bare = host.trim_start_matches('[').trim_end_matches(']'); // WHAT: an IPv6 address without its brackets. WHY: parsed as an address
    let ips: Vec<std::net::IpAddr> = match bare.parse::<std::net::IpAddr>() {
        Ok(ip) => vec![ip], // WHAT: an address as typed. WHY: nothing to look up
        Err(_) => std::net::ToSocketAddrs::to_socket_addrs(&(bare, 0)).map(|a| a.map(|s| s.ip()).collect()).unwrap_or_default(), // WHAT: looked up once. WHY: review: the same addresses are checked and connected to
    };
    let local = !ips.is_empty() && ips.iter().all(|ip| crate::office::is_local(*ip)); // WHAT: every one on a local network. WHY: §2.2
    (ips, local)
}

/// Whether a health answer says it is an Office Server (0.7.0 or later),
/// which always serves HTTPS.
fn is_office_server(health: &crate::model::Health) -> bool {
    health.product == "excalibur-office" || crate::update::compare(&health.version, "0.7.0") != std::cmp::Ordering::Less // WHAT: the product word, or a version from 0.7.0 on. WHY: review: such a server is never reached over plain HTTP
}

/// The refusal for an old, unencrypted server the person hasn't confirmed.
fn unencrypted(host: &str, local: bool) -> Trouble {
    if !local {
        return Trouble::Unreadable(format!("{host} doesn't offer an encrypted connection and isn't on the office network, so this computer won't connect to it.")); // WHAT: refused for good. WHY: a password is never sent unencrypted across the internet
    }
    Trouble::Refused(crate::model::Problem::new(
        CONFIRM_UNENCRYPTED,
        "This server doesn't offer an encrypted connection. It may be an old Excalibur View server (before 0.7.0), or someone pretending to be the office server. Your password would be sent unencrypted. Only go on if you know this server is old, or ask whoever looks after it to update it.",
    )) // WHAT: refused until confirmed. WHY: review: an explicit warning before any password is sent
}

/// The refusal for a server that isn't the one the other apps know.
fn other_server(kept: &str, seen: &str) -> Trouble {
    Trouble::Refused(crate::model::Problem::new(
        CONFIRM_OTHER_SERVER,
        format!(
            "This computer's other Excalibur programs know a different office server (certificate {}) from this one (certificate {}). If the office really has a new server, confirm it; otherwise someone may be pretending to be the server. Ask whoever looks after it.",
            crate::pinned::short(kept),
            crate::pinned::short(seen)
        ),
    )) // WHAT: refused until confirmed, both short pins shown. WHY: review: the shared record is never swapped quietly
}

/// The client a seat should use for `base`: the server's HTTPS by its kept pin
/// (kept now, on a first connection), or `base` itself when the server offers
/// no HTTPS and this seat has never held a pin for it.
///
/// # Errors
///
/// Refused, in words a person can act on, when it can't be done.
pub fn open(base: &str, book: &PinBook) -> Result<Client, Trouble> {
    open_with(base, book, Also::default()) // WHAT: the pin book alone. WHY: what every caller did before the shared file
}

/// [`open`], also reading and keeping the pin in the shared file, and holding
/// a server once seen verified to its office certificate (§2).
///
/// - A first contact keeps a pin only when the server's address is on a
///   local network (§2.2); anywhere else it is refused.
/// - A pin kept, here or in the shared file, is never exchanged for another
///   (guard 5).
/// - A server whose office certificate was once seen verified is never
///   accepted without one again (guard 6).
///
/// # Errors
///
/// Refused, in words a person can act on, when it can't be done.
pub fn open_with(base: &str, book: &PinBook, also: Also<'_>) -> Result<Client, Trouble> {
    let (server, host) = match address(base)? {
        Address::Https => return Ok(Client::new(base)), // WHAT: an https:// address as it is. WHY: its own certificate authority checks it
        Address::Plain(server, host) => (server, host),
    };
    let plain = format!("http://{server}"); // WHAT: the address as the connection reads it. WHY: the same one the pin is kept under
    let damaged = book.all().is_err();
    if damaged {
        return Err(Trouble::Unreadable("this computer's record of the office server's certificate doesn't read. Ask whoever looks after the server for its fingerprint before connecting again.".into())); // WHAT: refused. WHY: a damaged book never means "no pins"
    }
    let shared_any = match also.shared.map(crate::office::read) {
        Some(Err(why)) => return Err(Trouble::Unreadable(format!("{why}. Ask whoever looks after the server before connecting again."))), // WHAT: refused. WHY: a damaged shared record never means "no pin"
        Some(Ok(found)) => found, // WHAT: the shared record, whichever address it names. WHY: compared by pin below
        None => None,
    };
    let shared = shared_any.clone().filter(|s| crate::office::server_of(&s.address).as_deref() == Some(server.as_str())); // WHAT: the shared record, if it names this address. WHY: its pin is this server's
    let kept = book.pin(&server).or_else(|| shared.as_ref().map(|s| s.pin.clone())); // WHAT: this app's pin, or the one another app on this PC kept. WHY: §2.3
    let held = book.holds(&server) || shared.is_some(); // WHAT: whether any pin is held for it. WHY: then plain HTTP is never fallen back to
    let (ips, local) = looked_up(&host); // WHAT: the host's addresses, looked up once. WHY: review: checked and connected to as one
    let health = Client::plain_at(&plain, ips.clone()).health().ok(); // WHAT: the plain health answer, from those addresses. WHY: only where HTTPS is is taken from it
    let office = health.as_ref().is_some_and(is_office_server); // WHAT: whether it says it is an Office Server. WHY: then it is never reached over plain HTTP
    let plain_allowed = || -> Result<Client, Trouble> {
        if office || held || !local || !also.allow_plain {
            return Err(if office || held { Trouble::Unreachable("this server's encrypted connection didn't answer, so this computer won't fall back to an unencrypted one. Check the server, or ask whoever looks after it.".into()) } else { unencrypted(&host, local) }); // WHAT: refused. WHY: review: plain HTTP only for an old, local server the person confirmed
        }
        Ok(Client::plain_at(&plain, ips.clone())) // WHAT: plain HTTP, to the checked addresses only. WHY: an old server the person knows is old
    };
    let Some(port) = health.as_ref().and_then(|h| h.secure_port) else {
        if health.is_none() && !held {
            return Err(Trouble::Unreachable("nothing answered at that address. The server may be off, or the address wrong.".into())); // WHAT: no answer. WHY: never a client to nowhere
        }
        return plain_allowed(); // WHAT: no HTTPS offered. WHY: refused unless an old, local, confirmed server
    };
    let secure = format!("https://{host}:{port}"); // WHAT: the same host, its HTTPS port. WHY: the server said where
    let client = match kept {
        Some(pin) => Client::pinned(&secure, &pin).and_then(|c| c.health().map(|_| c)).map_err(|e| changed_or(e, &secure, &pin))?, // WHAT: the kept pin, checked by answering. WHY: guard 5: a changed certificate is refused with words, never accepted
        None if held => return Err(Trouble::Unreadable("this computer's record of the server's certificate is damaged. Ask whoever looks after the server for its fingerprint before connecting again.".into())), // WHAT: a damaged entry. WHY: never treated as a first connection
        None => {
            if !local {
                return Err(Trouble::Unreadable(format!("{host} isn't on the office network, so this computer won't take its certificate on trust. Link this computer to the office server once from inside the office."))); // WHAT: refused. WHY: §2.2: a first contact pins only on a local address
            }
            let (agent, check) = crate::pinned::agent(None).map_err(Trouble::Unreadable)?; // WHAT: a first connection, no pin yet. WHY: the pin is kept from this handshake
            let answered = agent.resolver(crate::client::fixed(ips.clone())).timeout_connect(std::time::Duration::from_secs(8)).build().get(&format!("{secure}{}/health", crate::API_ROOT)).call(); // WHAT: the health answer, over HTTPS, from the addresses checked as local. WHY: review: a name can't look up to the office network for the check and elsewhere for the connection
            match (answered, check.seen()) {
                (Ok(_), Some(pin)) => {
                    if let Some(other) = shared_any.as_ref().filter(|s| s.pin != pin && !also.replace_shared) {
                        return Err(other_server(&other.pin, &pin)); // WHAT: refused before anything is kept. WHY: review: another name for the server, or another server, never re-pins the computer quietly
                    }
                    book.keep(&server, &pin).map_err(|e| Trouble::Unreadable(format!("couldn't keep the server's certificate: {e}")))?; // WHAT: kept. WHY: every connection after must show this certificate
                    Client::pinned(&secure, &pin)?
                }
                _ => return plain_allowed(), // WHAT: HTTPS offered but not answering. WHY: refused unless an old, local, confirmed server; an Office Server never
            }
        }
    };
    let pin = client.pin().unwrap_or_default().to_string(); // WHAT: the pin this connection is held to. WHY: compared with the certificate, and shared
    let verified = match client.office_identity() {
        Ok(Some(text)) => crate::office::check_certificate(&text, also.office_keys).is_some_and(|c| c.pin == pin), // WHAT: a valid certificate for this very pin. WHY: §2.4: then the server is verified
        _ => false, // WHAT: none, or none that reads. WHY: unverified
    };
    let same_server = shared_any.as_ref().filter(|s| s.pin == pin); // WHAT: the shared record, when it is this very server, under whatever name. WHY: compared by pin (review)
    let was_verified = same_server.is_some_and(|s| s.verified); // WHAT: whether this server was ever seen verified on this PC. WHY: guard 6, whatever name it was reached by
    if was_verified && !verified {
        return Err(Trouble::Refused(crate::model::Problem::new(
            "office_unverified",
            "The office server's certificate was checked before, and it isn't there or doesn't check out now. That happens when someone is pretending to be the server. Ask whoever looks after it before connecting.",
        ))); // WHAT: refused. WHY: guard 6: once verified, never accepted unverified
    }
    if let Some(path) = also.shared {
        let record = match same_server {
            Some(known) => crate::office::Shared { verified: known.verified || verified, ..known.clone() }, // WHAT: the record as the other apps wrote it, verified kept or gained. WHY: review: never cleared, never re-addressed by an alias
            None => crate::office::Shared { address: secure.clone(), pin: pin.clone(), verified }, // WHAT: a new record, for a first server or one the person confirmed. WHY: the next app on this PC reads it
        };
        if shared_any.as_ref() != Some(&record) && (shared_any.is_none() || same_server.is_some() || also.replace_shared) {
            let _ = crate::office::write(path, &record); // WHAT: kept for the other apps. WHY: §2.3; another server's record is replaced only when confirmed
        }
    }
    Ok(client)
}

/// A failed pinned connection, in words a person can act on.
fn changed_or(e: Trouble, secure: &str, pin: &str) -> Trouble {
    let seen = crate::pinned::agent(Some(pin)).ok().and_then(|(agent, check)| {
        let _ = agent.timeout_connect(std::time::Duration::from_secs(8)).build().get(&format!("{secure}{}/health", crate::API_ROOT)).call(); // WHAT: one more look, to see which certificate came. WHY: tells a changed certificate apart from a server that's off
        check.seen()
    });
    match seen {
        Some(other) if other != pin => Trouble::Refused(crate::model::Problem::new(
            "certificate_changed",
            format!(
                "The office server is showing a different certificate ({}) from the one this computer knows ({}). That happens when the server is reinstalled, or when someone is in between. Ask whoever looks after the server before connecting.",
                crate::pinned::short(&other),
                crate::pinned::short(pin)
            ),
        )), // WHAT: a changed certificate, both short pins shown. WHY: a person compares them with the server's own; never connected
        _ => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(base: &str) -> Option<(String, String)> {
        match address(base) {
            Ok(Address::Plain(s, h)) => Some((s, h)),
            _ => None,
        }
    }

    #[test]
    fn one_server_is_one_key_however_its_address_is_written() {
        assert_eq!(plain("http://192.168.20.3:8714"), Some(("192.168.20.3:8714".into(), "192.168.20.3".into())));
        for same in ["http://office:8714", "HTTP://Office:8714/", "http://office.:8714", " http://OFFICE:8714/api "] {
            assert_eq!(plain(same).map(|p| p.0), Some("office:8714".into()), "{same}");
        }
        assert_eq!(plain("http://office"), plain("http://office:80"), "no port is port 80");
        assert_eq!(plain("http://[FE80::1]:8714"), Some(("[fe80::1]:8714".into(), "[fe80::1]".into())));
        assert!(matches!(address("https://drawings.example.com"), Ok(Address::Https)), "a tunnel's address is checked by its authority");
        assert!(matches!(address("HTTPS://drawings.example.com"), Ok(Address::Https)));
        for odd in ["http://user@evil:8714", "http://a:b@office", "ftp://office", "office:8714", "", "http://", "javascript:alert(1)"] {
            assert!(address(odd).is_err(), "{odd}");
        }
    }

    fn a_book() -> PinBook {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(); // WHAT: a fresh name. WHY: tests run side by side
        PinBook::at(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("pins-{n}.json")))
    }

    #[test]
    fn a_damaged_pin_is_held_but_never_used() {
        let book = a_book();
        assert!(!book.holds("a:1"));
        book.keep("a:1", "not a pin").unwrap();
        assert!(book.holds("a:1"), "held: so plain HTTP is never fallen back to");
        assert_eq!(book.pin("a:1"), None, "but never used as a pin");
        book.keep("b:2", &"c".repeat(64)).unwrap();
        assert_eq!(book.pin("b:2").as_deref(), Some("c".repeat(64).as_str()));
        assert!(book.holds("a:1"), "keeping one never loses another");
    }

    #[test]
    fn a_book_that_does_not_read_holds_every_server_and_is_never_written_over() {
        let book = a_book();
        book.keep("a:1", &"c".repeat(64)).unwrap();
        std::fs::write(&book.path, b"{\"a:1\": 7, trunc").unwrap(); // WHAT: a damaged book. WHY: a cut-off write, or a disk error
        assert!(book.holds("a:1") && book.holds("never-seen:1"), "every server counts as pinned");
        assert!(is_pinned("http://never-seen:1", &book));
        assert!(book.keep("b:2", &"d".repeat(64)).is_err(), "nothing written over it");
        assert!(open("http://never-seen:1", &book).is_err(), "and no connection falls back to plain HTTP");
    }

    #[test]
    fn a_damaged_shared_record_never_means_no_pin() {
        let book = a_book();
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let shared = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("shared-{n}.json"));
        std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
        std::fs::write(&shared, b"{trunc").unwrap();
        assert!(open_with("http://127.0.0.1:1", &book, Also { shared: Some(&shared), office_keys: &[], ..Also::default() }).is_err());
        let _ = std::fs::remove_file(&shared);
    }

    /// A plain-HTTP server on this machine that answers every request with
    /// `body` (or `status` and `extra` headers), and keeps what it was sent.
    fn plain_server(status: &'static str, extra: String, body: String) -> (u16, std::sync::Arc<std::sync::Mutex<String>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let heard = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let log = heard.clone();
        std::thread::spawn(move || {
            for mut tcp in listener.incoming().flatten() {
                let _ = tcp.set_read_timeout(Some(std::time::Duration::from_millis(300))); // WHAT: a short wait for more. WHY: the body can come in a second packet
                let mut buf = vec![0u8; 65536];
                let mut all = String::new(); // WHAT: this connection's request. WHY: measured on its own
                while let Ok(n) = tcp.read(&mut buf) {
                    if n == 0 {
                        break; // WHAT: the caller stopped sending. WHY: all of it heard
                    }
                    all.push_str(&String::from_utf8_lossy(&buf[..n])); // WHAT: this request so far. WHY: its length is checked
                    log.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n])); // WHAT: kept. WHY: the test reads what was sent
                    if let Some(head) = all.find("\r\n\r\n") {
                        let length = all[..head].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        if all.len() >= head + 4 + length {
                            break; // WHAT: the whole request. WHY: answered once read
                        }
                    }
                }
                let _ = write!(tcp, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        (port, heard)
    }

    fn health(version: &str, product: &str, secure_port: Option<u16>) -> String {
        let port = secure_port.map_or("null".to_string(), |p| p.to_string());
        format!("{{\"ok\":true,\"api_version\":{},\"version\":\"{version}\",\"name\":\"x\",\"claimed\":true,\"joining\":\"code\",\"product\":\"{product}\",\"secure_port\":{port}}}", crate::API_VERSION)
    }

    #[test]
    fn review_an_office_server_offering_no_https_never_gets_a_password_in_the_clear() {
        let (port, heard) = plain_server("200 OK", String::new(), health("0.7.0", "excalibur-office", None)); // WHAT: an Office Server's answer with its HTTPS port taken out. WHY: the attack
        let refused = open_with(&format!("http://127.0.0.1:{port}"), &a_book(), Also { allow_plain: true, ..Also::default() }); // WHAT: even with plain allowed. WHY: an Office Server never
        assert!(refused.is_err(), "never plain to an Office Server");
        let (gone, _) = plain_server("200 OK", String::new(), health("0.7.0", "excalibur-office", Some(1))); // WHAT: HTTPS offered on a port where nothing answers. WHY: the other way the old code fell back
        assert!(open_with(&format!("http://127.0.0.1:{gone}"), &a_book(), Also { allow_plain: true, ..Also::default() }).is_err(), "a first contact whose HTTPS fails never falls back");
        assert!(!heard.lock().unwrap().contains("hunter2"), "no password was ever sent");
    }

    #[test]
    fn review_an_old_plain_server_needs_the_person_to_confirm_before_anything_is_sent() {
        let (port, heard) = plain_server("200 OK", String::new(), health("0.6.31", "hyperview", None)); // WHAT: an old View server, plain only. WHY: the one plain path kept
        let base = format!("http://127.0.0.1:{port}");
        match open_with(&base, &a_book(), Also::default()) {
            Err(Trouble::Refused(p)) => assert_eq!(p.error, CONFIRM_UNENCRYPTED, "{}", p.message),
            other => panic!("not asked to confirm: {:?}", other.map(|c| c.base().to_string())),
        }
        let mut client = open_with(&base, &a_book(), Also { allow_plain: true, ..Also::default() }).expect("confirmed, it connects");
        assert!(client.pin().is_none());
        let _ = client.sign_in("dana@example.com", "hunter2-confirmed");
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(heard.lock().unwrap().contains("hunter2-confirmed"), "only once the person confirmed");
    }

    #[test]
    fn review_a_redirect_is_never_followed() {
        let (elsewhere, seen_there) = plain_server("200 OK", String::new(), "{}".into()); // WHAT: another host. WHY: the redirect's target
        let (port, _) = plain_server("302 Found", format!("Location: http://127.0.0.1:{elsewhere}/api/v1/health\r\n"), String::new()); // WHAT: a server that sends callers on. WHY: the attack
        assert!(Client::new(&format!("http://127.0.0.1:{port}")).health().is_err(), "the redirect is an answer, never followed");
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(seen_there.lock().unwrap().is_empty(), "nothing reached the other host");
    }

    #[test]
    fn review_a_plain_client_connects_only_to_the_addresses_checked() {
        let (port, heard) = plain_server("200 OK", String::new(), health("0.6.31", "hyperview", None));
        let client = Client::plain_at(&format!("http://no-such-host.invalid:{port}"), vec!["127.0.0.1".parse().unwrap()]); // WHAT: a name that looks up to nothing, held to 127.0.0.1. WHY: the addresses checked are the ones connected to
        assert!(client.health().is_ok(), "reached the checked address, never a fresh look-up");
        assert!(!heard.lock().unwrap().is_empty());
        let (_, local) = looked_up("192.0.2.1");
        assert!(!local, "a public address is never local");
    }
}
