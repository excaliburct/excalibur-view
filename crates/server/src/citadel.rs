//! The Office Server's `citadel` part: Citadel Server, run alongside and
//! watched, never opened.
//!
//! Citadel keeps an office's estimating jobs sealed: the bids are sealed on
//! the office's own computers, and Citadel Server only ever holds sealed bytes,
//! under its own Windows service account and its own locked folders. The Office
//! Server keeps it that way. It never reads Citadel's folders or its data; all
//! it asks is the one question anybody may ask Citadel Server, "what version
//! are you?", on this machine, over Citadel's own HTTPS, and by Citadel's
//! certificate pin (`hub::pinned`).
//!
//! The pin is kept from the first answer that reads as Citadel Server's. After
//! that, a server on Citadel's port with another certificate is reported as
//! "certificate-changed" and never believed: a reinstall a person must confirm,
//! or something pretending to be Citadel.

use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;

use crate::api::{Server, Shared};

/// The port Citadel Server listens on unless its settings say otherwise
/// (`Wire.DefaultPort` in Citadel).
pub const PORT: u16 = 47110; // WHAT: Citadel's own default. WHY: the Office Server never reads Citadel's settings to find another

/// Where the pin of Citadel Server's certificate is kept.
pub const PIN: &str = "citadel_pin"; // WHAT: the setting's name. WHY: kept with the server's other settings

/// How often Citadel is looked at.
pub const LOOK_EVERY: Duration = Duration::from_secs(300); // WHAT: every five minutes. WHY: soon enough for the board, never a load on Citadel

/// The most of an answer read. A version is a few dozen bytes.
const LARGEST: u64 = 4 * 1024; // WHAT: 4 KiB. WHY: a server pointed at something enormous doesn't fill memory finding out

/// What was seen when Citadel was last looked at.
#[derive(Clone, Debug, PartialEq)]
pub enum Seen {
    Running { version: String }, // WHAT: answered, by its pin. WHY: the part is running
    NotFound, // WHAT: nothing listening on Citadel's port. WHY: not installed here, or stopped
    NotAnswering, // WHAT: something there, but no good answer. WHY: starting, stuck, or not Citadel
    CertificateChanged { seen: String }, // WHAT: another certificate than the pinned one. WHY: never believed; a person decides
}

impl Seen {
    /// The part's state, as the Manager and Fleet are told it.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn state(&self) -> &'static str {
        match self {
            Seen::Running { .. } => "running",
            Seen::NotFound => "not-found",
            Seen::NotAnswering => "not-answering",
            Seen::CertificateChanged { .. } => "certificate-changed",
        }
    }

    /// The version it runs, when it is running.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn version(&self) -> Option<String> {
        match self {
            Seen::Running { version } => Some(version.clone()),
            _ => None,
        }
    }
}

/// The last look, for the reports. One Office Server per process, so one.
static LAST: Mutex<Option<Seen>> = Mutex::new(None); // WHAT: what was seen last. WHY: the reports read it instead of asking Citadel on every request

/// What was seen last, or `None` before the first look.
pub fn last() -> Option<Seen> {
    LAST.lock().ok().and_then(|l| l.clone()) // WHAT: a copy. WHY: read by /office and Fleet's report
}

#[derive(Deserialize)]
struct Version {
    version: String, // WHAT: Citadel's running version. WHY: the one field read
}

/// Whether `v` reads as a version: digits and dots, a few dozen at most.
fn plain_version(v: &str) -> bool {
    !v.is_empty() && v.len() <= 32 && v.starts_with(|c: char| c.is_ascii_digit()) && v.chars().all(|c| c.is_ascii_digit() || c == '.') // WHAT: like 1.0.2. WHY: anything else isn't Citadel's answer, and is never shown
}

/// Reads an answer as Citadel Server's version, or `None`.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn version_in(body: &str) -> Option<String> {
    let read: Version = serde_json::from_str(body).ok()?; // WHAT: the JSON. WHY: Citadel answers {"version": ...}
    plain_version(&read.version).then_some(read.version) // WHAT: kept only when it looks like a version. WHY: it is shown on the board
}

/// Looks once at `address` with `pin` kept from before. The new pin to keep,
/// when this was the first good answer, comes back beside what was seen.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn look_at(address: &str, pin: Option<&str>) -> (Seen, Option<String>) {
    use std::io::Read;
    let Ok((agent, check)) = hub::pinned::agent(pin) else {
        return (Seen::NotAnswering, None); // WHAT: a kept pin that isn't one. WHY: never treated as no pin
    };
    let asked = agent
        .timeout_connect(Duration::from_secs(3))
        .timeout(Duration::from_secs(10))
        .redirects(0) // WHAT: no redirects. WHY: the answer comes from Citadel's port or not at all
        .build()
        .get(address)
        .call(); // WHAT: GET /api/version. WHY: the one question asked
    let seen = check.seen(); // WHAT: the certificate's pin, if the handshake got that far. WHY: kept or compared
    if let (Some(p), Some(s)) = (pin, seen.as_deref()) {
        if p != s {
            return (Seen::CertificateChanged { seen: s.to_string() }, None); // WHAT: another certificate, whatever else happened. WHY: reported, never believed
        }
    }
    let response = match asked {
        Ok(r) => r,
        Err(hub::web::ureq::Error::Transport(t)) if seen.is_none() && t.kind() == hub::web::ureq::ErrorKind::ConnectionFailed => return (Seen::NotFound, None), // WHAT: nobody on the port, before any certificate. WHY: not running here
        Err(_) => return (Seen::NotAnswering, None), // WHAT: something there, no good answer. WHY: starting, stuck, or not Citadel
    };
    let mut body = String::new();
    if response.into_reader().take(LARGEST).read_to_string(&mut body).is_err() {
        return (Seen::NotAnswering, None); // WHAT: an answer that doesn't read. WHY: not Citadel's
    }
    match version_in(&body) {
        Some(version) => {
            let keep = if pin.is_none() { seen } else { None }; // WHAT: the pin, on a first good answer only. WHY: kept once, never replaced by a later answer
            (Seen::Running { version }, keep)
        }
        None => (Seen::NotAnswering, None), // WHAT: something else on the port. WHY: its certificate is never kept
    }
}

/// Looks once at this machine's Citadel Server, keeps the pin on the first
/// good answer, and notes what was seen.
pub fn look(server: &Server) -> Seen {
    let pin = server.store.setting(PIN).ok().flatten().filter(|p| !p.is_empty()); // WHAT: the pin kept, if any. WHY: compared on every look
    let (seen, keep) = look_at(&format!("https://127.0.0.1:{PORT}/api/version"), pin.as_deref()); // WHAT: this machine only. WHY: Citadel runs alongside, never elsewhere
    if let Some(new) = keep {
        if server.store.set_setting(PIN, &new).is_ok() {
            tracing::info!("Citadel Server found on this machine; its certificate is pinned"); // WHAT: noted in the log. WHY: the moment the pin was kept
        }
    }
    if let Seen::CertificateChanged { seen } = &seen {
        tracing::warn!("something on Citadel's port shows another certificate ({}…); not believed", &seen[..16.min(seen.len())]); // WHAT: a warning with the start of the pin. WHY: a person compares it with Citadel Server's own
    }
    if let Ok(mut last) = LAST.lock() {
        *last = Some(seen.clone()); // WHAT: kept for the reports. WHY: they read it
    }
    seen
}

/// `POST /api/v1/admin/citadel/forget-pin`: forgets Citadel Server's kept
/// pin, so the next look keeps the certificate that answers then. For after
/// Citadel Server was reinstalled, or when something else answered on its
/// port first. Administrators signed in only; the log says who.
pub(crate) async fn forget_pin(axum::extract::State(server): axum::extract::State<Shared>, headers: axum::http::HeaderMap) -> crate::api::Answer<axum::Json<serde_json::Value>> {
    let who = crate::api::administrator(&server, &headers)?; // WHAT: an administrator signed in, never a key. WHY: re-pinning decides which server is believed
    if !crate::office::licensed(&server).contains(&hub::license::Module::Citadel) {
        return Err(crate::api::Denied::missing("Citadel part on this server")); // WHAT: 404 without the part. WHY: nothing is looked at, or pinned, on an office that doesn't run Citadel
    }
    server.store.set_setting(PIN, "").map_err(|e| crate::api::Denied::broke("forget Citadel's pin", e))?; // WHAT: no pin kept. WHY: the next look keeps the one that answers
    if let Ok(mut last) = LAST.lock() {
        *last = None; // WHAT: nothing seen yet. WHY: the parts list says "looking" until the next look
    }
    tracing::warn!("Citadel Server's certificate pin was forgotten by {}; the next look keeps the one that answers", who.email); // WHAT: in the log, by name. WHY: who chose to trust a new certificate
    let server = server.clone();
    let _ = tokio::task::spawn_blocking(move || look(&server)).await; // WHAT: looks now. WHY: the administrator sees the result at once
    Ok(axum::Json(serde_json::json!({ "state": last().map_or("looking", |s| s.state()) })))
}

/// Looks at Citadel every [`LOOK_EVERY`] while the licence switches it on.
pub(crate) async fn watch(server: Shared) {
    loop {
        if crate::office::licensed(&server).contains(&hub::license::Module::Citadel) {
            let looking = std::sync::Arc::clone(&server);
            let _ = tokio::task::spawn_blocking(move || look(&looking)).await; // WHAT: one look, off the async threads. WHY: it waits on the network
        }
        tokio::time::sleep(LOOK_EVERY).await; // WHAT: wait. WHY: never a busy loop
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_version_is_read_from_the_answer() {
        assert_eq!(version_in(r#"{"version":"1.0.2","wave":null}"#).as_deref(), Some("1.0.2"));
        for odd in [r#"{"version":""}"#, r#"{"version":"<script>"}"#, r#"{"version":"1.0.2; drop"}"#, "{}", "not json", r#"{"version":7}"#] {
            assert_eq!(version_in(odd), None, "{odd}");
        }
        assert_eq!(version_in(&format!(r#"{{"version":"{}"}}"#, "1".repeat(33))), None, "too long to be a version");
    }

    #[test]
    fn nothing_on_the_port_is_not_found() {
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); // WHAT: a port nobody listens on, once this closes. WHY: a Citadel that isn't there
        let port = free.local_addr().unwrap().port();
        drop(free);
        let (seen, keep) = look_at(&format!("https://127.0.0.1:{port}/api/version"), None);
        assert_eq!(seen, Seen::NotFound);
        assert_eq!(keep, None, "no pin is kept from nothing");
    }

    #[test]
    fn a_kept_pin_that_is_not_one_never_means_no_pin() {
        let (seen, keep) = look_at("https://127.0.0.1:1/api/version", Some("not a pin"));
        assert_eq!(seen, Seen::NotAnswering);
        assert_eq!(keep, None);
    }

    #[test]
    fn the_first_good_answer_pins_citadel_and_the_pin_holds_after() {
        use hub::pinned::testing::{impostor, serve_once};
        let (address, pin) = serve_once(21, r#"{"version":"1.0.2","wave":null}"#); // WHAT: a Citadel Server, as it answers. WHY: the first look
        let (seen, keep) = look_at(&address, None);
        assert_eq!(seen, Seen::Running { version: "1.0.2".into() });
        assert_eq!(keep.as_deref(), Some(pin.as_str()), "its pin is kept from the first good answer");

        let (again, _) = serve_once(21, r#"{"version":"1.0.3"}"#); // WHAT: the same server, updated. WHY: the pin still matches
        let (seen, keep) = look_at(&again, Some(&pin));
        assert_eq!(seen, Seen::Running { version: "1.0.3".into() });
        assert_eq!(keep, None, "a kept pin is never replaced");

        let (other, other_pin) = serve_once(22, r#"{"version":"9.9.9"}"#); // WHAT: something else on the port. WHY: a reinstall, or an impostor
        let (seen, keep) = look_at(&other, Some(&pin));
        assert_eq!(seen, Seen::CertificateChanged { seen: other_pin }, "never believed");
        assert_eq!(keep, None);

        let copied = impostor(21, 23); // WHAT: Citadel's certificate without its key. WHY: the pin matches; only the signature can refuse it
        let (seen, _) = look_at(&copied, Some(&pin));
        assert_eq!(seen, Seen::NotAnswering, "a copied certificate never answers as Citadel");
    }

    #[test]
    fn something_else_on_the_port_is_never_pinned() {
        let (address, _) = hub::pinned::testing::serve_once(24, "<html>not citadel</html>"); // WHAT: another program's answer. WHY: its certificate must not become Citadel's pin
        let (seen, keep) = look_at(&address, None);
        assert_eq!(seen, Seen::NotAnswering);
        assert_eq!(keep, None);
    }

    #[test]
    fn states_are_the_words_the_board_reads() {
        assert_eq!(Seen::Running { version: "1.0.2".into() }.state(), "running");
        assert_eq!(Seen::Running { version: "1.0.2".into() }.version().as_deref(), Some("1.0.2"));
        assert_eq!(Seen::NotFound.state(), "not-found");
        assert_eq!(Seen::NotAnswering.state(), "not-answering");
        assert_eq!(Seen::CertificateChanged { seen: "a".repeat(64) }.state(), "certificate-changed");
        assert_eq!(Seen::NotFound.version(), None);
    }
}
