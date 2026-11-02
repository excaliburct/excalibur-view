//! Change notices: the office server tells another program when a drawing
//! set's takeoff changes, instead of that program asking over and over.
//!
//! An administrator gives an address (FabWire's, say) under Studio, or with
//! `POST /notices`, and is shown a secret, once. Each notice is a small JSON
//! body saying which set changed, in which project, under which of the other
//! program's references, and at what revision. It is signed with that secret
//! (`X-Excalibur-Signature: sha256=<hex HMAC-SHA256 of the body>`) so the
//! other program can tell it came from this server. No drawing, markup or
//! takeoff travels with it: the program asks the API for what it wants.
//!
//! Several changes to one set close together are one notice, sent a couple
//! of seconds after the first. One that doesn't get through is tried three
//! times, a little further apart each time, and then the attempt is recorded
//! and dropped; the other program can always catch up by asking. A sealed
//! server sends them only to addresses inside the company's network.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::params;
use sha2::{Digest, Sha256};

use crate::api::Shared;

/// Changes waiting to be told about, as (notice, set), so a burst of pushes
/// to one set goes out as one notice.
#[derive(Default)]
pub struct Pending(Mutex<HashSet<(String, String)>>);

/// How long a notice waits for more changes to the same set before it goes.
pub const GATHER: Duration = Duration::from_secs(2);

/// How long to wait before each try after the first.
const AGAIN_AFTER: [Duration; 2] = [Duration::from_secs(5), Duration::from_secs(30)];

/// An address a notice may be sent to: http or https, and not absurdly long.
pub fn acceptable(url: &str) -> Result<(), String> {
    let url = url.trim();
    let scheme_ok = url.starts_with("https://") || url.starts_with("http://");
    if !scheme_ok || url.len() > 2000 || url.contains(char::is_whitespace) {
        return Err("A notice address is a web address, starting http:// or https://.".into());
    }
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or_default(); // WHAT: what follows the scheme. WHY: the host is in it.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default(); // WHAT: host and port. WHY: checked below.
    if authority.contains('@') {
        return Err("A notice address carries no name or password before its host.".into()); // WHAT: user:password@ refused. WHY: a secret has no place in an address kept and shown in the Studio, and the form hides where it really goes.
    }
    let host = authority.trim_start_matches('[').split(']').next().unwrap_or_default(); // WHAT: an IPv6 host without its brackets. WHY: parsed as an address below.
    let host = if authority.starts_with('[') { host } else { authority.rsplit_once(':').map(|(h, _)| h).unwrap_or(authority) }; // WHAT: the host without its port. WHY: the same.
    let read_as = match url::Url::parse(url).ok().and_then(|u| u.host().map(|h| h.to_owned())) {
        Some(url::Host::Ipv4(v4)) => Some(std::net::IpAddr::V4(v4)), // WHAT: the host as the address library reads it. WHY: `2852039166`, `0xa9fea9fe` and `169.254.43518` are all 169.254.169.254 to it, and to the connection.
        Some(url::Host::Ipv6(v6)) => Some(std::net::IpAddr::V6(v6)), // WHAT: likewise for IPv6. WHY: the same.
        _ => host.parse::<std::net::IpAddr>().ok(), // WHAT: a name, or nothing the library could read. WHY: a name is checked again when it is looked up (`only_allowed`).
    };
    if read_as.is_some_and(never_a_notice) {
        return Err("That address is never another program's: link-local and 'any' addresses, where a cloud machine keeps its own secrets, are refused.".into()); // WHAT: refused. WHY: an administrator's session, or a stolen one, must not point the server at its own cloud metadata.
    }
    hub::web::outbound("Sending change notices", url)
}

/// Looks a notice's host up and keeps only the addresses a notice may go
/// to. Checking the address as typed was not enough: a name can be made to
/// look up to 169.254.169.254, and can change what it looks up to between the
/// check and the send. The connection is made only to what this returns.
fn only_allowed(netloc: &str) -> std::io::Result<Vec<std::net::SocketAddr>> {
    use std::net::ToSocketAddrs; // WHAT: the system's look-up. WHY: what the connection would use anyway.
    let found: Vec<std::net::SocketAddr> = netloc.to_socket_addrs()?.filter(|at| !never_a_notice(at.ip())).collect(); // WHAT: every address it looks up to, less the refused ones. WHY: only those are connected to.
    if found.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "that address is never another program's (link-local or 'any')")); // WHAT: nothing left. WHY: refused, never sent.
    }
    Ok(found) // WHAT: only the allowed addresses. WHY: the connection is made to nothing else.
}

/// Addresses a notice is never sent to: link-local (169.254.0.0/16, where a
/// cloud machine answers with its own credentials, and fe80::/10), the
/// "any" address, multicast and broadcast. The office's own network and this
/// machine stay allowed: that is where FabWire lives.
fn never_a_notice(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_link_local() || v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast(), // WHAT: the IPv4 ones. WHY: as above.
        std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => never_a_notice(std::net::IpAddr::V4(v4)), // WHAT: an IPv4 address written as IPv6. WHY: judged as itself.
            None => v6.is_unspecified() || v6.is_multicast() || (v6.segments()[0] & 0xffc0) == 0xfe80, // WHAT: the IPv6 ones. WHY: as above.
        },
    }
}

/// A new secret: 32 random bytes, as hex.
pub fn secret() -> String {
    let mut bytes = [0u8; 32];
    rand::Rng::fill(&mut rand::thread_rng(), &mut bytes);
    crate::store::hex(&bytes)
}

/// HMAC-SHA256, the way every webhook receiver already knows how to check.
pub fn sign(secret: &str, body: &[u8]) -> String {
    const BLOCK: usize = 64;
    let mut key = secret.as_bytes().to_vec();
    if key.len() > BLOCK {
        key = Sha256::digest(&key).to_vec();
    }
    key.resize(BLOCK, 0);
    let pad = |with: u8| key.iter().map(|k| k ^ with).collect::<Vec<u8>>();
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(body).finalize();
    let outer = Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize();
    crate::store::hex(&outer)
}

/// A drawing set's markups changed. Every notice address hears about it,
/// once, a moment from now.
pub fn changed(server: &Shared, set: &str) {
    let targets: Vec<(String, String, String)> = server
        .store
        .with(|db| {
            let mut q = db.prepare("SELECT id, url, secret FROM notices")?;
            let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(rows.filter_map(Result::ok).collect())
        })
        .unwrap_or_default();
    for (id, url, secret) in targets {
        let key = (id.clone(), set.to_string());
        let fresh = server.notices.0.lock().map(|mut p| p.insert(key.clone())).unwrap_or(false);
        if !fresh {
            continue;
        }
        let server = server.clone();
        let set = set.to_string();
        let _ = std::thread::Builder::new().name("change notice".into()).spawn(move || {
            std::thread::sleep(GATHER);
            if let Ok(mut pending) = server.notices.0.lock() {
                pending.remove(&key);
            }
            let Some(body) = body_for(&server, &set) else { return };
            let status = deliver(&url, &secret, "markups.changed", &body);
            record(&server, &id, &status);
        });
    }
}

/// Sends a notice straight away, to see whether the other program is
/// listening. Returns how it went.
pub fn test(server: &Shared, id: &str) -> Result<String, String> {
    let found: Option<(String, String)> = server
        .store
        .with(|db| {
            use rusqlite::OptionalExtension;
            Ok(db
                .query_row("SELECT url, secret FROM notices WHERE id = ?1", params![id], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?)
        })
        .map_err(|e| e.to_string())?;
    let (url, secret) = found.ok_or("There is no notice address with that id.")?;
    let body = serde_json::json!({
        "event": "ping",
        "server": server.config.name,
        "at": crate::api::now(),
    });
    let status = deliver(&url, &secret, "ping", &body.to_string());
    record(server, id, &status);
    Ok(status)
}

/// What a notice says about a set: enough to know what to ask for.
fn body_for(server: &Shared, set: &str) -> Option<String> {
    let found = server
        .store
        .with(|db| {
            use rusqlite::OptionalExtension;
            Ok(db
                .query_row(
                    "SELECT s.id, s.name, s.revision, p.id, p.number, p.reference
                     FROM sets s LEFT JOIN projects p ON p.id = s.project
                     WHERE s.id = ?1",
                    params![set],
                    |r| {
                        Ok(serde_json::json!({
                            "event": "markups.changed",
                            "set": r.get::<_, String>(0)?,
                            "file": r.get::<_, String>(1)?,
                            "revision": r.get::<_, i64>(2)?,
                            "project": r.get::<_, Option<String>>(3)?,
                            "number": r.get::<_, Option<String>>(4)?,
                            "reference": r.get::<_, Option<String>>(5)?,
                        }))
                    },
                )
                .optional()?)
        })
        .ok()??;
    let mut body = found;
    body["server"] = serde_json::json!(server.config.name);
    body["at"] = serde_json::json!(crate::api::now());
    Some(body.to_string())
}

/// Sends one notice, trying again twice. `delivered`, or what went wrong.
fn deliver(url: &str, secret: &str, event: &str, body: &str) -> String {
    if let Err(why) = hub::web::outbound("Sending change notices", url) {
        return why;
    }
    let agent = hub::web::agent().timeout(Duration::from_secs(15)).redirects(0).resolver(only_allowed).build(); // WHAT: no redirects followed, and only allowed addresses connected to. WHY: a notice goes to the address an administrator gave, never to one an answer names or one a name was made to look up to.
    let signature = format!("sha256={}", sign(secret, body.as_bytes()));
    let mut last = String::new();
    for attempt in 0..=AGAIN_AFTER.len() {
        if attempt > 0 {
            std::thread::sleep(AGAIN_AFTER[attempt - 1]);
        }
        let sent = agent
            .post(url)
            .set("Content-Type", "application/json")
            .set("User-Agent", concat!("excalibur-view-server/", env!("CARGO_PKG_VERSION")))
            .set("X-Excalibur-Event", event)
            .set("X-Excalibur-Signature", &signature)
            .send_string(body);
        match sent {
            Ok(_) => return "delivered".into(),
            // Told no by the other program: trying again won't change its mind.
            Err(hub::web::ureq::Error::Status(code, _)) if (400..500).contains(&code) => {
                return format!("refused with {code}");
            }
            Err(e) => last = e.to_string(),
        }
    }
    format!("not delivered: {last}")
}

fn record(server: &Shared, id: &str, status: &str) {
    let now = crate::api::now();
    let _ = server.store.with(|db| {
        db.execute(
            "UPDATE notices SET last_status = ?1, last_at = ?2 WHERE id = ?3",
            params![status, now, id],
        )?;
        Ok(())
    });
    if status != "delivered" {
        tracing::warn!("change notice {id}: {status}");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn signatures_match_the_published_hmac_test_vector() {
        // RFC 4231, test case 2.
        assert_eq!(
            super::sign("Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn only_a_web_address_is_taken() {
        assert!(super::acceptable("https://fabwire.example.lan/hooks/excalibur").is_ok());
        assert!(super::acceptable("http://10.0.0.7:8080/notice").is_ok());
        assert!(super::acceptable("ftp://example.com").is_err());
        assert!(super::acceptable("file:///etc/passwd").is_err());
        assert!(super::acceptable("https://example.com/a b").is_err());
    }

    #[test]
    fn a_notice_is_never_sent_where_a_cloud_machine_keeps_its_secrets() {
        for refused in [
            "http://169.254.169.254/latest/meta-data/", // WHAT: the cloud metadata address. WHY: it answers with the machine's own credentials.
            "http://169.254.169.254:80/x",
            "http://[fe80::1]/x",
            "http://[::ffff:169.254.169.254]/x",
            "http://0.0.0.0:8080/x",
            "https://user:secret@fabwire.example.lan/hooks",
        ] {
            assert!(super::acceptable(refused).is_err(), "{refused} was taken");
        }
        for refused in ["http://2852039166/latest/meta-data/", "http://0xa9fea9fe/x", "http://169.254.43518/x", "http://0251.0376.0251.0376/x", "http://0/x"] {
            assert!(super::acceptable(refused).is_err(), "{refused} was taken: it is 169.254.169.254 or 0.0.0.0 written another way"); // WHAT: the metadata address written as the address library and the connection read it. WHY: checked only as typed, these went straight past.
        }
        for refused in ["169.254.169.254:80", "[fe80::1]:80", "0.0.0.0:80"] {
            assert!(super::only_allowed(refused).is_err(), "{refused} would be connected to"); // WHAT: what a name looked up to, refused at the connection. WHY: a name can be made to look up to anything, and to change between the check and the send.
        }
        assert!(super::only_allowed("127.0.0.1:5000").is_ok(), "this machine is still allowed"); // WHAT: FabWire on the server itself. WHY: where notices go.
        for allowed in ["http://10.0.0.7:8080/notice", "http://127.0.0.1:5000/hook", "https://[2001:db8::1]/x", "https://fabwire.example.lan/hooks/excalibur"] {
            assert!(super::acceptable(allowed).is_ok(), "{allowed} was refused"); // WHAT: the office's own network and this machine. WHY: where the programs a notice is for live.
        }
    }
}
