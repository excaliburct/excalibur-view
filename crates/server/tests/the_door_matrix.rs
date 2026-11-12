//! Every route the Office Server serves, asked by every kind of caller, with
//! devices required as a real office runs it. The expected answer for each
//! comes from the table below; a route the router serves that the table
//! doesn't name fails the test, so a new route can't slip through unheld.
//!
//! What is checked is the door, not the room behind it: whether the device
//! gate refuses (401 `device_unknown`), whether an admin's door refuses a
//! device that isn't an admin's (403 `not_an_admin`), whether a key's door
//! refuses a key off its list (403 `key_not_allowed`), or whether the caller
//! gets past to the route's own checks.

use std::sync::Arc;

use hub::device::{self, DeviceKey, Signer};
use hyperview_server::{api, auth, Config, Store};
use rusqlite::params;

/// The maintenance key the test server is given.
const MAINTENANCE: &str = "maintenance-key-for-the-door-matrix-0123456789"; // WHAT: Fleet's key, made up. WHY: its doors are asked with it

/// A server as an office runs it: HTTPS, devices required, Fleet's key set.
struct Office {
    data: std::path::PathBuf, // WHAT: its data folder. WHY: the database is written beside it
    port: u16, // WHAT: its port. WHY: every request goes there
    pin: String, // WHAT: its certificate's pin. WHY: signatures and proofs are bound to it
    _runtime: tokio::runtime::Runtime, // WHAT: the server's runtime. WHY: kept alive for the test
}

fn start() -> Office {
    let n: u64 = rand::random(); // WHAT: a random folder name. WHY: below
    let data = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("matrix-{n:016x}")); // WHAT: a fresh folder. WHY: tests run side by side
    let config = Config {
        name: "LatCon".into(), // WHAT: the office's name. WHY: as LatCon's
        data: data.clone(), // WHAT: its data folder. WHY: fresh
        listen: "127.0.0.1:0".parse().unwrap(), // WHAT: any free port, on this machine. WHY: an inside address
        tls: true, // WHAT: HTTPS. WHY: as an office runs it
        devices: hyperview_server::config::Devices::Required, // WHAT: devices required. WHY: the thing under test
        maintenance_key: Some(MAINTENANCE.into()), // WHAT: Fleet's key. WHY: its doors are on the matrix too
        ..Config::default()
    };
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap(); // WHAT: the server's own runtime. WHY: the requests block
    let (told, heard) = std::sync::mpsc::channel(); // WHAT: a channel for the port. WHY: told once it listens
    runtime.spawn(async move { // WHAT: the server, running. WHY: until the test ends
        let _ = hyperview_server::run::serve(config, std::future::pending(), move |serving| {
            let _ = told.send(serving.address.port()); // WHAT: the port it took. WHY: below
        })
        .await; // WHAT: served. WHY: until the runtime is dropped
    });
    let port = heard.recv_timeout(std::time::Duration::from_secs(30)).expect("the server started"); // WHAT: the port. WHY: every request goes there
    let cert = std::fs::read_to_string(data.join(hyperview_server::tls::CERT)).expect("its certificate"); // WHAT: its certificate. WHY: made at the first start
    let pin = hyperview_server::tls::pin(&cert).unwrap(); // WHAT: its pin. WHY: what a computer pins
    Office { data, port, pin, _runtime: runtime } // WHAT: the running office. WHY: for the test
}

impl Office {
    fn store(&self) -> Store {
        let config = Config { data: self.data.clone(), ..Config::default() }; // WHAT: the same data folder. WHY: the same database
        Store::open(&config.database(), &config.blobs()).unwrap() // WHAT: the database. WHY: devices, keys and sessions are written into it
    }

    /// One request: its status and the error word, if any.
    fn knock(&self, method: &str, path: &str, body: &[u8], signer: Option<&Signer>, headers: &[(String, String)]) -> (u16, String) {
        let (agent, _) = hub::pinned::agent(Some(&self.pin)).unwrap(); // WHAT: an agent that trusts this pin only. WHY: as a computer connects
        let agent = agent.timeout(std::time::Duration::from_secs(20)).build(); // WHAT: with a time limit. WHY: a hung door fails the test, never stalls it
        let mut request = agent.request(method, &format!("https://127.0.0.1:{}{path}", self.port)); // WHAT: the request. WHY: from 127.0.0.1, an inside address
        if let Some(signer) = signer { // WHAT: a device's request. WHY: signed
            let s = signer.sign(&self.pin, method, path, body, device::unix_now()).unwrap(); // WHAT: signed for this request. WHY: as a real app signs
            request = request.set(device::HEADER_DEVICE, &s.device).set(device::HEADER_TIME, &s.time).set(device::HEADER_SIGNATURE, &s.signature).set(device::HEADER_BODY, &s.body); // WHAT: the four signature headers. WHY: §4
        }
        for (k, v) in headers { // WHAT: any other headers. WHY: a key, a session, the tunnel's mark
            request = request.set(k, v); // WHAT: each set. WHY: as given
        }
        let answer = if body.is_empty() { request.call() } else { request.set("Content-Type", "application/json").send_bytes(body) }; // WHAT: sent, with a body when there is one. WHY: a GET has none
        let word = |r: ureq::Response| r.into_json::<serde_json::Value>().ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or_default(); // WHAT: the error word of the answer. WHY: which door refused
        match answer { // WHAT: the status and word. WHY: compared with the table
            Ok(r) => (r.status(), word(r)), // WHAT: a success. WHY: past every door
            Err(ureq::Error::Status(code, r)) => (code, word(r)), // WHAT: a refusal. WHY: its status and word
            Err(e) => panic!("{method} {path}: {e}"), // WHAT: no answer. WHY: a door must always answer
        }
    }
}

/// Who knocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Caller {
    Nobody, // WHAT: no device, nothing signed. WHY: a stranger on the office network
    Waiting, // WHAT: a device waiting for an admin. WHY: let in nowhere but /devices/me
    Refused, // WHAT: a device an admin refused. WHY: likewise
    Approved, // WHAT: an approved device that isn't an admin's. WHY: View's routes, never the admin's
    Admin, // WHAT: an approved admin device. WHY: everywhere a device may go
    Citadel, // WHAT: a device carried over from Citadel Server, for the cell only. WHY: nowhere in View
    KeyAssistant, // WHAT: an assistant's key. WHY: keys pass the device gate; the routes decide
    KeyIntegration, // WHAT: an integration key. WHY: likewise
    KeySync, // WHAT: a sync key. WHY: its own list, and nothing else
    OldSessionInside, // WHAT: a session from before 0.7.0, no device, on the office network. WHY: carries a seat over at /devices/request only
    OldSessionOutside, // WHAT: the same, through the tunnel. WHY: likewise
    Maintenance, // WHAT: Fleet's maintenance key, no device. WHY: Fleet's doors only
    Outside, // WHAT: nobody, through the tunnel. WHY: the join door, and nothing else
}

const CALLERS: [Caller; 13] = [
    Caller::Nobody,
    Caller::Waiting,
    Caller::Refused,
    Caller::Approved,
    Caller::Admin,
    Caller::Citadel,
    Caller::KeyAssistant,
    Caller::KeyIntegration,
    Caller::KeySync,
    Caller::OldSessionInside,
    Caller::OldSessionOutside,
    Caller::Maintenance,
    Caller::Outside,
];

/// What a door says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Said {
    Past, // WHAT: through to the route's own checks. WHY: whatever they say then
    Gate, // WHAT: 401 device_unknown. WHY: the device gate
    NotAdmin, // WHAT: 403 not_an_admin. WHY: an admin's door
    Key, // WHAT: 403 key_not_allowed. WHY: a key off its list
    Fleet, // WHAT: refused by Fleet's door (401 or 404). WHY: no maintenance key
}

/// How a route is held, as the table says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Door {
    Open, // WHAT: no device needed. WHY: health, identity, asking, updates
    Me, // WHAT: any known device. WHY: /devices/me
    Admin, // WHAT: an approved admin device. WHY: the device list
    Gated, // WHAT: an approved device. WHY: View's routes
    Join, // WHAT: gated, except from outside. WHY: the join code and invitations
    FleetOpen, // WHAT: Fleet's open door. WHY: /api/version
    FleetKey, // WHAT: Fleet's keyed doors. WHY: only the maintenance key
}

/// Every route and its door. Paths as the router names them.
const TABLE: &[(&str, &str, Door)] = &[
    ("GET", "/health", Door::Open),
    ("GET", "/office/identity", Door::Open),
    ("POST", "/devices/request", Door::Open),
    ("GET", "/update/latest", Door::Open),
    ("GET", "/update/{version}/download", Door::Open),
    ("GET", "/devices/me", Door::Me),
    ("GET", "/devices", Door::Admin),
    ("POST", "/devices/{id}/approve", Door::Admin),
    ("POST", "/devices/{id}/refuse", Door::Admin),
    ("POST", "/devices/{id}/remove", Door::Admin),
    ("POST", "/devices/{id}/admin", Door::Admin),
    ("POST", "/join", Door::Join),
    ("GET", "/openapi.json", Door::Gated),
    ("POST", "/auth/token", Door::Gated),
    ("POST", "/setup", Door::Gated),
    ("GET", "/admin/joining", Door::Gated),
    ("POST", "/admin/joining", Door::Gated),
    ("GET", "/admin/invites", Door::Gated),
    ("POST", "/admin/invites", Door::Gated),
    ("DELETE", "/admin/invites/{code}", Door::Gated),
    ("GET", "/me", Door::Gated),
    ("POST", "/me/password", Door::Gated),
    ("POST", "/signout", Door::Gated),
    ("GET", "/me/keys", Door::Gated),
    ("POST", "/me/keys", Door::Gated),
    ("POST", "/keys/{id}/revoke", Door::Gated),
    ("GET", "/sets/{id}/markuplist", Door::Gated),
    ("GET", "/projects/{id}/markuplist", Door::Gated),
    ("GET", "/admin/updates", Door::Gated),
    ("POST", "/admin/updates", Door::Gated),
    ("GET", "/admin/fleet", Door::Gated),
    ("POST", "/admin/fleet", Door::Gated),
    ("GET", "/admin/remote", Door::Gated),
    ("POST", "/admin/remote", Door::Gated),
    ("DELETE", "/admin/remote", Door::Gated),
    ("GET", "/projects", Door::Gated),
    ("POST", "/projects", Door::Gated),
    ("GET", "/projects/{id}", Door::Gated),
    ("POST", "/projects/{id}/people", Door::Gated),
    ("GET", "/projects/{id}/people", Door::Gated),
    ("POST", "/projects/{id}/archive", Door::Gated),
    ("POST", "/projects/{id}/place", Door::Gated),
    ("GET", "/projects/{id}/stamp", Door::Gated),
    ("GET", "/projects/{id}/sets", Door::Gated),
    ("POST", "/sets", Door::Gated),
    ("GET", "/sets/{id}", Door::Gated),
    ("GET", "/sets/{id}/file", Door::Gated),
    ("POST", "/sets/{id}/copy", Door::Gated),
    ("GET", "/sets/{id}/sheets", Door::Gated),
    ("GET", "/sets/{id}/markups", Door::Gated),
    ("POST", "/sets/{id}/markups", Door::Gated),
    ("GET", "/sets/{id}/takeoff", Door::Gated),
    ("GET", "/sets/{id}/takeoff.csv", Door::Gated),
    ("GET", "/people", Door::Gated),
    ("POST", "/people", Door::Gated),
    ("POST", "/people/{id}/role", Door::Gated),
    ("POST", "/people/{id}/remove", Door::Gated),
    ("GET", "/chests", Door::Gated),
    ("POST", "/chests", Door::Gated),
    ("GET", "/chests/{id}/file", Door::Gated),
    ("GET", "/plugins", Door::Gated),
    ("POST", "/plugins", Door::Gated),
    ("GET", "/plugins/{id}/file", Door::Gated),
    ("POST", "/plugins/{id}/run", Door::Gated),
    ("POST", "/plugins/{id}/remove", Door::Gated),
    ("GET", "/notices", Door::Gated),
    ("POST", "/notices", Door::Gated),
    ("POST", "/notices/{id}/test", Door::Gated),
    ("POST", "/notices/{id}/remove", Door::Gated),
    ("GET", "/license", Door::Gated),
    ("POST", "/license", Door::Gated),
    ("GET", "/office", Door::Gated),
    ("POST", "/admin/citadel/forget-pin", Door::Gated),
    ("GET", "/archetype/jobs", Door::Gated),
    ("POST", "/archetype/jobs", Door::Gated),
    ("GET", "/archetype/jobs/{id}", Door::Gated),
    ("GET", "/archetype/jobs/{id}/files", Door::Gated),
    ("GET", "/archetype/jobs/{id}/history", Door::Gated),
    ("POST", "/archetype/jobs/{id}/checkout", Door::Gated),
    ("POST", "/archetype/jobs/{id}/checkin", Door::Gated),
    ("POST", "/archetype/jobs/{id}/release", Door::Gated),
    ("POST", "/archetype/jobs/{id}/takeback", Door::Gated),
    ("POST", "/archetype/files/missing", Door::Gated),
    ("PUT", "/archetype/files/{digest}", Door::Gated),
    ("GET", "/archetype/files/{digest}", Door::Gated),
    ("GET", "/cad/drawings", Door::Gated),
    ("POST", "/cad/drawings", Door::Gated),
    ("GET", "/cad/drawings/{id}", Door::Gated),
    ("GET", "/cad/drawings/{id}/file", Door::Gated),
    ("PUT", "/cad/drawings/{id}/file", Door::Gated),
    ("POST", "/cad/drawings/{id}/lock", Door::Gated),
    ("DELETE", "/cad/drawings/{id}/lock", Door::Gated),
    ("GET", "/cad/drawings/{id}/versions", Door::Gated),
    ("POST", "/cad/drawings/{id}/restore", Door::Gated),
    ("POST", "/cad/drawings/{id}/archive", Door::Gated),
    ("GET", "/audit", Door::Gated),
    ("GET", "/audit.csv", Door::Gated),
    ("POST", "/admin/pin", Door::Gated),
    ("POST", "/admin/releases", Door::Gated),
    ("POST", "/admin/releases/{version}/ready", Door::Gated),
    ("GET", "/groups", Door::Gated),
    ("POST", "/groups", Door::Gated),
    ("POST", "/groups/{id}/people", Door::Gated),
    ("GET", "/projects/{id}/folders", Door::Gated),
    ("POST", "/projects/{id}/folders", Door::Gated),
    ("GET", "/projects/{id}/files", Door::Gated),
    ("POST", "/files", Door::Gated),
    ("GET", "/files/{id}/file", Door::Gated),
    ("POST", "/files/{id}/folder", Door::Gated),
    ("POST", "/sets/{id}/folder", Door::Gated),
    ("GET", "/sets/{id}/signoffs", Door::Gated),
    ("POST", "/sets/{id}/signoffs", Door::Gated),
    ("GET", "/signoffs", Door::Gated),
    ("POST", "/signoffs/{id}/sign", Door::Gated),
    ("POST", "/signoffs/{id}/withdraw", Door::Gated),
    ("GET", "/signoffs/{id}/signature.png", Door::Gated),
];

/// Fleet's own routes and the assistant's, outside `/api/v1`.
const OUTSIDE_THE_API: &[(&str, &str, Door)] = &[
    ("GET", "/api/version", Door::FleetOpen), // WHAT: Fleet's version. WHY: open
    ("GET", "/api/admin/ops", Door::FleetKey), // WHAT: Fleet's routes. WHY: the maintenance key only
    ("GET", "/api/admin/logtail", Door::FleetKey), // WHAT: likewise. WHY: likewise
    ("GET", "/api/admin/presence", Door::FleetKey), // WHAT: likewise. WHY: likewise
    ("POST", "/api/admin/update", Door::FleetKey), // WHAT: likewise. WHY: likewise
    ("POST", "/api/admin/update/rollback", Door::FleetKey), // WHAT: likewise. WHY: likewise
    ("POST", "/mcp", Door::Gated), // WHAT: the assistant's endpoint. WHY: a device or a key
];

/// Every `.route("path", get(..).post(..))` a router's source declares, as
/// (method, path with `{id}` for `:id`), under `prefix`.
fn routed(source: &str, prefix: &str) -> Vec<(String, String)> {
    let mut found = Vec::new(); // WHAT: what was found. WHY: returned
    for piece in source.split(".route(").skip(1) { // WHAT: each route call. WHY: one at a time
        let Some(piece) = piece.trim_start().strip_prefix('"') else { continue }; // WHAT: the path's opening quote, on this line or the next. WHY: a route call written over several lines is still a route
        let Some((path, rest)) = piece.split_once('"') else { continue }; // WHAT: the path, and what follows. WHY: the methods are in what follows
        let rest = rest.trim_start().strip_prefix(',').unwrap_or(rest); // WHAT: past the comma. WHY: the methods follow it
        if !path.starts_with('/') { // WHAT: a path starts with a slash. WHY: below
            continue; // WHAT: not a route. WHY: a comment or a string that happens to match
        }
        let path = path.split('/').map(|s| s.strip_prefix(':').map_or(s.to_string(), |n| format!("{{{n}}}"))).collect::<Vec<_>>().join("/"); // WHAT: `:id` written `{id}`. WHY: as the table writes it
        let mut depth = 0i32; // WHAT: brackets open. WHY: the route call ends where its own bracket closes
        let mut call = String::new(); // WHAT: the rest of the route call. WHY: read for its methods
        for c in rest.chars() { // WHAT: each letter of the call. WHY: up to its closing bracket
            match c { // WHAT: the brackets. WHY: counted
                '(' => depth += 1, // WHAT: one opened. WHY: counted
                ')' if depth == 0 => break, // WHAT: the call's own close. WHY: the end
                ')' => depth -= 1, // WHAT: one closed. WHY: counted
                _ => {} // WHAT: anything else. WHY: kept
            }
            call.push(c); // WHAT: kept. WHY: read for its methods
        }
        for (word, method) in [("get(", "GET"), ("post(", "POST"), ("put(", "PUT"), ("delete(", "DELETE")] { // WHAT: each method word. WHY: get(..).post(..) and the rest
            if call.match_indices(word).any(|(i, _)| i == 0 || matches!(call.as_bytes()[i - 1], b' ' | b'.' | b':' | b',' | b'(')) { // WHAT: a method of this call, not part of a longer name. WHY: get_this( is not get(
                found.push((method.to_string(), format!("{prefix}{path}"))); // WHAT: one method of the route. WHY: each is its own door
            }
        }
    }
    found // WHAT: every route found. WHY: compared with the table
}

/// What a caller should meet at a door.
fn expected(door: Door, caller: Caller, method: &str, path: &str) -> Said {
    use Caller::*; // WHAT: the callers by name. WHY: the rules read plainly
    let sync_listed = hyperview_server::api::SYNC_ROUTES.iter().any(|(m, p)| *m == method && p.replace(":id", "{id}") == path); // WHAT: on the sync key's own list. WHY: the only routes a sync key reaches
    if caller == KeySync && !matches!(door, Door::FleetOpen | Door::FleetKey) && path != "/mcp" && !sync_listed { // WHAT: a sync key, on a View route off its list. WHY: below
        return Said::Key; // WHAT: a sync key off its list. WHY: refused before any door
    }
    match door { // WHAT: by the route's door. WHY: the table's
        Door::Open | Door::FleetOpen => Said::Past, // WHAT: everybody through. WHY: the route's own checks decide; what is running is no secret
        Door::FleetKey => if caller == Maintenance { Said::Past } else { Said::Fleet }, // WHAT: the maintenance key only. WHY: Fleet's channel
        Door::Me => match caller { // WHAT: the device's own state. WHY: any known device
            Waiting | Refused | Approved | Admin | Citadel => Said::Past, // WHAT: any known device. WHY: how it stands
            _ => Said::Gate, // WHAT: anybody else. WHY: no device, no answer
        },
        Door::Admin => match caller { // WHAT: the admin's door. WHY: guard 4
            Admin => Said::Past, // WHAT: an admin's device. WHY: guard 4
            Approved => Said::NotAdmin, // WHAT: approved, not an admin. WHY: guard 4
            _ => Said::Gate, // WHAT: anybody else. WHY: no approved device
        },
        Door::Gated | Door::Join => match caller { // WHAT: View's routes, and the join door. WHY: §5
            Approved | Admin | KeyAssistant | KeyIntegration | KeySync => Said::Past, // WHAT: an approved device, or a live key. WHY: keys are programs, held by their own purpose
            Outside | OldSessionOutside if door == Door::Join => Said::Past, // WHAT: a join from outside. WHY: §5: the join code stays a way in from outside
            _ => Said::Gate, // WHAT: anybody else. WHY: §5
        },
    }
}

/// The path with made-up ids in it.
fn filled(path: &str) -> String {
    path.replace("{id}", "x0").replace("{version}", "0.0.1").replace("{code}", "x0").replace("{digest}", &"0".repeat(64)) // WHAT: ids that name nothing. WHY: the door answers before the room is looked in
}

/// A key of `purpose` for a person, written into the database.
fn key_of(office: &Office, purpose: &str) -> String {
    let (key, digest) = auth::fresh_key(); // WHAT: a fresh key and its digest. WHY: the digest is stored, the key is sent
    let person = api::fresh_id("usr"); // WHAT: a person for it. WHY: a key belongs to someone
    office // WHAT: the database. WHY: written straight in
        .store()
        .with(|db| {
            db.execute("INSERT INTO people (id, name, email, password, role, created) VALUES (?1, 'Bot', ?2, 'x', 'admin', '2020-01-01T00:00:00Z')", params![person, format!("{person}@example.test")])?; // WHAT: the person, an admin. WHY: the key's purpose holds it, not the role
            db.execute("INSERT INTO keys (id, digest, person, purpose, name, made) VALUES (?1, ?2, ?3, ?4, 'a key', '2026-11-11T00:00:00Z')", params![api::fresh_id("key"), digest, person, purpose])?; // WHAT: the key, of this purpose. WHY: one per scope
            Ok(())
        })
        .unwrap();
    key // WHAT: the key. WHY: sent as a bearer
}

/// A session issued before devices began, written into the database.
fn old_session(office: &Office) -> String {
    let (token, hashed) = auth::new_token(); // WHAT: a session token and its hash. WHY: the hash is stored
    let person = api::fresh_id("usr"); // WHAT: a person for it. WHY: a session belongs to someone
    office // WHAT: the database. WHY: written straight in
        .store()
        .with(|db| {
            db.execute("INSERT INTO people (id, name, email, password, role, created) VALUES (?1, 'Sam', ?2, 'x', 'estimator', '2020-01-01T00:00:00Z')", params![person, format!("{person}@example.test")])?; // WHAT: the person. WHY: an estimator, from before devices
            db.execute("INSERT INTO sessions (token, person, issued, expires) VALUES (?1, ?2, '2026-01-01T00:00:00Z', '2099-01-01T00:00:00Z')", params![hashed, person])?; // WHAT: the session, issued before 0.7.0. WHY: a seat carried over
            Ok(())
        })
        .unwrap();
    token // WHAT: the token. WHY: sent as a bearer
}

#[test]
fn every_route_is_held_to_its_door_for_every_kind_of_caller() {
    // The table covers every route the router serves, read from the router's
    // own code, and nothing it doesn't.
    let served = routed(include_str!("../src/api.rs"), "/api/v1").into_iter().chain(routed(include_str!("../src/fleet.rs"), "")).chain(routed(include_str!("../src/mcp.rs"), "")).collect::<std::collections::BTreeSet<_>>(); // WHAT: every route the routers declare. WHY: a new route can't be missed
    let tabled: std::collections::BTreeSet<(String, String)> = TABLE.iter().map(|(m, p, _)| (m.to_string(), format!("/api/v1{p}"))).chain(OUTSIDE_THE_API.iter().map(|(m, p, _)| (m.to_string(), p.to_string()))).collect(); // WHAT: every route the table names. WHY: compared with the routers
    let missing: Vec<_> = served.difference(&tabled).collect(); // WHAT: routes served but not tabled. WHY: none may be
    assert!(missing.is_empty(), "routes the matrix doesn't know the door of: {missing:?}"); // WHAT: none missing. WHY: a new route can't slip through unheld
    let imaginary: Vec<_> = tabled.difference(&served).collect(); // WHAT: routes tabled but not served. WHY: none may be
    assert!(imaginary.is_empty(), "routes in the matrix the router doesn't serve: {imaginary:?}"); // WHAT: none imaginary. WHY: the table follows the router
    assert!(api::ROUTES.iter().all(|(m, p)| tabled.contains(&(m.to_string(), format!("/api/v1{p}")))), "every described route is in the matrix"); // WHAT: the described routes too. WHY: the openapi list is a subset

    let office = start(); // WHAT: the office, running. WHY: every knock goes there
    office.store().set_setting("device_claim_until", "2020-01-01T00:00:00Z").unwrap(); // WHAT: no claiming. WHY: devices are made by hand below
    let make = |seed: u8, state: &str, admin: bool, scope: &str| -> Arc<Signer> {
        let signer = Arc::new(Signer::new(DeviceKey::from_secret(&[seed; 32]))); // WHAT: a computer's key. WHY: one per kind of device
        office // WHAT: the database. WHY: written straight in
            .store()
            .with(|db| {
                Ok(db.execute(
                    "INSERT INTO devices (id, public_key, app, machine, os_user, state, admin, asked, changed, scope) VALUES (?1, ?2, 'view', 'PC', 'dana', ?3, ?4, '2026-11-12T00:00:00Z', '2026-11-12T00:00:00Z', ?5)",
                    params![signer.key().id(), signer.key().public_key_text(), state, i64::from(admin), scope],
                )?)
            })
            .unwrap(); // WHAT: a device in the state the matrix needs, written straight in. WHY: asking through the door is rate-limited per address, and is tested elsewhere
        signer // WHAT: the device's key. WHY: it signs
    };
    let waiting = make(1, "waiting", false, "all"); // WHAT: a waiting device. WHY: a caller
    let refused = make(2, "refused", false, "all"); // WHAT: a refused one. WHY: likewise
    let approved = make(3, "approved", false, "all"); // WHAT: an approved one, not an admin. WHY: likewise
    let admin = make(4, "approved", true, "all"); // WHAT: an admin's. WHY: likewise
    let citadel = make(5, "approved", false, "citadel"); // WHAT: one for the Citadel cell only. WHY: likewise
    let assistant = key_of(&office, "assistant"); // WHAT: an assistant key. WHY: likewise
    let integration = key_of(&office, "integration"); // WHAT: an integration key. WHY: likewise
    let sync = key_of(&office, "sync"); // WHAT: a sync key. WHY: likewise
    let session = old_session(&office); // WHAT: a session from before 0.7.0. WHY: likewise
    let outside = ("cf-connecting-ip".to_string(), "203.0.113.9".to_string()); // WHAT: the tunnel's mark. WHY: from outside
    let bearer = |token: &str| ("Authorization".to_string(), format!("Bearer {token}")); // WHAT: a bearer header. WHY: keys and sessions

    let mut wrong = Vec::new(); // WHAT: every door that said the wrong thing. WHY: all reported at once
    let all: Vec<(&str, String, Door)> = TABLE.iter().map(|(m, p, d)| (*m, format!("/api/v1{}", filled(p)), *d)).chain(OUTSIDE_THE_API.iter().map(|(m, p, d)| (*m, (*p).to_string(), *d))).collect(); // WHAT: every route, with ids filled in. WHY: to knock on
    for (route, (method, path, door)) in TABLE.iter().map(|(m, p, _)| (*m, *p)).chain(OUTSIDE_THE_API.iter().map(|(m, p, _)| (*m, *p))).zip(all.iter()) { // WHAT: each route. WHY: its table path and its filled one
        for caller in CALLERS { // WHAT: each caller. WHY: every kind
            let (signer, headers): (Option<&Signer>, Vec<(String, String)>) = match caller { // WHAT: how the caller knocks. WHY: a signature, a header, or nothing
                Caller::Nobody => (None, vec![]),
                Caller::Waiting => (Some(&waiting), vec![]),
                Caller::Refused => (Some(&refused), vec![]),
                Caller::Approved => (Some(&approved), vec![]),
                Caller::Admin => (Some(&admin), vec![]),
                Caller::Citadel => (Some(&citadel), vec![]),
                Caller::KeyAssistant => (None, vec![bearer(&assistant)]),
                Caller::KeyIntegration => (None, vec![bearer(&integration)]),
                Caller::KeySync => (None, vec![bearer(&sync)]),
                Caller::OldSessionInside => (None, vec![bearer(&session)]),
                Caller::OldSessionOutside => (None, vec![bearer(&session), outside.clone()]),
                Caller::Maintenance => (None, vec![(hyperview_server::fleet::KEY_HEADER.to_string(), MAINTENANCE.to_string())]),
                Caller::Outside => (None, vec![outside.clone()]),
            };
            let body: &[u8] = if *method == "GET" || *method == "DELETE" { b"" } else { b"{}" }; // WHAT: no body on a GET, an empty object otherwise. WHY: the door answers before the body is used
            let (status, word) = office.knock(method, path, body, signer, &headers); // WHAT: the knock. WHY: its status and word
            let said = match (status, word.as_str()) {
                (401, "device_unknown") => Said::Gate, // WHAT: the gate. WHY: §4
                (403, "not_an_admin") => Said::NotAdmin, // WHAT: the admin's door. WHY: guard 4
                (403, "key_not_allowed") if caller == Caller::KeySync => Said::Key, // WHAT: the sync key's own door. WHY: the routes behind it also refuse other keys with the same word, which is the room, not the door
                (401 | 404, _) if matches!(door, Door::FleetKey) && caller != Caller::Maintenance => Said::Fleet, // WHAT: Fleet's door. WHY: no maintenance key
                _ => Said::Past, // WHAT: through. WHY: to the room behind
            }; // WHAT: which door answered. WHY: compared with the table
            let want = expected(*door, caller, method, route.1); // WHAT: what the table says. WHY: compared
            if said != want { // WHAT: a wrong answer. WHY: kept
                wrong.push(format!("{method} {} as {caller:?}: {status} {word} (said {said:?}, should be {want:?})", route.1)); // WHAT: its line. WHY: all reported together
            }
        }
    }
    assert!(wrong.is_empty(), "{} wrong of {} knocks:\n{}", wrong.len(), all.len() * CALLERS.len(), wrong.join("\n")); // WHAT: every door right. WHY: the property
}
