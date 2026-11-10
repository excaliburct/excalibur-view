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
//! plain HTTP: only from the HTTPS handshake.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::client::{Client, Trouble};

/// The pins this seat keeps, one per office server, in a small file.
pub struct PinBook {
    path: PathBuf, // WHAT: the file. WHY: kept between runs, in the seat's own settings folder
}

impl PinBook {
    pub fn at(path: PathBuf) -> PinBook {
        PinBook { path }
    }

    fn all(&self) -> BTreeMap<String, String> {
        std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default() // WHAT: every kept pin. WHY: a missing or unreadable file is no pins, never a crash
    }

    /// The pin kept for `server`, if any.
    pub fn pin(&self, server: &str) -> Option<String> {
        self.all().remove(server).filter(|p| crate::pinned::is_pin(p)) // WHAT: the pin, only if it reads as one. WHY: a damaged entry is never used as a pin (and `holds` still sees it)
    }

    /// Whether anything at all is kept for `server`, readable or not.
    fn holds(&self, server: &str) -> bool {
        self.all().contains_key(server) // WHAT: an entry, even a damaged one. WHY: a damaged pin still forbids falling back to plain HTTP
    }

    fn keep(&self, server: &str, pin: &str) -> std::io::Result<()> {
        let mut all = self.all();
        all.insert(server.to_string(), pin.to_string()); // WHAT: this server's pin. WHY: kept from its first HTTPS answer
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let part = self.path.with_extension("part");
        std::fs::write(&part, serde_json::to_vec_pretty(&all).unwrap_or_default())?; // WHAT: written beside. WHY: a crash never leaves half the book
        std::fs::rename(&part, &self.path) // WHAT: moved into place. WHY: all at once
    }
}

/// `http://host:port` → (`host:port`, `host`). None for anything else.
fn plain_address(base: &str) -> Option<(String, String)> {
    let rest = base.trim().strip_prefix("http://")?; // WHAT: plain HTTP only. WHY: an https:// address (a tunnel) is already checked by its certificate authority
    let authority = rest.split(['/', '?', '#']).next()?.to_ascii_lowercase(); // WHAT: host and port. WHY: the pin is per server
    if authority.is_empty() || authority.contains('@') {
        return None; // WHAT: no host, or a user name in it. WHY: never guessed at
    }
    let host = if authority.starts_with('[') {
        authority.split_inclusive(']').next()?.to_string() // WHAT: an IPv6 address in brackets. WHY: its colons aren't the port's
    } else {
        authority.split(':').next()?.to_string()
    };
    Some((authority, host))
}

/// The client a seat should use for `base`: the server's HTTPS by its kept pin
/// (kept now, on a first connection), or `base` itself when the server offers
/// no HTTPS and this seat has never held a pin for it.
pub fn open(base: &str, book: &PinBook) -> Result<Client, Trouble> {
    let Some((server, host)) = plain_address(base) else {
        return Ok(Client::new(base)); // WHAT: an https:// address as it is. WHY: its own certificate authority checks it
    };
    let kept = book.pin(&server);
    let held = book.holds(&server);
    let offered = Client::new(base).health().ok().and_then(|h| h.secure_port); // WHAT: where the server says HTTPS is. WHY: only the port is taken from plain HTTP, never the pin
    let port = match (offered, held) {
        (Some(port), _) => port,
        (None, false) => return Ok(Client::new(base)), // WHAT: an older server, never pinned. WHY: reached as before
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
                _ => Ok(Client::new(base)), // WHAT: HTTPS offered but not answering, never pinned. WHY: a first connection may still use plain HTTP; once pinned it never will
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

    #[test]
    fn only_a_plain_http_address_is_moved() {
        assert_eq!(plain_address("http://192.168.20.3:8714"), Some(("192.168.20.3:8714".into(), "192.168.20.3".into())));
        assert_eq!(plain_address("http://Office-Server:8714/"), Some(("office-server:8714".into(), "office-server".into())));
        assert_eq!(plain_address("http://[fe80::1]:8714"), Some(("[fe80::1]:8714".into(), "[fe80::1]".into())));
        assert_eq!(plain_address("https://drawings.example.com"), None, "a tunnel's address is checked by its authority");
        assert_eq!(plain_address("http://user@evil:8714"), None);
        assert_eq!(plain_address("http://"), None);
    }

    #[test]
    fn a_damaged_pin_is_held_but_never_used() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(); // WHAT: a fresh name. WHY: tests run side by side
        let book = PinBook::at(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("pins-{n}.json")));
        assert!(!book.holds("a:1"));
        book.keep("a:1", "not a pin").unwrap();
        assert!(book.holds("a:1"), "held: so plain HTTP is never fallen back to");
        assert_eq!(book.pin("a:1"), None, "but never used as a pin");
        book.keep("b:2", &"c".repeat(64)).unwrap();
        assert_eq!(book.pin("b:2").as_deref(), Some("c".repeat(64).as_str()));
        assert!(book.holds("a:1"), "keeping one never loses another");
    }
}
