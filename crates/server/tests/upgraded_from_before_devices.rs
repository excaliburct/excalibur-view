//! An office's server upgraded from 0.6.31 to this version, end to end
//! (docs/OFFICE-ENROLLMENT.md §3a). The database is the one 0.6.31 made
//! (tests/fixtures/office-0.6.31.sql), with its people, a seat's session and
//! the next release already on it. Then: the seats from before 0.7.0 keep
//! working through the upgrade grace, a new computer claims the admin seat,
//! an existing account's sign-in approves a waiting computer, the old seat
//! fetches its update without being signed in or approved, and once the
//! grace is over the device gate holds again. Beside it, the offices the
//! grace must leave alone: a fresh install, and one already on 0.7.1 with
//! its admin set.

use std::sync::Arc;

use hub::device::{DeviceKey, Signer};
use hyperview_server::{auth, devices, Config, Store};
use rusqlite::params;

const PASSWORD: &str = "brace-gusset-purlin-shim-42"; // WHAT: everybody's password. WHY: long enough for the server's rule
const OWNER: &str = "owner@mesa.example"; // WHAT: the office's administrator from before 0.7.0. WHY: an existing admin account
const DANA: &str = "dana@mesa.example"; // WHAT: an estimator from before 0.7.0. WHY: an existing ordinary account
const NEWCOMER: &str = "new@mesa.example"; // WHAT: an account made after the upgrade. WHY: the grace is not for it

/// The server as an office runs it, over a data folder made before it started.
struct Office {
    data: std::path::PathBuf, // WHAT: its data folder. WHY: the tests read and write its database
    port: u16,                // WHAT: its port. WHY: HTTPS and plain HTTP answer on it
    pin: String, // WHAT: its certificate's pin. WHY: a 0.7 computer's proofs are bound to it
    _runtime: tokio::runtime::Runtime, // WHAT: the server's runtime. WHY: kept alive for the test
}

/// A fresh data folder.
fn folder(name: &str) -> std::path::PathBuf {
    let n: u64 = rand::random(); // WHAT: a random number. WHY: tests run side by side
    std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{n:016x}"))
    // WHAT: the folder. WHY: under the build's own temporary folder, never one another account shares
    // WHAT: under the build's own folder. WHY: never one another account shares
}

/// The configuration an office's server runs with, over `data`.
fn config(data: &std::path::Path) -> Config {
    Config {
        name: "Mesa Fab".into(), // WHAT: the office's name. WHY: as Mesa's server is called
        data: data.to_path_buf(), // WHAT: the data folder. WHY: the one the test made
        listen: "127.0.0.1:0".parse().unwrap(), // WHAT: any free port on this computer. WHY: tests run side by side
        tls: true, // WHAT: HTTPS, with plain HTTP on the same port. WHY: as 0.7.x serves; seats from before 0.7.0 speak plain HTTP
        devices: hyperview_server::config::Devices::Required, // WHAT: devices required. WHY: as the real Office Server runs
        ..Config::default() // WHAT: the rest as shipped. WHY: only what the test needs is changed
    }
}

/// The database 0.6.31 left: its schema, two people from March, a session
/// one of its seats still holds, and the next release on offer.
fn as_0_6_31_left_it(data: &std::path::Path) -> String {
    std::fs::create_dir_all(data).unwrap(); // WHAT: the data folder made. WHY: the database goes in it
    let db = rusqlite::Connection::open(config(data).database()).unwrap(); // WHAT: where the server reads it. WHY: the same file it will open
    db.execute_batch(include_str!("fixtures/office-0.6.31.sql"))
        .unwrap(); // WHAT: 0.6.31's own schema. WHY: the upgrade under test starts from it, not from this version's
    let hashed = auth::hash_password(PASSWORD).unwrap(); // WHAT: the password as stored. WHY: the old seats sign in with it
    db.execute("INSERT INTO people (id, name, email, password, role, created) VALUES ('usr_owner', 'Owner', ?1, ?2, 'admin', '2026-03-01T09:00:00Z')", params![OWNER, hashed]).unwrap(); // WHAT: the owner, made in March. WHY: an administrator account older than the device list
    db.execute("INSERT INTO people (id, name, email, password, role, created) VALUES ('usr_dana', 'Dana', ?1, ?2, 'estimator', '2026-03-02T09:00:00Z')", params![DANA, hashed]).unwrap(); // WHAT: an estimator, made in March. WHY: an ordinary account older than the device list
    let (token, digest) = auth::new_token(); // WHAT: a session a 0.6.30 seat holds. WHY: it keeps working through the grace without signing in again
    db.execute("INSERT INTO sessions (token, person, issued, expires) VALUES (?1, 'usr_dana', ?2, '2099-01-01T00:00:00Z')", params![digest, now_text(-3)]).unwrap(); // WHAT: Dana's seat's session, stored by its digest. WHY: as 0.6.31 keeps it
    token // WHAT: the session's own token. WHY: the old seat sends it
}

/// Starts this version over `data`.
fn start(data: &std::path::Path) -> Office {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (told, heard) = std::sync::mpsc::channel(); // WHAT: a way to hear the port. WHY: the server picks a free one
    let config = config(data); // WHAT: the office's configuration. WHY: moved into the server's task
    runtime.spawn(async move {
        let _ = hyperview_server::run::serve(config, std::future::pending(), move |serving| {
            let _ = told.send(serving.address.port()); // WHAT: the port told. WHY: the test connects to it
        })
        .await; // WHAT: served until the test ends. WHY: the runtime is dropped with the office
    });
    let port = heard
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the server started");
    let cert =
        std::fs::read_to_string(data.join(hyperview_server::tls::CERT)).expect("its certificate");
    let pin = hyperview_server::tls::pin(&cert).unwrap(); // WHAT: its pin. WHY: a 0.7 computer's client is held to it
    Office {
        data: data.to_path_buf(), // WHAT: the data folder. WHY: the one the test made
        port,
        pin,
        _runtime: runtime,
    }
}

fn now_text(offset_days: i64) -> String {
    (time::OffsetDateTime::now_utc() + time::Duration::days(offset_days))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap() // WHAT: the text. WHY: a time that can't be written is a broken test
}

fn parsed(text: &str) -> time::OffsetDateTime {
    time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339).unwrap()
    // WHAT: the time read. WHY: the settings keep RFC 3339
}

impl Office {
    fn store(&self) -> Store {
        let config = config(&self.data); // WHAT: the same data folder. WHY: the same database
        Store::open(&config.database(), &config.blobs()).unwrap() // WHAT: the store, opened beside the running server. WHY: the test reads and writes what the server keeps
    }

    fn setting(&self, key: &str) -> Option<String> {
        self.store().setting(key).unwrap() // WHAT: the setting, or none. WHY: checked by the tests
    }

    /// A request as a seat from before 0.7.0 sends it: plain HTTP, no
    /// device, a session or nothing. Its status and JSON.
    fn old_seat(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<serde_json::Value>,
        extra: &[(&str, &str)],
    ) -> (u16, serde_json::Value) {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(15))
            .build();
        let mut request = agent
            .request(method, &format!("http://127.0.0.1:{}{path}", self.port))
            .set("User-Agent", "excalibur-hyperview/0.6.31"); // WHAT: plain HTTP, as 0.6.31 connects. WHY: it knows nothing of the pin
        if let Some(token) = token {
            request = request.set("Authorization", &format!("Bearer {token}")); // WHAT: the session. WHY: as an old seat sends it
        }
        for (k, v) in extra {
            request = request.set(k, v); // WHAT: each extra header. WHY: how a request from outside is marked
        }
        let answer = match body {
            Some(body) => request.send_json(body),
            None => request.call(),
        };
        match answer {
            Ok(r) => (r.status(), r.into_json().unwrap_or(serde_json::Value::Null)),
            Err(ureq::Error::Status(code, r)) => {
                (code, r.into_json().unwrap_or(serde_json::Value::Null))
            }
            Err(e) => panic!("{e}"),
        }
    }

    /// The download as a seat from before 0.7.0 fetches it: its status and bytes.
    fn old_download(&self, path: &str, extra: &[(&str, &str)]) -> (u16, Vec<u8>) {
        let mut request = ureq::get(&format!("http://127.0.0.1:{}{path}", self.port)); // WHAT: plain HTTP, no device. WHY: as 0.6.31 downloads
        for (k, v) in extra {
            request = request.set(k, v); // WHAT: each extra header. WHY: how a request from outside is marked
        }
        match request.call() {
            Ok(r) => {
                let mut bytes = Vec::new(); // WHAT: the download. WHY: compared whole
                std::io::Read::read_to_end(&mut r.into_reader(), &mut bytes).unwrap(); // WHAT: read to the end. WHY: as above
                (200, bytes) // WHAT: answered. WHY: ureq hands back only success here
            }
            Err(ureq::Error::Status(code, _)) => (code, Vec::new()),
            Err(e) => panic!("{e}"),
        }
    }

    /// A 0.7 or later computer's client, signed by `key`.
    fn client(&self, key: &Arc<Signer>) -> hub::Client {
        hub::Client::pinned(&format!("https://127.0.0.1:{}", self.port), &self.pin)
            .unwrap()
            .with_device(key.clone())
    }

    fn state_of(&self, key: &Arc<Signer>) -> (String, bool) {
        let me = self.client(key).device_me().unwrap(); // WHAT: how this computer stands. WHY: signed by its own key
        (me.state, me.admin) // WHAT: its state and whether it is an admin. WHY: what the tests check
    }

    fn audit_says(&self, action: &str) -> i64 {
        // WHAT: how many lines of the audit log say `action`, as SQLite counts. WHY: compared with small numbers; no cast to lose a sign
        self.store()
            .with(|db| {
                Ok(db.query_row(
                    "SELECT count(*) FROM audit WHERE action = ?1",
                    params![action],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .unwrap() // WHAT: the count. WHY: a database that fails is a broken test
    }

    /// The next release, put where the server keeps the ones it offers.
    fn offer(&self, version: &str, bytes: &[u8]) {
        let store = self.store(); // WHAT: the store. WHY: the build goes in it
        let digest = store.put_blob(bytes).unwrap(); // WHAT: the build, kept by its digest. WHY: the download route reads it so
        store
            .with(|db| {
                db.execute(
                    "INSERT INTO releases (version, channel, platform, published, notes, bytes, digest, signature, signing_key, minimum_api, ready)
                     VALUES (?1, 'stable', 'windows-x64', ?2, 'the update', ?3, ?4, 'ab', 'k', 1, 1)",
                    params![version, now_text(0), i64::try_from(bytes.len()).unwrap(), digest],
                )?;
                Ok(())
            })
            .unwrap();
    }
}

fn signer(seed: u8) -> Arc<Signer> {
    Arc::new(Signer::new(DeviceKey::from_secret(&[seed; 32]))) // WHAT: a computer's key from a seed. WHY: each test computer its own, the same each run
}

/// An account made after the upgrade, straight into the database.
fn newcomer(office: &Office) {
    let hashed = auth::hash_password(PASSWORD).unwrap(); // WHAT: the password as stored. WHY: the newcomer signs in with it
    office.store().with(|db| Ok(db.execute("INSERT INTO people (id, name, email, password, role, created) VALUES ('usr_new', 'New', ?1, ?2, 'admin', ?3)", params![NEWCOMER, hashed, now_text(0)])?)).unwrap();
    // WHAT: an administrator made today. WHY: the strongest account the grace must still turn away
}

#[test]
#[allow(clippy::too_many_lines)] // WHAT: the whole upgrade in one test. WHY: each step leans on the one before, as at Mesa Fab; split, each part would start over from 0.6.31
fn an_office_upgraded_from_0_6_31_keeps_working_claims_its_admin_and_updates_its_seats() {
    let data = folder("upgraded"); // WHAT: a fresh data folder. WHY: this test's own
    let old_session = as_0_6_31_left_it(&data); // WHAT: the database as 0.6.31 left it, and a seat's session. WHY: the upgrade under test starts there
    let office = start(&data); // WHAT: this version, started over it. WHY: its first start is the upgrade

    // The first start after the upgrade: the grace and the claim are open.
    let grace = parsed(
        &office
            .setting(devices::settings::GRACE_UNTIL)
            .expect("the grace opened"),
    );
    let days = (grace - time::OffsetDateTime::now_utc()).whole_hours();
    assert!(
        (13 * 24..=14 * 24).contains(&days),
        "fourteen days of grace: {days} hours"
    );
    let claim = parsed(
        &office
            .setting(devices::settings::CLAIM_UNTIL)
            .expect("a claiming window"),
    );
    assert!(
        claim > time::OffsetDateTime::now_utc() + time::Duration::hours(23),
        "the claim opens for a day, as on a fresh install: {claim}"
    );
    assert_eq!(
        office.audit_says(devices::action::GRACE_OPENED),
        1,
        "the grace is in the audit log"
    );

    // A 0.6.30 seat with its session carries on, unsigned, over plain HTTP.
    let (status, me) = office.old_seat("GET", "/api/v1/me", Some(&old_session), None, &[]);
    assert_eq!(status, 200, "the old seat's session still works: {me}");
    assert_eq!(me["email"], DANA);
    assert_eq!(
        office
            .old_seat("GET", "/api/v1/projects", Some(&old_session), None, &[])
            .0,
        200,
        "and so does its work"
    );
    assert!(
        office.audit_says(devices::action::GRACE_SEAT) >= 1,
        "written in the audit log"
    );
    // A 0.6.31 seat that forgot its server signs in again, unsigned.
    let (status, signed) = office.old_seat(
        "POST",
        "/api/v1/auth/token",
        None,
        Some(serde_json::json!({"email": OWNER, "password": PASSWORD})),
        &[],
    );
    assert_eq!(
        status, 200,
        "an existing account signs in from an old seat: {signed}"
    );
    let owner_session = signed["token"].as_str().unwrap().to_string();
    assert_eq!(
        office
            .old_seat("GET", "/api/v1/me", Some(&owner_session), None, &[])
            .1["email"],
        OWNER
    );
    // A session that ran out is told to sign in, in the word an old seat knows.
    let (status, said) = office.old_seat("GET", "/api/v1/me", Some("not-a-session"), None, &[]);
    assert_eq!(
        (status, said["error"].as_str()),
        (401, Some("unauthorised")),
        "{said}"
    );
    // Never from outside, and never for an account made since.
    let outside = [("cf-connecting-ip", "203.0.113.9")];
    let (status, said) = office.old_seat("GET", "/api/v1/me", Some(&old_session), None, &outside);
    assert_eq!(
        (status, said["error"].as_str()),
        (401, Some("device_unknown")),
        "the grace is for the office network only: {said}"
    );
    newcomer(&office);
    let (status, said) = office.old_seat(
        "POST",
        "/api/v1/auth/token",
        None,
        Some(serde_json::json!({"email": NEWCOMER, "password": PASSWORD})),
        &[],
    );
    assert_eq!(
        (status, said["error"].as_str()),
        (401, Some("device_unknown")),
        "an account made after the upgrade gets no grace: {said}"
    );
    // And never a route that doesn't ask who is calling.
    let (status, said) = office.old_seat("POST", "/api/v1/join", None, Some(serde_json::json!({"code": "x", "name": "x", "email": "x@x.test", "password": PASSWORD})), &[]);
    assert_eq!(
        (status, said["error"].as_str()),
        (401, Some("device_unknown")),
        "an unsigned join from the office network is held as ever: {said}"
    );

    // The old seat fetches its update: not signed in, not approved.
    let build = b"the 0.7.2 installer, signed".to_vec();
    office.offer("0.7.2", &build);
    let (status, offer) = office.old_seat(
        "GET",
        "/api/v1/update/latest?version=0.6.31&channel=stable&platform=windows-x64",
        None,
        None,
        &[],
    );
    assert_eq!(status, 200, "{offer}");
    assert_eq!(
        offer["release"]["version"], "0.7.2",
        "offered to a seat nobody signed in: {offer}"
    );
    let download = offer["release"]["download"].as_str().unwrap().to_string();
    let (status, bytes) = office.old_download(&download, &[]);
    assert_eq!(
        (status, bytes),
        (200, build.clone()),
        "and downloaded whole"
    );
    assert_eq!(
        office
            .old_seat(
                "GET",
                "/api/v1/update/latest?version=0.6.31&platform=windows-x64",
                None,
                None,
                &outside
            )
            .0,
        401,
        "from outside, a sign-in is still needed"
    );
    assert_eq!(
        office.old_download(&download, &outside).0,
        401,
        "for the download too"
    );

    // A new 0.7.2 computer claims the admin seat: the office had none.
    let first = signer(1);
    assert_eq!(
        office
            .client(&first)
            .ask_to_be_let_in("view", "Owner")
            .unwrap()
            .unwrap()
            .state,
        "approved"
    );
    assert_eq!(
        office.state_of(&first),
        ("approved".to_string(), true),
        "the first new computer is the admin computer"
    );
    // The next waits, until an account from before the upgrade signs in on it.
    let second = signer(2);
    assert_eq!(
        office
            .client(&second)
            .ask_to_be_let_in("view", "Dana")
            .unwrap()
            .unwrap()
            .state,
        "waiting"
    );
    let mut danas_seat = office.client(&second); // WHAT: Dana's new computer's client. WHY: named apart from `data`, which clippy finds too alike
    danas_seat
        .sign_in(DANA, PASSWORD)
        .expect("an existing account's sign-in during the grace");
    assert_eq!(
        office.state_of(&second),
        ("approved".to_string(), false),
        "approved by it, as an ordinary computer"
    );
    assert_eq!(danas_seat.me().unwrap().email, DANA, "and signed in");
    assert_eq!(
        office.audit_says(devices::action::GRACE_APPROVED),
        1,
        "written in the audit log"
    );
    // An account made since approves nothing.
    // A new computer that only asks waits, now the office has an admin, and
    // an account made since approves nothing. (The refused and removed
    // computers are in their own office below: every ask from one address
    // counts against it, and this one stays within the free tries.)
    let third = signer(3); // WHAT: a new computer after the claim. WHY: it must wait, not claim
    assert_eq!(
        office
            .client(&third)
            .ask_to_be_let_in("view", "New")
            .unwrap()
            .unwrap()
            .state,
        "waiting",
        "a computer nobody signed in on waits for the admin"
    ); // WHAT: waiting. WHY: the office has its admin now
    assert!(
        office.client(&third).sign_in(NEWCOMER, PASSWORD).is_err(),
        "an account made after the upgrade can't approve a computer"
    ); // WHAT: refused. WHY: only accounts older than the device list get the grace
    assert_eq!(office.state_of(&third).0, "waiting"); // WHAT: still waiting. WHY: as above

    // A restart doesn't open the grace or the claim again.
    office
        .store()
        .set_setting(devices::settings::GRACE_UNTIL, &now_text(-1))
        .unwrap(); // WHAT: two weeks on. WHY: the grace is over
    office
        .store()
        .set_setting(devices::settings::CLAIM_UNTIL, &now_text(-1))
        .unwrap(); // WHAT: a day on. WHY: likewise
    devices::open_for_the_upgrade(&office.store(), time::OffsetDateTime::now_utc()); // WHAT: what the next start does. WHY: the mark keeps it from opening again
    assert!(
        parsed(&office.setting(devices::settings::GRACE_UNTIL).unwrap())
            < time::OffsetDateTime::now_utc(),
        "a restart never reopens the grace"
    );
    assert!(
        parsed(&office.setting(devices::settings::CLAIM_UNTIL).unwrap())
            < time::OffsetDateTime::now_utc(),
        "nor the claim"
    );

    // After the grace, the device gate holds again, and says to update.
    let (status, said) = office.old_seat("GET", "/api/v1/me", Some(&old_session), None, &[]);
    assert_eq!(
        (status, said["error"].as_str()),
        (401, Some("device_unknown")),
        "{said}"
    );
    assert!(
        said["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Update Excalibur View"),
        "an old seat is told to update: {said}"
    );
    let (status, _) = office.old_seat(
        "POST",
        "/api/v1/auth/token",
        None,
        Some(serde_json::json!({"email": OWNER, "password": PASSWORD})),
        &[],
    );
    assert_eq!(status, 401, "no old seat signs in after the grace");
    let sixth = signer(6);
    office
        .client(&sixth)
        .ask_to_be_let_in("view", "Dana")
        .unwrap();
    assert!(
        office.client(&sixth).sign_in(DANA, PASSWORD).is_err(),
        "a waiting computer waits for an admin after the grace"
    );
    // The update is still there for any seat on the office network.
    let (status, offer) = office.old_seat(
        "GET",
        "/api/v1/update/latest?version=0.6.31&platform=windows-x64",
        None,
        None,
        &[],
    );
    assert_eq!(
        (status, offer["release"]["version"].as_str()),
        (200, Some("0.7.2")),
        "{offer}"
    );
    assert_eq!(
        office.old_download(&download, &[]),
        (200, build),
        "and downloads"
    );
}

#[test]
fn an_admin_account_on_a_new_computer_becomes_the_admin_computer_when_there_is_none() {
    let data = folder("upgraded-admin");
    let _ = as_0_6_31_left_it(&data);
    let office = start(&data); // WHAT: this version, started over it. WHY: its first start is the upgrade
    office
        .store()
        .set_setting(devices::settings::CLAIM_UNTIL, &now_text(-1))
        .unwrap(); // WHAT: nobody claimed in the day. WHY: the owner's sign-in is the other way to an admin computer
    let estimator = signer(10);
    office
        .client(&estimator)
        .ask_to_be_let_in("view", "Dana")
        .unwrap();
    office.client(&estimator).sign_in(DANA, PASSWORD).unwrap();
    assert_eq!(
        office.state_of(&estimator),
        ("approved".to_string(), false),
        "an estimator's computer is never the admin computer"
    );
    let owner = signer(11);
    office
        .client(&owner)
        .ask_to_be_let_in("view", "Owner")
        .unwrap();
    office.client(&owner).sign_in(OWNER, PASSWORD).unwrap();
    assert_eq!(
        office.state_of(&owner),
        ("approved".to_string(), true),
        "the first admin account's computer is the admin computer"
    );
    let second = signer(12);
    office
        .client(&second)
        .ask_to_be_let_in("view", "Owner")
        .unwrap();
    office.client(&second).sign_in(OWNER, PASSWORD).unwrap();
    assert_eq!(
        office.state_of(&second),
        ("approved".to_string(), false),
        "once there is one, the admin computers decide who else is"
    );
}

#[test]
fn a_fresh_install_gets_no_grace_and_keeps_its_own_day() {
    let data = folder("fresh");
    let office = start(&data); // WHAT: no database before it. WHY: LatCon's server, installed fresh
    newcomer(&office); // WHAT: the first account, made after the first start. WHY: as /setup makes it
    devices::open_for_the_upgrade(&office.store(), time::OffsetDateTime::now_utc()); // WHAT: the next start. WHY: it must find nothing to carry over
    assert_eq!(
        office.setting(devices::settings::GRACE_UNTIL),
        None,
        "no grace for a fresh office"
    );
    assert_eq!(
        office.setting(devices::settings::UPGRADE_SEEN),
        None,
        "and no upgrade noted"
    );
    let (status, _) = office.old_seat(
        "POST",
        "/api/v1/auth/token",
        None,
        Some(serde_json::json!({"email": NEWCOMER, "password": PASSWORD})),
        &[],
    );
    assert_eq!(status, 401, "an unsigned sign-in is refused, as in 0.7.1");
}

#[test]
fn an_office_already_on_0_7_1_with_its_admin_set_gets_the_grace_and_no_new_claim() {
    let data = folder("mesa-on-0-7-1");
    let old_session = as_0_6_31_left_it(&data); // WHAT: the database as 0.6.31 left it, and a seat's session. WHY: the upgrade under test starts there
    let office = start(&data); // WHAT: this version, started over it. WHY: its first start is the upgrade
    let store = office.store();
    for key in [
        devices::settings::GRACE_UNTIL,
        devices::settings::UPGRADE_SEEN,
    ] {
        store
            .with(|db| Ok(db.execute("DELETE FROM settings WHERE key = ?1", params![key])?))
            .unwrap(); // WHAT: as 0.7.1 left it. WHY: 0.7.1 knew neither
    }
    store
        .set_setting(devices::settings::BEGAN, &now_text(-1))
        .unwrap(); // WHAT: 0.7.1 began the device list yesterday. WHY: Mesa, 2026-11-20
    store
        .set_setting(devices::settings::CLAIM_UNTIL, &now_text(-1))
        .unwrap(); // WHAT: its claiming window long shut. WHY: as 0.7.1 set it
    let admin = signer(20);
    office
        .client(&admin)
        .ask_to_be_let_in("view", "Owner")
        .unwrap();
    store
        .with(|db| {
            Ok(db.execute(
                "UPDATE devices SET state = 'approved', admin = 1 WHERE id = ?1",
                params![admin.key().id()],
            )?)
        })
        .unwrap(); // WHAT: the admin computer the owner made with --make-admin. WHY: as at Mesa
    devices::open_for_the_upgrade(&store, time::OffsetDateTime::now_utc()); // WHAT: 0.7.2's first start. WHY: under test
    assert!(
        office.setting(devices::settings::GRACE_UNTIL).is_some(),
        "the seats still on 0.6.x get their grace"
    );
    assert!(
        parsed(&office.setting(devices::settings::CLAIM_UNTIL).unwrap())
            < time::OffsetDateTime::now_utc(),
        "an office with an admin computer is never opened to a claim"
    );
    assert_eq!(
        office
            .old_seat("GET", "/api/v1/me", Some(&old_session), None, &[])
            .0,
        200,
        "the old seat works again"
    );
    assert_eq!(
        office.state_of(&admin),
        ("approved".to_string(), true),
        "the admin computer is as it was"
    );
    let updated = signer(21); // WHAT: an old desktop, updated to 0.7.2. WHY: Mesa's seats after they update
    office
        .client(&updated)
        .ask_to_be_let_in("view", "Dana")
        .unwrap();
    office
        .client(&updated)
        .sign_in(DANA, PASSWORD)
        .expect("its person's sign-in during the grace");
    assert_eq!(
        office.state_of(&updated),
        ("approved".to_string(), false),
        "approved by its person, as an ordinary computer, with the admin computer already set"
    );

    let late = folder("upgraded-long-ago");
    let _ = as_0_6_31_left_it(&late);
    let long_ago = start(&late);
    let store = long_ago.store();
    for key in [
        devices::settings::GRACE_UNTIL,
        devices::settings::UPGRADE_SEEN,
    ] {
        store
            .with(|db| Ok(db.execute("DELETE FROM settings WHERE key = ?1", params![key])?))
            .unwrap(); // WHAT: as an older 0.7 left it. WHY: as above
    }
    store
        .set_setting(devices::settings::BEGAN, &now_text(-40))
        .unwrap(); // WHAT: the device list began forty days ago. WHY: past the month the grace is for
    devices::open_for_the_upgrade(&store, time::OffsetDateTime::now_utc());
    assert_eq!(
        long_ago.setting(devices::settings::GRACE_UNTIL),
        None,
        "no grace a month and more after the device list began"
    );
}

#[test]
fn the_console_sets_the_grace_within_a_month_and_writes_it_down() {
    let data = folder("grace-console");
    let _ = as_0_6_31_left_it(&data);
    let office = start(&data); // WHAT: this version, started over it. WHY: its first start is the upgrade
    let store = office.store();
    assert!(
        devices::set_grace_here(&store, devices::MOST_GRACE_DAYS + 1).is_err(),
        "never more than a month"
    );
    assert!(
        devices::set_grace_here(&store, -1).is_err(),
        "never less than nothing"
    );
    devices::set_grace_here(&store, 0).unwrap();
    assert!(
        parsed(&office.setting(devices::settings::GRACE_UNTIL).unwrap())
            <= time::OffsetDateTime::now_utc(),
        "0 ends it"
    );
    assert_eq!(
        office.audit_says(devices::action::GRACE_CHANGED),
        1,
        "in the audit log"
    );
}

#[test]
fn a_computer_an_admin_refused_or_removed_is_never_brought_back_by_a_sign_in() {
    let data = folder("upgraded-removed"); // WHAT: an office of its own. WHY: the main test's wrong tries would slow this one's requests
    let _ = as_0_6_31_left_it(&data); // WHAT: the database as 0.6.31 left it. WHY: the grace opens on the first start
    let office = start(&data); // WHAT: this version over it. WHY: under test
    let admin = signer(30); // WHAT: the first new computer. WHY: it claims the office, so it can approve and remove
    office
        .client(&admin)
        .ask_to_be_let_in("view", "Owner")
        .unwrap(); // WHAT: it claims the admin seat. WHY: the office had none
    let refused = signer(32); // WHAT: a computer an admin turned down. WHY: the grace must never move it either
    office
        .client(&refused)
        .ask_to_be_let_in("view", "Stranger")
        .unwrap(); // WHAT: it asks. WHY: so it is on the list
    office
        .client(&admin)
        .refuse_device(&refused.key().id())
        .unwrap(); // WHAT: refused by the admin computer. WHY: the state under test
    assert!(
        office.client(&refused).sign_in(OWNER, PASSWORD).is_err(),
        "a refused computer is never brought back by a sign-in"
    ); // WHAT: refused, even for the owner's account. WHY: only an admin's Approve brings it back
    assert_eq!(office.state_of(&refused).0, "refused"); // WHAT: unchanged. WHY: as above
    let removed = signer(31); // WHAT: a computer an admin approved, then took off the list. WHY: the other state the grace must never move
    office
        .client(&removed)
        .ask_to_be_let_in("view", "Leaver")
        .unwrap(); // WHAT: it asks. WHY: so it is on the list
    office
        .client(&admin)
        .approve_device(&removed.key().id(), false)
        .unwrap(); // WHAT: approved, not as an admin. WHY: as an ordinary seat is
    office
        .client(&admin)
        .remove_device(&removed.key().id())
        .unwrap(); // WHAT: then removed. WHY: the state under test
    assert!(
        office.client(&removed).sign_in(OWNER, PASSWORD).is_err(),
        "a removed computer is never brought back by a sign-in"
    ); // WHAT: refused, even for the owner's account. WHY: only an admin's Approve brings it back
    assert_eq!(office.state_of(&removed).0, "removed"); // WHAT: unchanged. WHY: as above
}
