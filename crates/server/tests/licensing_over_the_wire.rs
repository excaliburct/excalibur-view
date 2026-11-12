//! Every way a licence reaches an office server, run for real over a socket,
//! with licences this test really signed (a made-up key, given to the server
//! the way `Config::license_keys_for_tests` allows and nothing else does).
//!
//! What is held here: a new office runs a 30-day trial and then only sharing
//! new work pauses; an administrator adds a licence from a seat and nobody
//! else can; a licence from a key the build doesn't know, or with one field
//! changed, is refused; the user limit stops the next person and nobody
//! already in; a licence whose updates ended before this build pauses
//! sharing; a founding office is never asked; and a licence file dropped in
//! the data folder is picked up, and a restart never undoes a later one.

use std::sync::Arc; // WHAT: shared ownership of the server. WHY: the router takes it as an Arc.

use ed25519_dalek::{Signer, SigningKey}; // WHAT: Ed25519 signing. WHY: the test signs its own licences.
use hub::license::{Edition, License}; // WHAT: the licence file. WHY: made here exactly as the publishing tools make one.
use hyperview_server::api::{self, Server}; // WHAT: the router and the server state. WHY: the tests run the real thing.
use hyperview_server::{auth, Config, Store}; // WHAT: password hashing, configuration and storage. WHY: to set up an office with people in it.
use rusqlite::params; // WHAT: bound SQL parameters. WHY: people are put in the database directly, as the other wire tests do.
use serde_json::{json, Value}; // WHAT: JSON values. WHY: requests and answers.

const PASSWORD: &str = "brace-gusset-purlin-shim-42"; // WHAT: everybody's password here. WHY: long enough for the server's rules.
const KEY_NAME: &str = "licence-test-key"; // WHAT: the made-up key's name. WHY: a licence names the key that signed it.

/// The made-up licence key. The real ones are never in a repository.
fn issuer() -> SigningKey {
    SigningKey::from_bytes(&[11u8; 32]) // WHAT: a fixed seed. WHY: the same key every run, and nobody's real key.
}

/// A licence, signed by `signing`, as text.
fn signed(signing: &SigningKey, id: &str, edition: Edition, users: u32, through: &str) -> String {
    let mut license = License {
        id: id.into(),                       // WHAT: the licence's id. WHY: a renewal keeps it.
        company: "Example Steel, Inc.".into(), // WHAT: who it is for. WHY: printed on it.
        edition,                             // WHAT: office or sealed. WHY: the two editions.
        users,                               // WHAT: the user limit, 0 none. WHY: under test.
        updates_through: through.into(),     // WHAT: the last day of updates. WHY: under test.
        issued: "2026-11-10".into(),         // WHAT: the day it was signed. WHY: every licence has one.
        note: String::new(),                 // WHAT: no note. WHY: an ordinary licence.
        modules: Vec::new(), // WHAT: no Office Server parts. WHY: View alone
        key: KEY_NAME.into(),                // WHAT: the key's name. WHY: the server looks it up by name.
        signature: String::new(),            // WHAT: none yet. WHY: signed just below.
    };
    license.signature = hyperview_server::store::hex(&signing.sign(&license.signing_payload()).to_bytes()); // WHAT: the signature over the signed text. WHY: what every server checks.
    serde_json::to_string_pretty(&license).unwrap() // WHAT: as the file is written. WHY: the server is handed the file's text.
}

/// A folder that is gone when the test is.
struct Scratch(std::path::PathBuf); // WHAT: the server's data folder. WHY: each test gets its own.

impl Scratch {
    fn new() -> Scratch {
        let n: u64 = rand::random(); // WHAT: a random number. WHY: a folder name no other test is using.
        let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("hyperview-licensing-{n:016x}")); // WHAT: the folder. WHY: under the build's own scratch folder, not one shared with other users
        std::fs::create_dir_all(&path).expect("a scratch directory"); // WHAT: made. WHY: the store writes into it.
        Scratch(path) // WHAT: handed back. WHY: dropped at the end of the test.
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0); // WHAT: the folder removed. WHY: tests leave nothing behind.
    }
}

/// A running office server on a folder.
struct Office {
    base: String,                       // WHAT: http://127.0.0.1:port. WHY: every request goes there.
    _runtime: tokio::runtime::Runtime, // WHAT: the async runtime. WHY: dropping it stops the server.
}

/// Who is on a new office, and when they were added.
fn people(folder: &std::path::Path, added: &str) {
    let config = Config { data: folder.to_path_buf(), ..Config::default() }; // WHAT: the folder's database. WHY: the people go in before the server opens.
    let store = Store::open(&config.database(), &config.blobs()).expect("a store"); // WHAT: the database. WHY: as the server opens it.
    let hashed = auth::hash_password(PASSWORD).unwrap(); // WHAT: the password, hashed once. WHY: Argon2 is slow on purpose; both share it.
    store
        .with(|db| {
            for (id, name, email, role) in [
                ("usr_admin", "Sam", "sam@example.com", "admin"), // WHAT: the administrator. WHY: adds the licence.
                ("usr_estimator", "Eve", "eve@example.com", "estimator"), // WHAT: an estimator. WHY: may not.
            ] {
                db.execute(
                    "INSERT INTO people (id, name, email, password, role, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", // WHAT: one person. WHY: the office's accounts.
                    params![id, name, email, hashed, role, added], // WHAT: added on `added`. WHY: before 2026-10-01 is a founding office, after is a trial.
                )?;
            }
            Ok(())
        })
        .expect("two people"); // WHAT: both in. WHY: every test needs them.
}

/// Starts the server on `folder`, trusting the test key for licences.
fn serve(folder: &std::path::Path) -> Office {
    let config = Config {
        name: "Example Steel".into(),                     // WHAT: the office's name. WHY: any name will do.
        data: folder.to_path_buf(),                       // WHAT: where it keeps everything. WHY: the scratch folder, kept across a restart.
        listen: "127.0.0.1:0".parse().unwrap(),           // WHAT: any free port. WHY: tests run side by side.
        license_keys_for_tests: vec![(KEY_NAME.into(), issuer().verifying_key().to_bytes())], // WHAT: the test key. WHY: as a real server trusts its built-in ones
        ..Config::default()                               // WHAT: everything else as shipped. WHY: the server as an office runs it.
    };
    let store = Store::open(&config.database(), &config.blobs()).expect("a store"); // WHAT: the database. WHY: the server's own.
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("a runtime"); // WHAT: a small runtime. WHY: enough for one test.
    let listener = runtime.block_on(tokio::net::TcpListener::bind(config.listen)).expect("a socket"); // WHAT: the listening socket. WHY: its port is the address.
    let address = listener.local_addr().expect("an address"); // WHAT: the port it got. WHY: for the base address.
    let app = api::router(Arc::new(Server::new(store, config))); // WHAT: the real router; Server::new runs the licence's start. WHY: the data folder is read there.
    runtime.spawn(async move {
        let _ = axum::serve(listener, app).await; // WHAT: serving until the runtime is dropped. WHY: the test talks to it over the socket.
    });
    Office { base: format!("http://{address}"), _runtime: runtime } // WHAT: the running office. WHY: handed to the test.
}

/// A session for one of the office's people.
fn session(office: &Office, email: &str) -> String {
    let mut client = hub::Client::new(&office.base); // WHAT: the program's own client. WHY: signs in the way the desktop does.
    client.sign_in(email, PASSWORD).expect("sign in"); // WHAT: signed in. WHY: everything here is asked as somebody.
    client.token().expect("a token").to_string() // WHAT: the session token. WHY: used as a bearer below.
}

/// A request with `token` as its bearer: the status and the answer's JSON.
fn ask(office: &Office, method: &str, path: &str, token: &str, body: Option<Value>) -> (u16, Value) {
    let request = ureq::request(method, &format!("{}/api/v1{path}", office.base)).set("Authorization", &format!("Bearer {token}")); // WHAT: the request with its bearer. WHY: every route here is asked as somebody.
    let answer = match body {
        Some(body) => request.send_json(body), // WHAT: with a JSON body. WHY: the writes take one.
        None => request.call(),                // WHAT: without one. WHY: the reads.
    };
    match answer {
        Ok(r) => (r.status(), r.into_json().unwrap_or(Value::Null)), // WHAT: a success and its JSON. WHY: answers are checked.
        Err(ureq::Error::Status(code, r)) => (code, r.into_json().unwrap_or(Value::Null)), // WHAT: a refusal and its problem. WHY: the error name is checked.
        Err(e) => panic!("{method} {path}: {e}"), // WHAT: nothing answered. WHY: a broken test, not a refusal.
    }
}

/// Adds a licence as `token`, as Studio > Office > License does.
fn add(office: &Office, token: &str, text: &str) -> (u16, Value) {
    ask(office, "POST", "/license", token, Some(json!({ "license": text }))) // WHAT: the whole file's text. WHY: what the seat sends
}

/// A new job: the simplest piece of new work there is.
fn new_job(office: &Office, token: &str, number: &str) -> (u16, Value) {
    ask(office, "POST", "/projects", token, Some(json!({ "number": number, "name": "Job" }))) // WHAT: a project made. WHY: sharing new work
}

/// Sets when licensing began on this office, straight in its database.
fn founded(folder: &std::path::Path) {
    let db = rusqlite::Connection::open(folder.join("hyperview.sqlite")).unwrap(); // WHAT: the office's database. WHY: as an earlier version left it
    db.execute("INSERT OR REPLACE INTO settings (key, value) VALUES ('founding_install', '2026-09-01T00:00:00Z')", []).unwrap(); // WHAT: marked founding. WHY: an earlier version decided it
}

fn began(folder: &std::path::Path, when: &str) {
    let db = rusqlite::Connection::open(folder.join("hyperview.sqlite")).unwrap(); // WHAT: the office's database. WHY: the clock can't be moved, the start can.
    db.execute("UPDATE settings SET value = ?1 WHERE key = 'licensing_began'", params![when]).unwrap(); // WHAT: the trial's start. WHY: to end the trial
}

#[test]
fn a_new_office_runs_a_thirty_day_trial_then_only_sharing_pauses_until_an_administrator_adds_a_licence() {
    let scratch = Scratch::new(); // WHAT: a new office's folder. WHY: nothing from another test.
    people(&scratch.0, "2026-11-10T00:00:00Z"); // WHAT: its people, added after licensing began. WHY: not a founding office
    let office = serve(&scratch.0);
    let sam = session(&office, "sam@example.com"); // WHAT: the administrator. WHY: adds the licence.
    let eve = session(&office, "eve@example.com"); // WHAT: an estimator. WHY: may not.

    let (_, standing) = ask(&office, "GET", "/license", &sam, None);
    assert_eq!((standing["state"].as_str(), standing["trial_days_left"].as_u64(), standing["sharing"].as_bool()), (Some("trial"), Some(30), Some(true)), "{standing}"); // WHAT: day 1 of 30, everything on. WHY: the trial
    assert!(standing["message"].as_str().unwrap().contains("30 days left"), "{standing}"); // WHAT: the administrator is told. WHY: so they can act
    let (_, theirs) = ask(&office, "GET", "/license", &eve, None);
    assert_eq!(theirs["message"], "", "nobody else is bothered during a trial: {theirs}");
    assert_eq!(new_job(&office, &eve, "1001").0, 200, "everything works in the trial");

    began(&scratch.0, "2026-01-01T00:00:00Z"); // WHAT: the trial began long ago. WHY: now it has ended
    let (_, ended) = ask(&office, "GET", "/license", &sam, None);
    assert_eq!((ended["state"].as_str(), ended["sharing"].as_bool()), (Some("unlicensed"), Some(false)), "{ended}");
    let (status, refused) = new_job(&office, &eve, "1002");
    assert_eq!((status, refused["error"].as_str()), (403, Some("license_needed")), "new work pauses, in words, never a sign-out: {refused}");
    assert_eq!(ask(&office, "GET", "/projects", &eve, None).0, 200, "what is there still reads");
    let (_, told) = ask(&office, "GET", "/license", &eve, None);
    assert_eq!(told["message"], "Sharing is paused. Your administrator has been told.");

    let licence = signed(&issuer(), "evl_trial0001", Edition::Office, 10, "2099-01-01"); // WHAT: a real licence. WHY: what the customer is sent
    assert_eq!(add(&office, &eve, &licence).0, 403, "an estimator cannot add a licence");
    let (_, made) = ask(&office, "POST", "/me/keys", &sam, Some(json!({ "purpose": "integration", "name": "Other program" }))); // WHAT: the administrator's own key. WHY: a key never administers
    let key = made["key"].as_str().expect("a key").to_string();
    assert_eq!(add(&office, &key, &licence).0, 403, "not even the administrator's key can add a licence");
    assert_eq!(ask(&office, "GET", "/license", &sam, None).1["state"], "unlicensed", "neither changed anything");

    let (status, licensed) = add(&office, &sam, &licence);
    assert_eq!(status, 200, "{licensed}");
    assert_eq!((licensed["state"].as_str(), licensed["users_allowed"].as_u64(), licensed["license_id"].as_str(), licensed["sharing"].as_bool()), (Some("licensed"), Some(10), Some("evl_trial0001"), Some(true)), "{licensed}");
    assert_eq!(new_job(&office, &eve, "1003").0, 200, "sharing is back on");
    let (_, theirs) = ask(&office, "GET", "/license", &eve, None);
    assert!(theirs["license_id"].is_null(), "only an administrator is told the licence id: {theirs}");
}

#[test]
fn a_licence_from_a_key_this_build_does_not_trust_or_with_one_field_changed_is_refused() {
    let scratch = Scratch::new();
    people(&scratch.0, "2026-11-10T00:00:00Z"); // WHAT: an office in its trial. WHY: so a licence would change something
    let office = serve(&scratch.0);
    let sam = session(&office, "sam@example.com");

    let stranger = SigningKey::from_bytes(&[12u8; 32]); // WHAT: a key the server was never given. WHY: anybody can make one
    let mut theirs: Value = serde_json::from_str(&signed(&stranger, "evl_x", Edition::Office, 0, "forever")).unwrap(); // WHAT: a licence a stranger signed. WHY: anybody can sign one
    theirs["key"] = json!("someone-else"); // WHAT: named for their own key. WHY: a key the build doesn't know (the name isn't in the signed text)
    let (status, why) = add(&office, &sam, &theirs.to_string());
    assert_eq!(status, 400, "{why}");
    assert!(why["message"].as_str().unwrap().contains("a key this version does not know"), "{why}");
    let (status, why) = add(&office, &sam, &signed(&stranger, "evl_x", Edition::Office, 0, "forever")); // WHAT: the stranger's licence under our key's name. WHY: a forgery that names a key the server does trust
    assert_eq!(status, 400, "{why}");
    assert!(why["message"].as_str().unwrap().contains("does not check out"), "{why}");

    let good = signed(&issuer(), "evl_one", Edition::Office, 3, "2027-11-10"); // WHAT: a real licence. WHY: changed one field at a time below
    for (field, to) in [
        ("id", json!("evl_two")),
        ("company", json!("Somebody Else LLC")),
        ("edition", json!("sealed")),
        ("users", json!(30)),
        ("users", json!(0)),
        ("updates_through", json!("forever")),
        ("updates_through", json!("2099-11-10")),
        ("issued", json!("2026-11-11")),
        ("note", json!("Founding customer")),
    ] {
        let mut changed: Value = serde_json::from_str(&good).unwrap(); // WHAT: the good licence. WHY: one field changed
        changed[field] = to.clone();
        let (status, why) = add(&office, &sam, &changed.to_string());
        assert_eq!(status, 400, "{field} changed to {to}: {why}");
        assert!(why["message"].as_str().unwrap().contains("does not check out"), "{field}: {why}");
    }
    let mut renamed: Value = serde_json::from_str(&good).unwrap();
    renamed["key"] = json!("mesafab-2026"); // WHAT: the signature claimed for the real release key. WHY: a name the build does trust
    assert_eq!(add(&office, &sam, &renamed.to_string()).0, 400, "a licence that names a trusted key it wasn't signed with");
    assert_eq!(ask(&office, "GET", "/license", &sam, None).1["state"], "trial", "nothing refused changed anything");
    assert_eq!(add(&office, &sam, &good).0, 200, "and the unchanged one is taken");
}

#[test]
fn the_user_limit_stops_the_next_person_and_never_anybody_already_in() {
    let scratch = Scratch::new();
    people(&scratch.0, "2026-11-10T00:00:00Z"); // WHAT: two people. WHY: a two-user licence is then full
    let office = serve(&scratch.0);
    let sam = session(&office, "sam@example.com");
    assert_eq!(add(&office, &sam, &signed(&issuer(), "evl_two", Edition::Office, 2, "2099-01-01")).0, 200);
    let (_, standing) = ask(&office, "GET", "/license", &sam, None);
    assert!(standing["message"].as_str().unwrap().contains("All 2 users on your license are in use"), "the administrator is told: {standing}");

    let third = json!({ "name": "Tom", "email": "tom@example.com", "password": PASSWORD, "role": "estimator" }); // WHAT: one person too many. WHY: under test
    let (status, refused) = ask(&office, "POST", "/people", &sam, Some(third.clone()));
    assert_eq!((status, refused["error"].as_str()), (403, Some("license_users")), "{refused}");
    assert_eq!(ask(&office, "POST", "/admin/joining", &sam, Some(json!({ "how": "open" }))).0, 200); // WHAT: joining opened. WHY: the other way a person is added
    let joined = ureq::post(&format!("{}/api/v1/join", office.base)).send_json(json!({ "code": "", "name": "Tom", "email": "tom@example.com", "password": PASSWORD })); // WHAT: Tom joins himself. WHY: must be held to the same limit
    match joined {
        Err(ureq::Error::Status(403, r)) => assert_eq!(r.into_json::<Value>().unwrap()["error"], "license_users"),
        other => panic!("joining past the limit was not refused: {other:?}"),
    }
    let eve = session(&office, "eve@example.com"); // WHAT: somebody already on it signs in again. WHY: nobody is turned away
    assert_eq!(new_job(&office, &eve, "2001").0, 200, "and works as before");

    assert_eq!(add(&office, &sam, &signed(&issuer(), "evl_two", Edition::Office, 3, "2099-01-01")).0, 200); // WHAT: a third seat bought. WHY: then there is room
    assert_eq!(ask(&office, "POST", "/people", &sam, Some(third)).0, 200, "room for the third person now");
}

#[test]
fn a_licence_whose_updates_ended_before_this_build_pauses_sharing_and_says_both_ways_out() {
    let scratch = Scratch::new();
    people(&scratch.0, "2026-11-10T00:00:00Z");
    let office = serve(&scratch.0);
    let sam = session(&office, "sam@example.com");
    let (status, standing) = add(&office, &sam, &signed(&issuer(), "evl_old", Edition::Office, 5, "2026-01-31")); // WHAT: updates ended before this build was made. WHY: a newer server put in by hand
    assert_eq!(status, 200, "the licence itself is good: {standing}");
    assert_eq!((standing["state"].as_str(), standing["sharing"].as_bool()), (Some("licensed"), Some(false)), "{standing}");
    let message = standing["message"].as_str().unwrap();
    assert!(message.contains("Jan 31, 2026") && message.contains("renew") && message.contains("put back"), "{message}");
    assert_eq!(new_job(&office, &sam, "3001").1["error"], "license_needed", "new work pauses");
    assert_eq!(ask(&office, "GET", "/projects", &sam, None).0, 200, "reading never does");
}

#[test]
fn an_earlier_versions_founding_mark_no_longer_counts_and_its_signed_founding_licence_does() {
    // WHAT: audit re-check, 2026-11-11: the unsigned mark could be had through an older release. WHY: founding is a signed licence only
    let scratch = Scratch::new();
    people(&scratch.0, "2026-09-01T00:00:00Z");
    let office = serve(&scratch.0);
    let sam = session(&office, "sam@example.com");
    founded(&scratch.0); // WHAT: marked founding by an earlier version. WHY: the route
    began(&scratch.0, "2020-01-01T00:00:00Z"); // WHAT: any trial long over. WHY: the mark no longer saves it
    let (_, standing) = ask(&office, "GET", "/license", &sam, None);
    assert_eq!((standing["state"].as_str(), standing["sharing"].as_bool()), (Some("unlicensed"), Some(false)), "{standing}");
    let (status, standing) = add(&office, &sam, &signed(&issuer(), "evl_f", Edition::Office, 0, "forever")); // WHAT: its signed founding licence. WHY: what Mesa Fab, Arc Valley and LatCon are given
    assert_eq!(status, 200, "{standing}");
    assert_eq!((standing["state"].as_str(), standing["sharing"].as_bool()), (Some("licensed"), Some(true)), "{standing}");
    assert_eq!(new_job(&office, &sam, "4001").0, 200);
}

#[test]
fn people_from_before_licensing_no_longer_make_an_office_founding_and_a_signed_founding_licence_does() {
    // WHAT: audit, 2026-11-11: a clock set back at the first start made any server founding. WHY: founding comes only from a signed licence now
    let scratch = Scratch::new();
    people(&scratch.0, "2026-09-01T00:00:00Z"); // WHAT: people dated before licensing, as a set-back clock would date them. WHY: the attack
    let office = serve(&scratch.0);
    let sam = session(&office, "sam@example.com");
    began(&scratch.0, "2020-01-01T00:00:00Z"); // WHAT: the trial long over. WHY: nothing founding saves it now
    let (_, standing) = ask(&office, "GET", "/license", &sam, None);
    assert_eq!((standing["state"].as_str(), standing["sharing"].as_bool()), (Some("unlicensed"), Some(false)), "{standing}");
    let (status, standing) = add(&office, &sam, &signed(&issuer(), "evl_founding", Edition::Office, 0, "forever")); // WHAT: the signed founding licence: every seat, every update. WHY: how a founding office is set up now
    assert_eq!(status, 200, "{standing}");
    assert_eq!((standing["state"].as_str(), standing["sharing"].as_bool()), (Some("licensed"), Some(true)), "{standing}");
    assert_eq!(new_job(&office, &sam, "4002").0, 200, "and new work goes ahead");
}

#[test]
fn a_licence_file_in_the_data_folder_is_picked_up_at_start_and_a_restart_never_undoes_a_renewal() {
    let scratch = Scratch::new();
    people(&scratch.0, "2026-11-10T00:00:00Z");
    std::fs::write(scratch.0.join("Example Steel, Inc.evlicense"), signed(&issuer(), "evl_folder", Edition::Office, 3, "2099-01-01")).unwrap(); // WHAT: the emailed file, dropped in the data folder. WHY: the hands-off way in
    let office = serve(&scratch.0);
    let sam = session(&office, "sam@example.com");
    let (_, standing) = ask(&office, "GET", "/license", &sam, None);
    assert_eq!((standing["state"].as_str(), standing["users_allowed"].as_u64()), (Some("licensed"), Some(3)), "picked up at start: {standing}");

    assert_eq!(add(&office, &sam, &signed(&issuer(), "evl_folder", Edition::Office, 8, "2099-01-01")).0, 200); // WHAT: five more seats, added from a seat. WHY: the old file is still in the folder
    drop(office); // WHAT: the server stops. WHY: and starts again below, as after a reboot
    let office = serve(&scratch.0);
    let sam = session(&office, "sam@example.com");
    let (_, standing) = ask(&office, "GET", "/license", &sam, None);
    assert_eq!(standing["users_allowed"].as_u64(), Some(8), "a restart must not put the old seats back: {standing}");
}
