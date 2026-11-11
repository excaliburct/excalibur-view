//! The office's device list: which computers may connect, for every app
//! (docs/OFFICE-ENROLLMENT.md §3 and §4).
//!
//! A device is one app on one computer for one Windows user, with its own
//! Ed25519 key (`hub::device`). It asks to be let in from the office network;
//! an admin device approves it. The first computer to ask, while the server
//! has no approved device and within a day of the service's first start, is
//! approved as an admin at once: that is whoever sets the office up.
//!
//! Every request after is signed by the device. The gate here checks the
//! signature, the time, a replay cache and that the device is approved,
//! before View's own sign-in checks run; each app keeps its own sign-in on
//! top. Any failure is the same answer, 401 `device_unknown`.
//!
//! Seats that were already signed in before this server knew about devices
//! are carried over: a seat that presents a session issued before the device
//! list began, still valid at that moment, is enrolled approved, and an
//! administrator's seat as an admin device.

use std::collections::HashMap;
use std::sync::Mutex;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use hub::device::{self as wire, Ask};
use hub::Problem;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;

use crate::api::{self, Answer, Denied, Server, Shared};
use crate::audit::Entry;

/// Where the device list's own settings are kept.
pub mod settings {
    /// When this server first ran with a device list (RFC 3339). Sessions
    /// issued before it may carry their seat over; none issued after can.
    pub const BEGAN: &str = "devices_began"; // WHAT: the migration cut-off. WHY: guard 8: only a session from before 0.7.0 migrates a seat
    /// Until when the first computer to ask is made the admin (RFC 3339).
    pub const CLAIM_UNTIL: &str = "device_claim_until"; // WHAT: the end of the 24-hour claiming window. WHY: §3; kept so a restart never reopens it
    /// When the Citadel cell carried its paired computers over (RFC 3339).
    pub const CITADEL_MIGRATED: &str = "citadel_migrated"; // WHAT: the cell's one migration. WHY: §6a says it is sent once; a second is refused
    /// The office certificate, as it was stored, served at
    /// `GET /api/v1/office/identity` (§2.4).
    pub const OFFICE_CERT: &str = "office_cert"; // WHAT: the setting's name. WHY: the contract names it
}

/// The `YubiKey` slot the office certificate will be signed from (§2.4). A
/// placeholder until that signing exists: nothing is signed with it yet.
pub const OFFICE_KEY_SLOT: &str = "office"; // WHAT: the slot's name. WHY: the later step that signs certificates fills it in; the apps' built-in keys are `OFFICE_KEYS` in trust.rs

/// How long after the service's first start the first computer may claim it.
pub const CLAIM_HOURS: i64 = 24; // WHAT: one day. WHY: §3; a stranger can't race the owner a week later

/// The most computers kept waiting at once.
pub const MOST_WAITING: i64 = 200; // WHAT: two hundred. WHY: a stranger on the office network can't fill the list without end; far more than any office has computers

/// The most a device request's body may be.
const ASK_BODY: usize = 16 * 1024; // WHAT: 16 KiB. WHY: a request is a few hundred bytes; read only this much of what a stranger sends

/// The device states, as stored and as the wire says them.
pub mod state {
    pub const WAITING: &str = "waiting"; // WHAT: asked, not yet answered. WHY: §3
    pub const APPROVED: &str = "approved"; // WHAT: let in. WHY: §3
    pub const REFUSED: &str = "refused"; // WHAT: turned down. WHY: §3
    pub const REMOVED: &str = "removed"; // WHAT: taken off the list; must ask again. WHY: §3
}

/// What the audit log calls each device action.
pub mod action {
    pub const ASKED: &str = "a computer asked to be let in"; // WHAT: a device request. WHY: §3: every call is written in the audit log
    pub const CLAIMED: &str = "the first computer was made the office's admin"; // WHAT: the first-admin claim. WHY: likewise
    pub const MIGRATED: &str = "a computer was carried over with its session"; // WHAT: a seat from before 0.7.0. WHY: likewise
    pub const JOINED: &str = "a computer was let in with a join code or invitation"; // WHAT: a join from outside. WHY: likewise
    pub const APPROVED: &str = "approved a computer"; // WHAT: an admin let one in. WHY: likewise
    pub const REFUSED: &str = "refused a computer"; // WHAT: an admin turned one down. WHY: likewise
    pub const REMOVED: &str = "removed a computer"; // WHAT: an admin took one off. WHY: likewise
    pub const MADE_ADMIN: &str = "made a computer an admin"; // WHAT: admin on. WHY: likewise
    pub const UNMADE_ADMIN: &str = "made a computer not an admin"; // WHAT: admin off. WHY: likewise
    pub const CARRIED_FROM_CITADEL: &str = "computers paired with Citadel Server were carried over"; // WHAT: the cell's migration. WHY: likewise
    pub const REFUSED_REQUEST: &str = "a computer's signed request was refused"; // WHAT: a failed signature check. WHY: written at most once a minute an address, so a stranger can't fill the log
    pub const REOPENED_CLAIM: &str = "claiming the office server was reopened"; // WHAT: the Manager's reopen. WHY: likewise
}

/// The SDDL-free description of one device, as the server keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: String, // WHAT: the device id. WHY: §3
    pub public_key: [u8; 32], // WHAT: its public key. WHY: every signature is checked with it
    pub state: String, // WHAT: waiting, approved, refused or removed. WHY: only approved passes the gate
    pub admin: bool, // WHAT: whether it may approve others. WHY: guard 4
    pub app: String, // WHAT: which app. WHY: shown, and told to the Citadel cell
    pub machine: String, // WHAT: the computer's name. WHY: likewise
    pub os_user: String, // WHAT: the Windows user. WHY: likewise
    pub display_name: String, // WHAT: the person's name. WHY: likewise
    pub approved_by: Option<String>, // WHAT: who approved it. WHY: /devices/me says it
    pub approved_at: Option<String>, // WHAT: when. WHY: likewise
}

impl Device {
    /// How the audit log and the list name this device: "Dana (DANA-LAPTOP)".
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn label(&self) -> String {
        label(&self.display_name, &self.os_user, &self.machine) // WHAT: the person, then the computer. WHY: what an admin recognises
    }
}

/// "Dana (DANA-LAPTOP)", or the Windows user when no name was given.
fn label(display_name: &str, os_user: &str, machine: &str) -> String {
    let who = if display_name.trim().is_empty() { os_user } else { display_name }; // WHAT: the name, or the user. WHY: always somebody
    format!("{who} ({machine})")
}

/// The columns read for a device, in order.
const COLUMNS: &str = "id, public_key, state, admin, app, machine, os_user, display_name, approved_by, approved_at"; // WHAT: one list. WHY: every read builds a Device the same way

fn device_from(r: &rusqlite::Row) -> rusqlite::Result<Option<Device>> {
    let text: String = r.get(1)?; // WHAT: the key as stored, base64. WHY: read back into bytes
    let Some(public_key) = wire::parse_public_key(&text) else { return Ok(None) }; // WHAT: a stored key that doesn't read. WHY: never trusted; treated as no device
    Ok(Some(Device {
        id: r.get(0)?,
        public_key,
        state: r.get(2)?,
        admin: r.get::<_, i64>(3)? != 0,
        app: r.get(4)?,
        machine: r.get(5)?,
        os_user: r.get(6)?,
        display_name: r.get(7)?,
        approved_by: r.get(8)?,
        approved_at: r.get(9)?,
    }))
}

/// One device by id, if the server has it.
///
/// # Errors
///
/// When the database can't be read.
pub fn find(db: &Connection, id: &str) -> rusqlite::Result<Option<Device>> {
    Ok(db
        .query_row(&format!("SELECT {COLUMNS} FROM devices WHERE id = ?1"), params![id], device_from)
        .optional()?
        .flatten()) // WHAT: the row, read, or none. WHY: a row whose key doesn't read is no device
}

/// How many approved admin devices there are.
fn admins(db: &Connection) -> rusqlite::Result<i64> {
    db.query_row("SELECT count(*) FROM devices WHERE state = 'approved' AND admin = 1", [], |r| r.get(0)) // WHAT: the count. WHY: the last one can't be lost
}

/// How many devices are approved at all.
fn approved(db: &Connection) -> rusqlite::Result<i64> {
    db.query_row("SELECT count(*) FROM devices WHERE state = 'approved'", [], |r| r.get(0)) // WHAT: the count. WHY: none means the server is unclaimed
}

// ---- the clock and the claiming window -------------------------------------

/// RFC 3339 for a time, as the rest of the server writes it.
fn rfc3339(at: time::OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default() // WHAT: the text. WHY: the settings and the table keep times this way
}

/// A time from its RFC 3339 text.
fn parse(text: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(text.trim(), &time::format_description::well_known::Rfc3339).ok() // WHAT: read, or none. WHY: a time that doesn't read never opens anything
}

/// Notes, once, when the device list began and until when the first computer
/// may claim the server. Called as the server starts.
///
/// The service's first start is the earliest of now and the first account's
/// creation: a server that already had people when it first met 0.7.0 has
/// been running a long time, so its claiming window is long shut and its
/// seats come over with their sessions instead.
pub fn begin(server: &Server) {
    let now = time::OffsetDateTime::now_utc(); // WHAT: this start. WHY: the device list's beginning, if it has none yet
    if !matches!(server.store.setting(settings::BEGAN), Ok(Some(_))) {
        let _ = server.store.set_setting(settings::BEGAN, &rfc3339(now)); // WHAT: written once. WHY: sessions issued after it can't carry a seat over
    }
    if !matches!(server.store.setting(settings::CLAIM_UNTIL), Ok(Some(_))) {
        let first_person = server
            .store
            .with(|db| Ok(db.query_row("SELECT min(created) FROM people", [], |r| r.get::<_, Option<String>>(0))?))
            .ok()
            .flatten()
            .and_then(|t| parse(&t)); // WHAT: when the first account was made, if any. WHY: the service ran then
        let first_start = first_person.map_or(now, |p| p.min(now)); // WHAT: the earlier of the two. WHY: as above
        let until = first_start + time::Duration::hours(CLAIM_HOURS); // WHAT: a day after. WHY: §3
        let _ = server.store.set_setting(settings::CLAIM_UNTIL, &rfc3339(until)); // WHAT: kept. WHY: a restart never reopens it
    }
}

/// Opens claiming again for a day, from now: the Manager's "reopen", on the
/// server PC itself (`hyperview-server --reopen-claim`).
///
/// # Errors
///
/// When the setting can't be written.
pub fn reopen_claim(store: &crate::Store) -> anyhow::Result<String> {
    let until = rfc3339(time::OffsetDateTime::now_utc() + time::Duration::hours(CLAIM_HOURS)); // WHAT: a day from now. WHY: the same window as at first
    store.set_setting(settings::CLAIM_UNTIL, &until)?; // WHAT: kept. WHY: read by the next request
    let _ = store.audit(&Entry::new(action::REOPENED_CLAIM, "").saying(format!("until {until}"))); // WHAT: written. WHY: every call is in the audit log
    Ok(until)
}

/// Whether the first computer to ask would be made the admin now: no device
/// is approved, and the claiming window is open.
fn claimable(db: &Connection, until: Option<&str>, now: time::OffsetDateTime) -> rusqlite::Result<bool> {
    let open = until.and_then(parse).is_some_and(|until| now < until); // WHAT: inside the window. WHY: guard 2; a window that doesn't read is shut
    Ok(open && approved(db)? == 0) // WHAT: and unclaimed. WHY: guard 2
}

/// The pin line this server's enrol proofs are bound to.
pub fn pin_line(server: &Server) -> String {
    server.pin.get().cloned().unwrap_or_else(|| wire::NO_PIN.to_string()) // WHAT: the 8714 certificate's pin, or "none" without HTTPS. WHY: §3: a request made to an impostor can't be relayed
}

// ---- the replay cache -------------------------------------------------------

/// Signatures already seen inside the ±300 s window.
#[derive(Default)]
pub struct Replays {
    seen: Mutex<HashMap<String, i64>>, // WHAT: each signature and when it may be forgotten (Unix seconds). WHY: §4: a replay is refused
}

/// The most signatures held at once.
const MOST_REPLAYS: usize = 200_000; // WHAT: two hundred thousand. WHY: ten minutes of a busy office several times over; past it, requests are refused rather than replays let through

impl Replays {
    /// True the first time `signature` is offered; false for a replay, or
    /// when the cache is full even after forgetting what has run out.
    pub fn first_time(&self, signature: &str, signed_at: i64, now: i64) -> bool {
        let Ok(mut seen) = self.seen.lock() else { return false }; // WHAT: a poisoned lock refuses. WHY: failing closed: a replay must never get through
        if seen.len() >= MOST_REPLAYS {
            seen.retain(|_, forget| *forget >= now); // WHAT: forget what is past the window. WHY: those can't be replayed anyway
            if seen.len() >= MOST_REPLAYS {
                return false; // WHAT: still full. WHY: refusing is the safe way round
            }
        }
        let forget = signed_at.max(now) + wire::WINDOW_SECONDS; // WHAT: kept until its time leaves the window. WHY: after that the window check refuses it
        seen.insert(signature.to_string(), forget).is_none() // WHAT: inserted, and whether it was new. WHY: one lock, so two copies sent together can't both pass
    }
}

// ---- checking a signed request ----------------------------------------------

/// Why a signed request was refused. Every one is answered the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Missing, // WHAT: no or malformed headers. WHY: told apart in tests only
    Time, // WHAT: outside ±300 s. WHY: likewise
    Unknown, // WHAT: no such device. WHY: likewise
    NotApproved, // WHAT: waiting, refused or removed. WHY: likewise
    Signature, // WHAT: the signature doesn't verify. WHY: likewise
    Replay, // WHAT: seen before. WHY: likewise
}

/// The headers of a signed request, read.
struct Headers {
    device: String, // WHAT: the device id. WHY: the key is found by it
    time: i64, // WHAT: when it was signed. WHY: the window
    signature: String, // WHAT: the base64 signature. WHY: checked
}

/// Reads the three headers, or `None` when any is missing or malformed.
fn headers_of(headers: &HeaderMap) -> Option<Headers> {
    let get = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim); // WHAT: one header as text. WHY: read three times
    let device = get(wire::HEADER_DEVICE)?; // WHAT: the id. WHY: §4
    let time = get(wire::HEADER_TIME)?; // WHAT: the time. WHY: §4
    let signature = get(wire::HEADER_SIGNATURE)?; // WHAT: the signature. WHY: §4
    if !wire::is_device_id(device) || time.is_empty() || time.len() > 12 || !time.bytes().all(|b| b.is_ascii_digit()) || signature.len() != 88 {
        return None; // WHAT: malformed. WHY: hostile headers only ever make it refuse
    }
    Some(Headers { device: device.to_string(), time: time.parse().ok()?, signature: signature.to_string() })
}

/// The first half of the check, before the body is read: the headers, the
/// time, and a device in a state that may pass. Cheap, so a stranger's body
/// is never read.
fn before_the_body(server: &Server, headers: &HeaderMap, any_state: bool, now: i64) -> Result<(Headers, Device), Refusal> {
    let signed = headers_of(headers).ok_or(Refusal::Missing)?; // WHAT: the headers. WHY: none, none further
    if (now - signed.time).abs() > wire::WINDOW_SECONDS {
        return Err(Refusal::Time); // WHAT: outside ±300 s. WHY: §4
    }
    let device = server.store.with(|db| Ok(find(db, &signed.device)?)).map_err(|_| Refusal::Unknown)?.ok_or(Refusal::Unknown)?; // WHAT: the device, or refused; a database failure refuses too. WHY: failing closed
    if !any_state && device.state != state::APPROVED {
        return Err(Refusal::NotApproved); // WHAT: not approved. WHY: §4, guard 3
    }
    Ok((signed, device))
}

/// The second half: the signature over the method, target and body, and the
/// replay cache.
fn after_the_body(server: &Server, signed: &Headers, device: &Device, method: &str, target: &str, body: &[u8], now: i64) -> Result<(), Refusal> {
    let hash = wire::body_hash(body); // WHAT: the body's digest. WHY: the fifth line
    let message = wire::Request { device_id: &signed.device, method, target, body_sha256: &hash, time: signed.time }.message().map_err(|_| Refusal::Signature)?; // WHAT: the six lines. WHY: a target that can't be signed can't verify
    if !wire::verify(&device.public_key, &message, &signed.signature) {
        return Err(Refusal::Signature); // WHAT: not this device's signature. WHY: §4, guard 3
    }
    if !server.replays.first_time(&signed.signature, signed.time, now) {
        return Err(Refusal::Replay); // WHAT: seen before. WHY: §4, guard 3
    }
    Ok(())
}

/// The whole check for a request whose body is already read: an approved
/// device's signature (or, with `any_state`, any known device's).
///
/// # Errors
///
/// Why it was refused.
pub fn check(server: &Server, headers: &HeaderMap, method: &str, target: &str, body: &[u8], any_state: bool, now: i64) -> Result<Device, Refusal> {
    let (signed, device) = before_the_body(server, headers, any_state, now)?; // WHAT: headers, time, device. WHY: as the gate does
    after_the_body(server, &signed, &device, method, target, body, now)?; // WHAT: signature and replay. WHY: likewise
    Ok(device)
}

/// The one answer every refusal gets: 401 `device_unknown`.
pub(crate) fn unknown() -> Denied {
    Denied::new(
        StatusCode::UNAUTHORIZED,
        Problem::new("device_unknown", "This computer isn't approved on the office server. Ask an admin to approve it, then try again."),
    ) // WHAT: §4's answer. WHY: the same whatever failed, so a stranger learns nothing
}

// ---- the gate -----------------------------------------------------------------

/// How a route is held by the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    Open, // WHAT: no device needed. WHY: health, the office's identity, the device request, and updates for seats from before 0.7.0
    Me, // WHAT: any known device's signature. WHY: a waiting device asks how it stands
    Admin, // WHAT: an approved admin device's signature, always. WHY: guard 4
    Gated, // WHAT: an approved device's signature, when the server requires devices. WHY: §5
}

/// Which door a route is, by its method and its pattern inside `/api/v1`.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn door(method: &str, route: &str) -> Door {
    match (method, route) {
        // WHAT: is it there; the office certificate; asking to be let in; the office's update.
        // WHY: health is kept open as it is now; the certificate is read before anything is trusted; the request is held by
        // its own checks (local only, rate-limited, proven); a seat from before 0.7.0 can't sign, and must still be able to
        // fetch the build that can (the update routes keep their own sign-in checks).
        ("GET", "/health" | "/office/identity" | "/update/latest" | "/update/:version/download") | ("POST", "/devices/request") => Door::Open,
        ("GET", "/devices/me") => Door::Me, // WHAT: how this device stands. WHY: answered in every state
        ("GET", "/devices") => Door::Admin, // WHAT: the list. WHY: guard 4
        ("POST", r) if r.starts_with("/devices/") => Door::Admin, // WHAT: approve, refuse, remove, admin. WHY: guard 4
        _ => Door::Gated, // WHAT: everything else. WHY: a new route needs a device unless it is put above on purpose
    }
}

/// The most large signed bodies read at once.
fn body_turn() -> &'static tokio::sync::Semaphore {
    static TURNS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new(); // WHAT: one limit for the gate. WHY: apart from the routes' own, so the two can never wait on each other
    TURNS.get_or_init(|| tokio::sync::Semaphore::new(4)) // WHAT: four at a time. WHY: as the routes' own limit
}

/// Holds every API route to its door, in front of View's own checks.
pub(crate) async fn gate(State(server): State<Shared>, request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let route = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_default(); // WHAT: the route's pattern. WHY: compared, never the raw address
    let route = route.strip_prefix(hub::API_ROOT).unwrap_or(&route).to_string(); // WHAT: without /api/v1. WHY: `door` names routes the way the router does inside the API
    let method = request.method().as_str().to_string(); // WHAT: GET, POST... WHY: half of which door
    let door = door(&method, &route); // WHAT: this route's door. WHY: decides what is checked
    match door {
        Door::Open => return next.run(request).await, // WHAT: through. WHY: its own checks decide
        Door::Gated if server.config.devices != crate::config::Devices::Required => return next.run(request).await, // WHAT: a server that doesn't require devices. WHY: only the Office Server started for real does (Config::from_environment)
        Door::Gated if route == "/join" && api::from_outside(request.headers()) => return next.run(request).await, // WHAT: a join from outside. WHY: §5: the join code and invitations stay a way in for computers outside the office; a good one admits the computer
        Door::Gated if is_live_key(&server, request.headers()) => return next.run(request).await, // WHAT: a program's key. WHY: keys are made by a signed-in person, scoped and taken back one at a time; the programs holding them are servers, not computers on the list
        _ => {}
    }
    let now = wire::unix_now(); // WHAT: the time, once. WHY: the window and the cache read the same clock
    let (mut parts, body) = request.into_parts(); // WHAT: the request in halves. WHY: the body is read only once the headers pass
    let (signed, device) = match before_the_body(&server, &parts.headers, door == Door::Me, now) {
        Ok(found) => found, // WHAT: a device that may pass. WHY: its body is worth reading now
        Err(why) => return refused(&server, &parts.headers, why, body).await, // WHAT: refused unread. WHY: a stranger's body is never read
    };
    let large = parts.headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).is_none_or(|n| n > api::SMALL_BODY as u64); // WHAT: a body that says it is big, or doesn't say. WHY: those take a turn
    let _turn = if large { body_turn().acquire().await.ok() } else { None }; // WHAT: a turn for a big body. WHY: a few big uploads at once can't hold gigabytes
    let limit = usize::try_from(server.config.largest_upload).unwrap_or(usize::MAX).saturating_add(1024 * 1024); // WHAT: the upload limit, and a little for the form around it. WHY: the routes' own limits still apply after
    let Ok(bytes) = axum::body::to_bytes(body, limit).await else {
        return Denied::new(StatusCode::PAYLOAD_TOO_LARGE, Problem::new("too_large", "That is more than this server takes in one request.")).into_response(); // WHAT: 413. WHY: past any route's limit
    };
    let target = parts
        .extensions
        .get::<axum::extract::OriginalUri>()
        .and_then(|u| u.0.path_and_query().map(|p| p.as_str().to_string()))
        .unwrap_or_else(|| parts.uri.path_and_query().map(|p| p.as_str().to_string()).unwrap_or_default()); // WHAT: the path and query exactly as sent, before the router took /api/v1 off. WHY: §4's fourth line
    if let Err(why) = after_the_body(&server, &signed, &device, &method, &target, &bytes, now) {
        return refused(&server, &parts.headers, why, Body::empty()).await; // WHAT: refused. WHY: the signature or a replay
    }
    if door == Door::Admin && !(device.admin && device.state == state::APPROVED) {
        return Denied::new(
            StatusCode::FORBIDDEN,
            Problem::new("not_an_admin", "Only an admin's computer can approve, refuse or remove computers. An admin can make this one an admin."),
        )
        .into_response(); // WHAT: 403. WHY: guard 4
    }
    parts.extensions.insert(device); // WHAT: the device, for the route. WHY: /devices/me and the admin routes read it
    next.run(axum::extract::Request::from_parts(parts, Body::from(bytes))).await // WHAT: on, with the body read. WHY: the routes read it as always
}

/// Whether the bearer token is a program's live key.
fn is_live_key(server: &Server, headers: &HeaderMap) -> bool {
    let Some(token) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer "))).map(str::trim) else {
        return false; // WHAT: no bearer. WHY: not a key
    };
    if !token.starts_with(crate::auth::KEY_PREFIX) {
        return false; // WHAT: a session, not a key. WHY: sessions are people on computers, which need a device
    }
    let now = api::now(); // WHAT: the time. WHY: a key past its end is no key
    matches!(server.store.with(|db| crate::auth::purpose_of(db, token, &now)), Ok(Some(_))) // WHAT: a key that exists and is live. WHY: a made-up "hvk_" is never waved through
}

/// The refusal, written to the audit log at most once a minute an address.
async fn refused(server: &Server, headers: &HeaderMap, why: Refusal, body: Body) -> Response {
    let from = api::seen_from(headers); // WHAT: where it came from. WHY: one machine or forty
    if api::refusal_worth_a_line(server, &format!("device {from}")) {
        api::record(server, Entry::new(action::REFUSED_REQUEST, "").saying(format!("{why:?}")).from(from)); // WHAT: one line. WHY: an admin can see a computer being turned away
    }
    api::turned_away(body, unknown()).await // WHAT: 401 device_unknown, with a little of the body read. WHY: §4's one answer
}

// ---- asking to be let in ----------------------------------------------------------

/// A session from before the device list, still valid now: the person it
/// belongs to. Only a session, never a key.
fn migratable_session(db: &Connection, headers: &HeaderMap, began: Option<&str>, now: &str) -> rusqlite::Result<Option<(String, String, bool)>> {
    let Some(token) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::trim) else {
        return Ok(None); // WHAT: no session sent. WHY: nothing to carry over
    };
    if token.starts_with(crate::auth::KEY_PREFIX) {
        return Ok(None); // WHAT: a key. WHY: a key is a program, never a seat
    }
    let Some(began) = began.and_then(parse) else { return Ok(None) }; // WHAT: no beginning recorded. WHY: nothing can be shown to predate it
    let found: Option<(String, String, String, String)> = db
        .query_row(
            "SELECT people.id, people.email, people.role, sessions.issued
             FROM sessions JOIN people ON people.id = sessions.person
             WHERE sessions.token = ?1 AND sessions.expires > ?2",
            params![crate::auth::token_hash(token), now],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?; // WHAT: the session's person and when it was issued, only if it hasn't run out. WHY: guard 8: valid at the moment of migration
    Ok(found.and_then(|(id, email, role, issued)| {
        let before = parse(&issued).is_some_and(|issued| issued < began); // WHAT: issued before the device list began. WHY: a session made since was made on an approved device, or let in from outside, and never needs carrying over
        before.then_some((id, email, role == "admin")) // WHAT: the person, and whether an administrator. WHY: §5: the office's View admins are made device admins
    }))
}

/// One line of ordinary text, or empty, at most `wire::LONGEST_NAME` long.
fn plain(text: &str) -> bool {
    text.chars().count() <= wire::LONGEST_NAME && !text.chars().any(char::is_control) // WHAT: bounded, no control characters. WHY: it is shown to an admin and written in the log
}

/// The checks every request to be let in passes, before anything is kept:
/// its fields, its time and its proof. The key's bytes when it holds.
fn proven(server: &Server, ask: &Ask, now: i64) -> Answer<[u8; 32]> {
    if !plain(&ask.app_version) || !plain(&ask.display_name) || !plain(&ask.platform) {
        return Err(Denied::wrong("The app's version, the person's name and the platform are each one short line.")); // WHAT: refused. WHY: shown to an admin as they are
    }
    if (now - ask.time).abs() > wire::WINDOW_SECONDS {
        return Err(Denied::wrong("This computer's clock is more than five minutes from the server's. Set the clock right and try again.")); // WHAT: refused. WHY: an old proof is never reused
    }
    ask.proven(&pin_line(server)).ok_or_else(|| {
        Denied::new(StatusCode::FORBIDDEN, Problem::new("bad_proof", "That request wasn't signed for this server. Connect to the office server again and ask once more."))
    }) // WHAT: the key, when its proof is for this server. WHY: §3: a request made to an impostor can't be relayed
}

/// How a device came in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum How {
    Asked, // WHAT: asked, to wait for an admin. WHY: the ordinary way
    Claimed, // WHAT: the first computer, made the admin. WHY: §3
    Migrated, // WHAT: carried over with a session from before 0.7.0. WHY: §5
    Joined, // WHAT: let in from outside by a join code or invitation. WHY: §5
}

impl How {
    fn word(self) -> &'static str {
        match self {
            How::Asked => "asked",
            How::Claimed => "claimed",
            How::Migrated => "migrated",
            How::Joined => "joined",
        }
    }
}

/// What came of a request to be let in.
struct Outcome {
    state: &'static str, // WHAT: waiting or approved. WHY: the answer
    how: Option<How>, // WHAT: how a new or re-asking device came in; none when it was already known and nothing changed. WHY: what is written in the log
    approved_by: String, // WHAT: who let it in, when it was let in at once. WHY: the log and the list
    known: bool, // WHAT: whether the device was already waiting or approved. WHY: a known device's ask isn't a stranger's guess
}

/// Keeps a device request, in one step: a known device answered as it
/// stands, a refused or removed one put back to waiting, a new one carried
/// over, claimed, or put to wait.
fn keep(db: &Connection, ask: &Ask, id: &str, from: &str, session: Option<&(String, String, bool)>, joined: Option<&str>, claim_until: Option<&str>) -> rusqlite::Result<Result<Outcome, Denied>> {
    let now_at = time::OffsetDateTime::now_utc(); // WHAT: now. WHY: written on the row
    let now = rfc3339(now_at); // WHAT: as text. WHY: likewise
    let existing = find(db, id)?; // WHAT: the device, if already known. WHY: asking twice is the same as asking once
    if let Some(device) = &existing {
        if device.state == state::APPROVED {
            return Ok(Ok(Outcome { state: state::APPROVED, how: None, approved_by: String::new(), known: true })); // WHAT: already in. WHY: nothing to change
        }
        if device.state == state::WAITING && session.is_none() && joined.is_none() {
            db.execute(
                "UPDATE devices SET app_version = ?2, display_name = ?3, platform = ?4, asked_from = ?5, changed = ?6 WHERE id = ?1",
                params![id, ask.app_version, ask.display_name, ask.platform, from, now],
            )?; // WHAT: the shown details refreshed. WHY: the admin sees what the computer says now
            return Ok(Ok(Outcome { state: state::WAITING, how: None, approved_by: String::new(), known: true })); // WHAT: still waiting. WHY: an admin hasn't answered
        }
    }
    let waiting: i64 = db.query_row("SELECT count(*) FROM devices WHERE state = 'waiting'", [], |r| r.get(0))?; // WHAT: how many wait now. WHY: the list is bounded
    if existing.is_none() && waiting >= MOST_WAITING {
        return Ok(Err(Denied::new(StatusCode::SERVICE_UNAVAILABLE, Problem::new("too_many_waiting", "Too many computers are waiting to be approved. Ask an admin to answer them first.")))); // WHAT: refused. WHY: a stranger can't fill the list without end
    }
    let (state_now, admin, how, by) = if let Some((_, email, is_admin)) = session {
        (state::APPROVED, *is_admin, How::Migrated, email.clone()) // WHAT: carried over, an administrator's seat as an admin device. WHY: §5
    } else if let Some(how_joined) = joined {
        (state::APPROVED, false, How::Joined, how_joined.to_string()) // WHAT: let in by the code or invitation it joined with. WHY: §5
    } else if claimable(db, claim_until, now_at)? {
        (state::APPROVED, true, How::Claimed, "the first computer to set up the office server".to_string()) // WHAT: the first computer, made the admin. WHY: §3
    } else {
        (state::WAITING, false, How::Asked, String::new()) // WHAT: waiting. WHY: an admin decides
    };
    let approved_at = (state_now == state::APPROVED).then(|| now.clone()); // WHAT: when, if let in now. WHY: /devices/me says it
    let approved_by = (state_now == state::APPROVED).then(|| by.clone()); // WHAT: who, if let in now. WHY: likewise
    let person = session.map(|(id, _, _)| id.clone()); // WHAT: the person it was carried over with. WHY: kept for whoever reads the list later
    db.execute(
        "INSERT INTO devices (id, public_key, app, app_version, machine, os_user, display_name, platform, state, admin, asked, asked_from, approved_by, approved_at, person, how, changed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?11)
         ON CONFLICT(id) DO UPDATE SET app = excluded.app, app_version = excluded.app_version, machine = excluded.machine, os_user = excluded.os_user,
             display_name = excluded.display_name, platform = excluded.platform, state = excluded.state, admin = excluded.admin, asked = excluded.asked,
             asked_from = excluded.asked_from, approved_by = excluded.approved_by, approved_at = excluded.approved_at, person = excluded.person, how = excluded.how, changed = excluded.changed",
        params![id, ask.public_key, ask.app, ask.app_version, ask.machine, ask.os_user, ask.display_name, ask.platform, state_now, i64::from(admin), now, from, approved_by, approved_at, person, how.word()],
    )?; // WHAT: the device, new or asking again. WHY: one row per key
    Ok(Ok(Outcome { state: state_now, how: Some(how), approved_by: by, known: false }))
}

/// `POST /api/v1/devices/request` (§3): a computer asks to be let in.
pub(crate) async fn request(State(server): State<Shared>, headers: HeaderMap, body: Body) -> Response {
    match asked(&server, &headers, body).await {
        Ok(answer) => Json(answer).into_response(), // WHAT: the device id and its state. WHY: §3
        Err(denied) => denied.into_response(), // WHAT: why not. WHY: in words
    }
}

async fn asked(server: &Shared, headers: &HeaderMap, body: Body) -> Answer<wire::Answer> {
    let now = api::now(); // WHAT: the time, as text. WHY: sessions are compared with it
    let began = server.store.setting(settings::BEGAN).ok().flatten(); // WHAT: when the device list began. WHY: the migration cut-off
    let session = server
        .store
        .with(|db| Ok(migratable_session(db, headers, began.as_deref(), &now)?))
        .map_err(|e| Denied::broke("check that session", e))?; // WHAT: a session from before 0.7.0, if one came. WHY: §5's migration
    let local = !api::from_outside(headers) && api::seen_from(headers).parse::<std::net::IpAddr>().is_ok_and(hub::office::is_local); // WHAT: not marked as from the internet, and the socket's address on a local network. WHY: §3; a router forwarding the port straight in is not the office network either, and no forwarded header alone decides
    if !local && session.is_none() {
        return Err(Denied::new(
            StatusCode::FORBIDDEN,
            Problem::new("not_on_the_office_network", "Computers ask to be let in from the office network. Connect from the office, or use a join code or invitation."),
        )); // WHAT: refused (403). WHY: §3, guard 1; only a seat carrying a session from before 0.7.0 comes over from outside, because it was admitted before
    }
    let from = api::seen_from(headers); // WHAT: where it came from. WHY: counted, and shown to the admin
    let counted = format!("device-ask {}", crate::patience::counted_as(&from)); // WHAT: the address as counted, apart from sign-ins and joins. WHY: one door's tries never cost another's
    if let Err(owed) = server.patience.begin(crate::patience::JOINING, &[counted.as_str()]) {
        return Err(Denied::new(StatusCode::TOO_MANY_REQUESTS, Problem::new("too_many", crate::patience::refusal(owed)))); // WHAT: 429. WHY: §3: rate-limited per address with the JOINING rule
    }
    let ask: Ask = api::json_once_let_in(body, ASK_BODY).await?; // WHAT: the request, read only now. WHY: after the cheap checks
    let key = proven(server, &ask, wire::unix_now())?; // WHAT: the key, its proof checked. WHY: nothing is kept unproven
    let id = wire::device_id(&key); // WHAT: the id, worked out here. WHY: never taken from the asker
    let until = server.store.setting(settings::CLAIM_UNTIL).ok().flatten(); // WHAT: the claiming window. WHY: guard 2
    let outcome = server
        .store
        .with(|db| Ok(keep(db, &ask, &id, &from, session.as_ref(), None, until.as_deref())?))
        .map_err(|e| Denied::broke("keep that request", e))??; // WHAT: kept, in one step. WHY: two computers asking together can't both claim the server
    if outcome.known || outcome.state == state::APPROVED {
        server.patience.right(&counted); // WHAT: a known computer asking again, or one let in at once (claimed, or carried over with its session). WHY: not a stranger's try; only a new computer left waiting is counted
    }
    write_down(server, &ask, &id, &from, &outcome); // WHAT: the audit line. WHY: every call is written in the audit log
    Ok(wire::Answer { device_id: id, state: outcome.state.to_string() })
}

/// The audit line for a request to be let in.
fn write_down(server: &Server, ask: &Ask, id: &str, from: &str, outcome: &Outcome) {
    let who = label(&ask.display_name, &ask.os_user, &ask.machine); // WHAT: "Dana (DANA-LAPTOP)". WHY: what an admin recognises
    let what = match outcome.how {
        Some(How::Claimed) => action::CLAIMED,
        Some(How::Migrated) => action::MIGRATED,
        Some(How::Joined) => action::JOINED,
        Some(How::Asked) | None => action::ASKED,
    }; // WHAT: which action. WHY: the log reads plainly
    let mut entry = Entry::new(what, &who).about(format!("{} on {}", wire::app_title(&ask.app), id)).from(from); // WHAT: who, which app, which device, where from. WHY: the line an auditor wants
    if outcome.approved_by.is_empty() {
        entry = entry.saying(outcome.state.to_string()); // WHAT: waiting, or already approved. WHY: said
    } else {
        entry = entry.saying(format!("approved by {}", outcome.approved_by)); // WHAT: who let it in. WHY: said
    }
    api::record(server, entry); // WHAT: written. WHY: §3
}

/// The proof of a join's computer, checked before the account is made.
pub(crate) fn proven_for_join(server: &Server, ask: &Ask) -> Answer<()> {
    proven(server, ask, wire::unix_now()).map(|_| ()) // WHAT: the same checks as any request. WHY: refused before anything is kept
}

/// A join from outside that carried its computer's request: the computer
/// let in, approved, by the code or invitation it joined with (§5).
pub(crate) fn joined(server: &Server, ask: &Ask, from: &str, by: &str) -> Answer<()> {
    let key = proven(server, ask, wire::unix_now())?; // WHAT: the proof, as for any request. WHY: a join can't let in a key nobody holds
    let id = wire::device_id(&key); // WHAT: its id. WHY: worked out here
    let outcome = server
        .store
        .with(|db| Ok(keep(db, ask, &id, from, None, Some(by), None)?))
        .map_err(|e| Denied::broke("keep that computer", e))??; // WHAT: kept approved. WHY: the join admitted it
    write_down(server, ask, &id, from, &outcome); // WHAT: the audit line. WHY: every call is written
    Ok(())
}

// ---- how a device stands --------------------------------------------------------

/// `GET /api/v1/devices/me` (§3), signed by the device in any state.
pub(crate) async fn me(Extension(device): Extension<Device>) -> Json<wire::Me> {
    Json(wire::Me { state: device.state.clone(), admin: device.admin && device.state == state::APPROVED, approved_by: device.approved_by.clone(), approved_at: device.approved_at.clone() }) // WHAT: its state, and only an approved device is an admin. WHY: the waiting screen reads it
}

// ---- what an admin does -----------------------------------------------------------

/// `GET /api/v1/devices`: every device, newest waiting first.
pub(crate) async fn list(State(server): State<Shared>, Extension(_admin): Extension<Device>) -> Answer<Json<Vec<wire::Info>>> {
    let all = server
        .store
        .with(|db| {
            let mut statement = db.prepare(
                "SELECT id, app, app_version, machine, os_user, display_name, platform, state, admin, asked, asked_from, approved_by, approved_at, how
                 FROM devices ORDER BY (state = 'waiting') DESC, asked DESC",
            )?; // WHAT: waiting first, newest first. WHY: §3
            let rows = statement.query_map([], |r| {
                Ok(wire::Info {
                    id: r.get(0)?,
                    app: r.get(1)?,
                    app_version: r.get(2)?,
                    machine: r.get(3)?,
                    os_user: r.get(4)?,
                    display_name: r.get(5)?,
                    platform: r.get(6)?,
                    state: r.get(7)?,
                    admin: r.get::<_, i64>(8)? != 0,
                    asked: r.get(9)?,
                    asked_from: r.get(10)?,
                    approved_by: r.get(11)?,
                    approved_at: r.get(12)?,
                    how: r.get(13)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .map_err(|e| Denied::broke("read the computers", e))?;
    Ok(Json(all))
}

/// The body of an approval.
#[derive(Deserialize, Default)]
pub(crate) struct Approve {
    #[serde(default)]
    admin: bool, // WHAT: approve as an admin. WHY: "Approve as admin"
}

/// The body of an admin change.
#[derive(Deserialize)]
pub(crate) struct AdminOn {
    on: bool, // WHAT: make or unmake. WHY: §3
}

/// What an admin asked to do to a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Approve { admin: bool }, // WHAT: let it in. WHY: §3
    Refuse, // WHAT: turn it down. WHY: §3
    Remove, // WHAT: take it off; it must ask again. WHY: §3
    Admin { on: bool }, // WHAT: make or unmake an admin. WHY: §3
}

/// The answer when the last admin would be lost.
fn last_admin() -> Denied {
    Denied::new(StatusCode::CONFLICT, Problem::new("last_admin", "Make someone else an admin first: this is the office's last admin computer.")) // WHAT: 409. WHY: §3, guard 4
}

/// Makes one change, in one step, keeping the last admin.
fn change(db: &Connection, id: &str, what: Change, by: &str) -> rusqlite::Result<Result<Device, Denied>> {
    let Some(target) = find(db, id)? else {
        return Ok(Err(Denied::missing("computer"))); // WHAT: 404. WHY: nothing to change
    };
    let is_admin_now = target.state == state::APPROVED && target.admin; // WHAT: whether it counts as an admin now. WHY: only those can be the last
    let loses_admin = match what {
        Change::Approve { admin } => !admin, // WHAT: approved without admin. WHY: an approved admin approved again as not one is unmade
        Change::Refuse | Change::Remove => true, // WHAT: no longer anything. WHY: it stops being an admin
        Change::Admin { on } => !on, // WHAT: unmade. WHY: likewise
    };
    if is_admin_now && loses_admin && admins(db)? <= 1 {
        return Ok(Err(last_admin())); // WHAT: refused. WHY: guard 4: the last admin can't be lost
    }
    let now = rfc3339(time::OffsetDateTime::now_utc()); // WHAT: now. WHY: written on the row
    match what {
        Change::Approve { admin } => {
            db.execute(
                "UPDATE devices SET state = 'approved', admin = ?2, approved_by = ?3, approved_at = ?4, changed = ?4 WHERE id = ?1",
                params![id, i64::from(admin), by, now],
            )?; // WHAT: approved. WHY: let in
        }
        Change::Refuse => {
            db.execute("UPDATE devices SET state = 'refused', admin = 0, changed = ?2 WHERE id = ?1", params![id, now])?; // WHAT: refused. WHY: turned down
        }
        Change::Remove => {
            db.execute("UPDATE devices SET state = 'removed', admin = 0, approved_by = NULL, approved_at = NULL, changed = ?2 WHERE id = ?1", params![id, now])?; // WHAT: removed. WHY: it must ask again
        }
        Change::Admin { on } => {
            if target.state != state::APPROVED {
                return Ok(Err(Denied::new(StatusCode::CONFLICT, Problem::new("not_approved", "Approve this computer first, then make it an admin.")))); // WHAT: 409. WHY: an admin that isn't let in makes no sense
            }
            db.execute("UPDATE devices SET admin = ?2, changed = ?3 WHERE id = ?1", params![id, i64::from(on), now])?; // WHAT: made or unmade. WHY: §3
        }
    }
    Ok(Ok(target))
}

/// Applies a change and writes it in the log.
fn changed(server: &Server, admin: &Device, id: &str, what: Change, headers: &HeaderMap) -> Answer<Json<serde_json::Value>> {
    let by = admin.label(); // WHAT: the admin's computer, by name. WHY: who approved it
    let target = server.store.with(|db| Ok(change(db, id, what, &by)?)).map_err(|e| Denied::broke("change that computer", e))??; // WHAT: the change, in one step. WHY: two admins at once can't both remove the other
    let (action, state_now) = match what {
        Change::Approve { admin: true } => (action::APPROVED, "approved as an admin"),
        Change::Approve { admin: false } => (action::APPROVED, "approved"),
        Change::Refuse => (action::REFUSED, "refused"),
        Change::Remove => (action::REMOVED, "removed"),
        Change::Admin { on: true } => (action::MADE_ADMIN, "an admin"),
        Change::Admin { on: false } => (action::UNMADE_ADMIN, "not an admin"),
    }; // WHAT: the log's words. WHY: plain
    api::record(server, Entry::new(action, &by).about(format!("{} on {} ({})", target.label(), wire::app_title(&target.app), target.id)).saying(state_now).from(api::seen_from(headers))); // WHAT: written. WHY: §3: every call is in the audit log
    Ok(Json(serde_json::json!({ "id": target.id, "done": state_now }))) // WHAT: what was done. WHY: the app refreshes its list
}

/// `POST /api/v1/devices/{id}/approve`, with `{ "admin": false }`.
pub(crate) async fn approve(State(server): State<Shared>, Extension(admin): Extension<Device>, Path(id): Path<String>, headers: HeaderMap, body: Body) -> Answer<Json<serde_json::Value>> {
    let bytes = api::body_once_let_in(body, ASK_BODY).await?; // WHAT: the body. WHY: may be empty
    let asked: Approve = if bytes.is_empty() { Approve::default() } else { serde_json::from_slice(&bytes).map_err(|e| Denied::wrong(format!("That is not the JSON this route takes: {e}")))? }; // WHAT: admin or not. WHY: empty means not
    changed(&server, &admin, &id, Change::Approve { admin: asked.admin }, &headers)
}

/// `POST /api/v1/devices/{id}/refuse`.
pub(crate) async fn refuse(State(server): State<Shared>, Extension(admin): Extension<Device>, Path(id): Path<String>, headers: HeaderMap) -> Answer<Json<serde_json::Value>> {
    changed(&server, &admin, &id, Change::Refuse, &headers)
}

/// `POST /api/v1/devices/{id}/remove`.
pub(crate) async fn remove(State(server): State<Shared>, Extension(admin): Extension<Device>, Path(id): Path<String>, headers: HeaderMap) -> Answer<Json<serde_json::Value>> {
    changed(&server, &admin, &id, Change::Remove, &headers)
}

/// `POST /api/v1/devices/{id}/admin`, with `{ "on": true|false }`.
pub(crate) async fn make_admin(State(server): State<Shared>, Extension(admin): Extension<Device>, Path(id): Path<String>, headers: HeaderMap, Json(asked): Json<AdminOn>) -> Answer<Json<serde_json::Value>> {
    changed(&server, &admin, &id, Change::Admin { on: asked.on }, &headers)
}

// ---- the office's identity ----------------------------------------------------------

/// `GET /api/v1/office/identity` (§2.4): the office certificate, as stored,
/// or 404 when there is none yet.
pub(crate) async fn identity(State(server): State<Shared>) -> Response {
    match server.store.setting(settings::OFFICE_CERT) {
        Ok(Some(cert)) if !cert.trim().is_empty() => ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], cert).into_response(), // WHAT: the certificate, exactly as stored. WHY: an app checks its signature and pin itself
        Ok(_) => Denied::missing("office certificate").into_response(), // WHAT: 404. WHY: none stored yet; the baseline pin still holds
        Err(e) => Denied::broke("read the office certificate", e).into_response(), // WHAT: 500. WHY: said, never served as missing
    }
}

/// Stores the office certificate the server will serve.
///
/// # Errors
///
/// When the setting can't be written.
pub fn store_identity(store: &crate::Store, cert: &str) -> anyhow::Result<()> {
    store.set_setting(settings::OFFICE_CERT, cert) // WHAT: kept as given. WHY: signing it is a later step; this only keeps and serves it
}

// ---- for the Citadel cell -------------------------------------------------------------

/// The answer the cell is given about one device (§6a).
pub fn for_the_cell(store: &crate::Store, id: &str) -> serde_json::Value {
    if !wire::is_device_id(id) {
        return serde_json::json!({ "state": "unknown" }); // WHAT: not an id. WHY: never looked up
    }
    match store.with(|db| Ok(find(db, id)?)) {
        Ok(Some(device)) if device.state == state::APPROVED => serde_json::json!({
            "state": "approved",
            "public_key": wire::public_key_text(&device.public_key),
            "admin": device.admin,
            "name": device.display_name,
            "machine": device.machine,
            "app": device.app,
        }), // WHAT: approved, with its key. WHY: the cell works Citadel's own id out from the key
        Ok(Some(device)) => serde_json::json!({ "state": device.state }), // WHAT: waiting, refused or removed. WHY: the cell refuses it
        Ok(None) | Err(_) => serde_json::json!({ "state": "unknown" }), // WHAT: unknown, or the database failed. WHY: failing closed
    }
}

/// One computer an old Citadel Server had paired (§6a `migrate`).
#[derive(Deserialize, Debug, Clone)]
pub struct Paired {
    pub public_key: String, // WHAT: its key, base64. WHY: the device list keeps it
    #[serde(default)]
    pub name: String, // WHAT: the person's name. WHY: shown
    #[serde(default)]
    pub machine: String, // WHAT: the computer. WHY: shown
    #[serde(default)]
    pub paired_at: String, // WHAT: when Citadel paired it. WHY: kept as its approval time
    #[serde(default)]
    pub paired_by: String, // WHAT: who paired it. WHY: §5: recorded
}

/// Brings an old Citadel Server's paired computers into the device list,
/// approved, once. The number added; `None` when it was already done.
///
/// # Errors
///
/// When the database can't be written.
pub fn from_citadel(store: &crate::Store, paired: &[Paired]) -> anyhow::Result<Option<usize>> {
    let added = store.with(|db| {
        let done: Option<String> = db.query_row("SELECT value FROM settings WHERE key = ?1", params![settings::CITADEL_MIGRATED], |r| r.get(0)).optional()?; // WHAT: whether it was done. WHY: §6a: sent once
        if done.is_some() {
            return Ok(None); // WHAT: already done. WHY: a second migration is refused
        }
        let now = rfc3339(time::OffsetDateTime::now_utc()); // WHAT: now. WHY: written on the rows
        let mut added = 0; // WHAT: how many came in. WHY: the answer
        for one in paired.iter().take(1000) {
            let Some(key) = wire::parse_public_key(&one.public_key) else { continue }; // WHAT: a key that doesn't read. WHY: skipped, never stored
            if !plain(&one.name) || !plain(&one.machine) || !plain(&one.paired_by) || !plain(&one.paired_at) {
                continue; // WHAT: a field that isn't one short line. WHY: skipped
            }
            let id = wire::device_id(&key); // WHAT: its id. WHY: worked out here
            let at = if parse(&one.paired_at).is_some() { one.paired_at.clone() } else { now.clone() }; // WHAT: when it was paired, or now. WHY: a date that doesn't read isn't kept
            let by = if one.paired_by.trim().is_empty() { "Citadel Server".to_string() } else { one.paired_by.clone() }; // WHAT: who paired it. WHY: §5
            added += db.execute(
                "INSERT OR IGNORE INTO devices (id, public_key, app, app_version, machine, os_user, display_name, platform, state, admin, asked, asked_from, approved_by, approved_at, person, how, changed)
                 VALUES (?1, ?2, 'latcon-estimating', '', ?3, '', ?4, 'windows-x64', 'approved', 0, ?5, '', ?6, ?5, NULL, 'citadel', ?7)",
                params![id, one.public_key, one.machine, one.name, at, by, now],
            )?; // WHAT: approved, unless the key is already listed. WHY: a computer already here keeps how it stands
        }
        db.execute("INSERT INTO settings (key, value) VALUES (?1, ?2)", params![settings::CITADEL_MIGRATED, now])?; // WHAT: done. WHY: never again
        Ok(Some(added))
    })?;
    if let Some(n) = added {
        let _ = store.audit(&Entry::new(action::CARRIED_FROM_CITADEL, "Citadel Server").saying(format!("{n} computers"))); // WHAT: written. WHY: every call is in the audit log
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replay_is_refused_and_a_fresh_signature_is_not() {
        let replays = Replays::default();
        assert!(replays.first_time("a", 100, 100));
        assert!(!replays.first_time("a", 100, 101), "the same signature again");
        assert!(replays.first_time("b", 100, 101));
    }

    #[test]
    fn every_new_route_needs_a_device_unless_it_is_named() {
        assert_eq!(door("GET", "/health"), Door::Open);
        assert_eq!(door("POST", "/devices/request"), Door::Open);
        assert_eq!(door("GET", "/devices/me"), Door::Me);
        assert_eq!(door("GET", "/devices"), Door::Admin);
        assert_eq!(door("POST", "/devices/:id/approve"), Door::Admin);
        assert_eq!(door("GET", "/projects"), Door::Gated);
        assert_eq!(door("POST", "/auth/token"), Door::Gated);
        assert_eq!(door("GET", "/something-new"), Door::Gated, "a new route is closed unless named");
    }
}
