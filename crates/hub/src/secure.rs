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

/// The client a seat should use for `base`: the server's HTTPS by its kept pin
/// (kept now, on a first connection), or `base` itself when the server offers
/// no HTTPS and this seat has never held a pin for it.
///
/// # Errors
///
/// Refused, in words a person can act on, when it can't be done.
pub fn open(base: &str, book: &PinBook) -> Result<Client, Trouble> {
    let (server, host) = match address(base)? {
        Address::Https => return Ok(Client::new(base)), // WHAT: an https:// address as it is. WHY: its own certificate authority checks it
        Address::Plain(server, host) => (server, host),
    };
    let plain = format!("http://{server}"); // WHAT: the address as the connection reads it. WHY: the same one the pin is kept under
    let damaged = book.all().is_err();
    if damaged {
        return Err(Trouble::Unreadable("this computer's record of the office server's certificate doesn't read. Ask whoever looks after the server for its fingerprint before connecting again.".into())); // WHAT: refused. WHY: a damaged book never means "no pins"
    }
    let kept = book.pin(&server);
    let held = book.holds(&server);
    let offered = Client::new(&plain).health().ok().and_then(|h| h.secure_port); // WHAT: where the server says HTTPS is. WHY: only the port is taken from plain HTTP, never the pin
    let port = match (offered, held) {
        (Some(port), _) => port,
        (None, false) => return Ok(Client::new(&plain)), // WHAT: an older server, never pinned. WHY: reached as before
        (None, true) => {
            return Err(Trouble::Unreachable("this server's encrypted connection didn't answer. It was encrypted before, so this computer won't fall back to an unencrypted one. Check the server, or ask whoever looks after it.".into())); // WHAT: refused. WHY: blocking HTTPS must never push a seat back to plain HTTP
        }
    };
    let secure = format!("https://{host}:{port}"); // WHAT: the same host, its HTTPS port. WHY: the server said where
    match kept {
        Some(pin) => Client::pinned(&secure, &pin).and_then(|c| c.health().map(|_| c)).map_err(|e| changed_or(e, &secure, &pin)), // WHAT: the kept pin, checked by answering. WHY: a changed certificate is refused with words, not a stack of TLS errors
        None if held => Err(Trouble::Unreadable("this computer's record of the server's certificate is damaged. Ask whoever looks after the server for its fingerprint before connecting again.".into())), // WHAT: a damaged entry. WHY: never treated as a first connection
        None => {
            let (agent, check) = crate::pinned::agent(None).map_err(Trouble::Unreadable)?; // WHAT: a first connection, no pin yet. WHY: the pin is kept from this handshake
            let answered = agent.timeout_connect(std::time::Duration::from_secs(8)).build().get(&format!("{secure}{}/health", crate::API_ROOT)).call(); // WHAT: the health answer, over HTTPS. WHY: proves the server answers there before its pin is kept
            let seen = check.seen();
            match (answered, seen) {
                (Ok(_), Some(pin)) => {
                    book.keep(&server, &pin).map_err(|e| Trouble::Unreadable(format!("couldn't keep the server's certificate: {e}")))?; // WHAT: kept. WHY: every connection after must show this certificate
                    Client::pinned(&secure, &pin)
                }
                _ => Ok(Client::new(&plain)), // WHAT: HTTPS offered but not answering, never pinned. WHY: a first connection may still use plain HTTP; once pinned it never will
            }
        }
    }
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
}
