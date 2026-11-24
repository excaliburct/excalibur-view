//! An office's server upgraded from 0.6.31 to this version, end to end
//! (docs/OFFICE-ENROLLMENT.md §3a). The database is the one 0.6.31 made
//! (tests/fixtures/office-0.6.31.sql), with its people, a seat's session and
//! the next release already on it. Then: the seats from before 0.7.0 keep
//! working through the upgrade grace, no new computer can claim the admin seat,
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
    db.execute("INSERT INTO settings (key, value) VALUES ('licensing_began', '2026-03-01T08:00:00Z')", []).unwrap(); // WHAT: when 0.6.x first ran here, as it writes at its first start. WHY: how an office whose database already has a device table is told from a fresh one
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

    // The first start after the upgrade: the grace is open, the claim is not.
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
        claim < time::OffsetDateTime::now_utc(),
        "the claim is NOT reopened by the upgrade (review M1): {claim}"
    ); // WHAT: the window is shut. WHY: only an admin account's sign-in makes the admin computer after an upgrade
    assert_eq!(
        office.audit_says(devices::action::REOPENED_CLAIM),
        0,
        "and nothing says it was"
    ); // WHAT: no reopen line. WHY: nothing reopened it
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

    // A new 0.7.2 computer can't claim the admin seat, though the office has none (review M1).
    let first = signer(1);
    assert_eq!(
        office
            .client(&first)
            .ask_to_be_let_in("view", "Owner")
            .unwrap()
            .unwrap()
            .state,
        "waiting",
        "no computer claims an upgraded office"
    );
    assert_eq!(
        office.state_of(&first),
        ("waiting".to_string(), false),
        "and none is made the admin computer by asking"
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
    assert!(
        office.client(&first).sign_in(NEWCOMER, PASSWORD).is_err(),
        "an account made after the upgrade can't approve a computer"
    ); // WHAT: refused, on the computer that only asked. WHY: only accounts older than the device list get the grace
    assert_eq!(office.state_of(&first).0, "waiting"); // WHAT: still waiting. WHY: as above

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
    assert!(
        office.client(&first).sign_in(DANA, PASSWORD).is_err(),
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
        devices::settings::FROM_BEFORE_DEVICES,
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
        devices::settings::FROM_BEFORE_DEVICES,
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
    let admin = signer(30); // WHAT: the owner's new computer. WHY: the owner's sign-in makes it the admin computer, so it can approve and remove
    office
        .client(&admin)
        .ask_to_be_let_in("view", "Owner")
        .unwrap(); // WHAT: it asks, and waits. WHY: nothing claims an upgraded office (review M1)
    office.client(&admin).sign_in(OWNER, PASSWORD).unwrap(); // WHAT: the owner signs in on it. WHY: the one way to the admin seat after an upgrade
    assert_eq!(office.state_of(&admin), ("approved".to_string(), true)); // WHAT: the admin computer. WHY: it must be, for what follows
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

// ---- the security review of 0.7.2 -------------------------------------------

impl Office {
    /// A release put where the server keeps them: `channel`, ready or not,
    /// with `bytes` as its installer.
    fn release(&self, version: &str, channel: &str, ready: i64, bytes: &[u8]) {
        let store = self.store(); // WHAT: the store. WHY: the build goes in it
        let digest = store.put_blob(bytes).unwrap(); // WHAT: the build, kept by its digest. WHY: the download route reads it so
        store
            .with(|db| {
                Ok(db.execute(
                    "INSERT INTO releases (version, channel, platform, published, notes, bytes, digest, signature, signing_key, minimum_api, ready)
                     VALUES (?1, ?2, 'windows-x64', ?3, 'notes', ?4, ?5, 'ab', 'k', 1, ?6)",
                    params![version, channel, now_text(0), i64::try_from(bytes.len()).unwrap(), digest, ready],
                )?)
            })
            .unwrap(); // WHAT: the row. WHY: as the release feed writes it
    }
}

/// A plain GET with no device, no session: its status, its Content-Length
/// header and its bytes.
fn plain_get(office: &Office, path: &str) -> (u16, Option<String>, Vec<u8>) {
    match ureq::get(&format!("http://127.0.0.1:{}{path}", office.port)).call() {
        Ok(r) => {
            let length = r.header("content-length").map(str::to_string); // WHAT: the length it promised. WHY: a streamed body still says it
            let mut bytes = Vec::new(); // WHAT: the download. WHY: compared whole
            std::io::Read::read_to_end(&mut r.into_reader(), &mut bytes).unwrap(); // WHAT: read to the end. WHY: as above
            (200, length, bytes)
        }
        Err(ureq::Error::Status(code, _)) => (code, None, Vec::new()),
        Err(e) => panic!("{e}"),
    }
}

const DOWNLOAD: &str = "/api/v1/update/{v}/download?platform=windows-x64"; // WHAT: the download's address, its version left to fill. WHY: one place for it

fn download_of(version: &str) -> String {
    DOWNLOAD.replace("{v}", version) // WHAT: the address for one version. WHY: the tests ask for several
}

#[test]
fn a_release_not_ready_is_never_downloaded_by_a_stranger() {
    let data = folder("m2-not-ready");
    let _ = as_0_6_31_left_it(&data);
    let office = start(&data);
    let bytes = b"unready unsigned build".to_vec();
    office.release("9.9.9-private", "preview", 0, &bytes); // WHAT: a preview build still being made. WHY: the reviewer's probe
    office.release("0.7.5", "stable", 0, &bytes); // WHAT: a stable one not ready either. WHY: ready is the gate, not only the channel
    assert_eq!(office.old_download(&download_of("9.9.9-private"), &[]).0, 404, "not ready, not preview");
    assert_eq!(office.old_download(&download_of("0.7.5"), &[]).0, 404, "not ready, though stable");
    office.store().set_setting("update_channel", "preview").unwrap(); // WHAT: the office on preview. WHY: ready is still needed
    assert_eq!(office.old_download(&download_of("9.9.9-private"), &[]).0, 404, "not ready, though the office is on preview");
}

#[test]
fn a_preview_release_is_served_only_to_an_office_on_preview() {
    let data = folder("m2-channel");
    let _ = as_0_6_31_left_it(&data);
    let office = start(&data);
    let bytes = b"a preview build, ready".to_vec();
    office.release("9.9.8", "preview", 1, &bytes);
    assert_eq!(office.old_download(&download_of("9.9.8"), &[]).0, 404, "a stable office doesn't serve it");
    assert_eq!(
        office.old_download(&format!("{}&channel=preview", download_of("9.9.8")), &[]).0,
        404,
        "and a stranger can't ask for the preview channel"
    );
    office.store().set_setting("update_channel", "preview").unwrap(); // WHAT: the office on preview. WHY: then it is the office's channel
    assert_eq!(office.old_download(&download_of("9.9.8"), &[]), (200, bytes), "an office on preview serves it");
}

#[test]
fn the_download_is_streamed_with_its_length() {
    let data = folder("m2-stream");
    let _ = as_0_6_31_left_it(&data);
    let office = start(&data);
    let bytes: Vec<u8> = (0..3 * 1024 * 1024 + 17).map(|i: u32| u8::try_from(i % 251).unwrap()).collect(); // WHAT: an installer of a few MiB, not a multiple of a piece. WHY: the last piece is a short one
    office.release("0.7.5", "stable", 1, &bytes);
    let (status, length, got) = plain_get(&office, &download_of("0.7.5"));
    assert_eq!(status, 200);
    assert_eq!(length, Some(bytes.len().to_string()), "it says how long it is");
    assert!(got == bytes, "and every byte arrives, in order");
}

#[test]
fn only_a_few_anonymous_downloads_run_at_once_and_the_rest_are_told_to_retry() {
    let data = folder("m2-cap");
    let _ = as_0_6_31_left_it(&data);
    let office = start(&data);
    let big = vec![7u8; 48 * 1024 * 1024]; // WHAT: an installer far bigger than the sockets' buffers. WHY: a client that doesn't read leaves the server holding its place
    office.release("0.7.5", "stable", 1, &big);
    let mut held = Vec::new(); // WHAT: the connections left open. WHY: each keeps its place
    for _ in 0..devices_places() {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", office.port)).unwrap(); // WHAT: a plain connection. WHY: sends the request by hand, then never reads the body
        std::io::Write::write_all(&mut stream, format!("GET {} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n", download_of("0.7.5")).as_bytes()).unwrap(); // WHAT: the request. WHY: it starts a download
        let mut first = [0u8; 16]; // WHAT: the start of the answer. WHY: the download has begun when it arrives
        std::io::Read::read_exact(&mut stream, &mut first).unwrap(); // WHAT: read. WHY: proof the place is taken
        held.push(stream);
    }
    let (status, said) = office.old_seat("GET", &download_of("0.7.5"), None, None, &[]);
    assert_eq!(status, 503, "the next is told to retry: {said}");
    assert_eq!(said["error"], "busy");
    drop(held); // WHAT: the connections closed. WHY: the places are given back
}

/// The number of anonymous downloads the server allows at once.
fn devices_places() -> usize {
    hyperview_server::download::AT_ONCE // WHAT: the server's own number. WHY: the test follows it
}

#[test]
fn one_address_may_start_only_so_many_anonymous_downloads() {
    let data = folder("m2-rate");
    let _ = as_0_6_31_left_it(&data);
    let office = start(&data);
    office.release("0.7.5", "stable", 1, b"a small build");
    for n in 0..hyperview_server::download::PER_WINDOW {
        assert_eq!(office.old_download(&download_of("0.7.5"), &[]).0, 200, "download {n} of the window");
    }
    let (status, said) = office.old_seat("GET", &download_of("0.7.5"), None, None, &[]);
    assert_eq!(status, 429, "past the window's share: {said}");
    assert_eq!(said["error"], "too_many");
}

#[test]
fn a_signed_in_person_is_not_held_to_the_anonymous_limit() {
    let data = folder("m2-signed");
    let old_session = as_0_6_31_left_it(&data);
    let office = start(&data);
    office.release("0.7.5", "stable", 1, b"a small build");
    for _ in 0..hyperview_server::download::PER_WINDOW + 2 {
        let agent = ureq::get(&format!("http://127.0.0.1:{}{}", office.port, download_of("0.7.5"))).set("Authorization", &format!("Bearer {old_session}")); // WHAT: a seat with its session. WHY: it is signed in
        assert_eq!(agent.call().unwrap().status(), 200);
    }
}

#[test]
fn what_an_unsigned_seat_is_offered_is_only_what_the_office_would_install() {
    let data = folder("m2-offer");
    let old_session = as_0_6_31_left_it(&data);
    let office = start(&data);
    office.release("0.7.5", "stable", 1, b"stable build");
    office.release("9.9.9", "preview", 1, b"preview build");
    office.store().set_setting("pin_version", "0.7.5").unwrap(); // WHAT: the office held on 0.7.5. WHY: its hold is not for strangers
    let ask = "/api/v1/update/latest?version=0.6.31&platform=windows-x64&channel=preview&fresh=true";
    let (status, offer) = office.old_seat("GET", ask, None, None, &[]);
    assert_eq!(status, 200, "{offer}");
    assert_eq!(offer["release"]["version"], "0.7.5", "stable only, whatever the asker says: {offer}");
    assert!(offer.get("pinned_to").is_none(), "no hold details for a stranger: {offer}");
    assert!(!offer["looking"].as_bool().unwrap_or(false), "nor whether the server is looking: {offer}");
    // Signed in, the person sees the hold; with no pin, their own preview channel.
    let (_, signed) = office.old_seat("GET", ask, Some(&old_session), None, &[]);
    assert_eq!(signed["pinned_to"], "0.7.5", "a signed-in person sees the hold: {signed}");
    office.store().set_setting("pin_version", "").unwrap(); // WHAT: the hold taken off. WHY: so the preview channel can be seen
    let (_, signed) = office.old_seat("GET", ask, Some(&old_session), None, &[]);
    assert_eq!(signed["release"]["version"], "9.9.9", "a signed-in seat may try the preview channel: {signed}");
    let (_, stranger) = office.old_seat("GET", ask, None, None, &[]);
    assert_eq!(stranger["release"]["version"], "0.7.5", "a stranger still gets stable: {stranger}");
}

#[test]
fn a_stale_bearer_on_a_sign_in_neither_blocks_it_nor_skips_the_older_account_rule() {
    let data = folder("l1-stale");
    let old_session = as_0_6_31_left_it(&data);
    let office = start(&data);
    let good = serde_json::json!({"email": OWNER, "password": PASSWORD});
    let (status, said) = office.old_seat("POST", "/api/v1/auth/token", Some("stale-token-from-last-week"), Some(good), &[]);
    assert_eq!(status, 200, "a stale bearer doesn't block a correct sign-in: {said}");
    newcomer(&office);
    let body = serde_json::json!({"email": NEWCOMER, "password": PASSWORD});
    let (status, said) = office.old_seat("POST", "/api/v1/auth/token", Some(&old_session), Some(body.clone()), &[]);
    assert_eq!((status, said["error"].as_str()), (401, Some("device_unknown")), "a good session on the sign-in doesn't skip the older-account rule: {said}");
    let (status, said) = office.old_seat("POST", "/api/v1/auth/token", Some("stale-token-from-last-week"), Some(body), &[]);
    assert_eq!((status, said["error"].as_str()), (401, Some("device_unknown")), "nor does a stale one: {said}");
}

#[test]
fn a_computer_with_no_password_never_becomes_the_admin_computer_after_an_upgrade() {
    let data = folder("m1-claim");
    let _ = as_0_6_31_left_it(&data);
    let office = start(&data);
    let attacker = signer(66);
    let said = office.client(&attacker).ask_to_be_let_in("view", "guest-wifi").unwrap().unwrap().state;
    assert_eq!(said, "waiting", "asking with no password gets nothing");
    assert_eq!(office.state_of(&attacker), ("waiting".to_string(), false));
    assert!(office.client(&attacker).device_me().unwrap().claimable.not(), "and is not offered the admin seat");
    let owner = signer(67);
    office.client(&owner).ask_to_be_let_in("view", "Owner").unwrap();
    office.client(&owner).sign_in(OWNER, PASSWORD).unwrap();
    assert_eq!(office.state_of(&owner), ("approved".to_string(), true), "the owner's admin sign-in makes the admin computer");
    assert_eq!(office.state_of(&attacker), ("waiting".to_string(), false), "and the attacker still waits");
}

use std::ops::Not as _; // WHAT: `.not()` on a bool. WHY: reads as a plain assertion above

#[test]
fn the_upgrade_is_read_from_the_old_schema_marker_not_from_account_dates() {
    // An upgraded office: people and no device list when 0.7.x first opened it.
    let data = folder("l2-marker");
    let _ = as_0_6_31_left_it(&data);
    let office = start(&data);
    assert!(office.setting(devices::settings::FROM_BEFORE_DEVICES).is_some(), "the 0.6.x database is marked");
    assert!(office.setting(devices::settings::GRACE_UNTIL).is_some(), "and gets its grace");
    // Opened again, it keeps its mark, and a restart changes nothing.
    let first = office.setting(devices::settings::FROM_BEFORE_DEVICES);
    assert_eq!(office.setting(devices::settings::FROM_BEFORE_DEVICES), first, "the mark is kept once");
    // An office already on 0.7.1 has the device table and no mark; licensing's start, long before the device list's, tells it.
    let store = office.store();
    for key in [devices::settings::GRACE_UNTIL, devices::settings::UPGRADE_SEEN, devices::settings::FROM_BEFORE_DEVICES] {
        store.with(|db| Ok(db.execute("DELETE FROM settings WHERE key = ?1", params![key])?)).unwrap(); // WHAT: as 0.7.1 left it. WHY: only the licensing marker remains
    }
    store.set_setting(devices::settings::BEGAN, &now_text(-1)).unwrap(); // WHAT: the device list began yesterday. WHY: Mesa
    devices::open_for_the_upgrade(&store, time::OffsetDateTime::now_utc());
    assert!(office.setting(devices::settings::GRACE_UNTIL).is_some(), "licensing's old start alone marks it");
    // A fresh install is never marked.
    let fresh = folder("l2-fresh-marker");
    let new_office = start(&fresh);
    assert_eq!(new_office.setting(devices::settings::FROM_BEFORE_DEVICES), None, "a fresh office is not marked");
}

#[test]
fn a_fresh_install_whose_clock_ran_ahead_is_not_upgraded() {
    // LatCon's case: 0.7.1 installed fresh, its clock three days ahead at the first start, then set right.
    let data = folder("l2-skew");
    let office = start(&data); // WHAT: a fresh server. WHY: its device list begins now
    let ahead = now_text(3); // WHAT: what the wrong clock wrote at the first start. WHY: both settings are written in one start, by one clock
    office.store().set_setting(devices::settings::BEGAN, &ahead).unwrap(); // WHAT: the device list's start, in the future. WHY: the accounts made since look older than it
    office.store().set_setting("licensing_began", &ahead).unwrap(); // WHAT: licensing's start, the same moment. WHY: as above
    let hashed = auth::hash_password(PASSWORD).unwrap();
    office
        .store()
        .with(|db| Ok(db.execute("INSERT INTO people (id, name, email, password, role, created) VALUES ('usr_owner', 'Owner', ?1, ?2, 'admin', ?3)", params![OWNER, hashed, now_text(-1)])?))
        .unwrap(); // WHAT: an administrator dated days before the device list's start. WHY: the date alone would make it an upgrade
    devices::open_for_the_upgrade(&office.store(), time::OffsetDateTime::now_utc()); // WHAT: the next start. WHY: it must find nothing to carry over
    assert_eq!(office.setting(devices::settings::GRACE_UNTIL), None, "no grace");
    assert_eq!(office.setting(devices::settings::UPGRADE_SEEN), None, "no upgrade noted");
    let (status, said) = office.old_seat("POST", "/api/v1/auth/token", None, Some(serde_json::json!({"email": OWNER, "password": PASSWORD})), &[]);
    assert_eq!((status, said["error"].as_str()), (401, Some("device_unknown")), "an unsigned sign-in is held: {said}");
}

#[test]
fn an_account_dated_near_the_device_list_start_is_not_taken_for_an_old_one() {
    let data = folder("l2-margin");
    let _ = as_0_6_31_left_it(&data);
    let office = start(&data);
    let hashed = auth::hash_password(PASSWORD).unwrap();
    let began = parsed(&office.setting(devices::settings::BEGAN).unwrap());
    let near = (began - time::Duration::minutes(5)).format(&time::format_description::well_known::Rfc3339).unwrap(); // WHAT: five minutes before the list began. WHY: inside the margin
    office
        .store()
        .with(|db| Ok(db.execute("INSERT INTO people (id, name, email, password, role, created) VALUES ('usr_near', 'Near', ?1, ?2, 'admin', ?3)", params![NEWCOMER, hashed, near])?))
        .unwrap();
    let (status, said) = office.old_seat("POST", "/api/v1/auth/token", None, Some(serde_json::json!({"email": NEWCOMER, "password": PASSWORD})), &[]);
    assert_eq!((status, said["error"].as_str()), (401, Some("device_unknown")), "{said}");
}

#[test]
fn a_headless_first_admin_writes_the_device_list_start_before_the_account() {
    let data = folder("l2-headless");
    std::fs::create_dir_all(&data).unwrap();
    let said = std::fs::File::create(data.join("server-stderr.txt")).unwrap(); // WHAT: where the server's complaints go. WHY: printed below if it never gets there
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_hyperview-server"))
        .arg("--console") // WHAT: the documented headless way to run it (README). WHY: with no word, Windows takes the double-click's front door and tries to install the service instead of serving
        .env("HYPERVIEW_FIRST_ADMIN", "1") // WHAT: the headless way. WHY: under test
        .env("HYPERVIEW_DATA", &data) // WHAT: its own folder. WHY: never a real server's
        .env("HYPERVIEW_LISTEN", "127.0.0.1:0") // WHAT: any free port. WHY: tests run side by side
        .env("HYPERVIEW_TLS", "off") // WHAT: plain. WHY: nothing to pin here
        .stdin(std::process::Stdio::null()) // WHAT: nothing to read. WHY: a server that waited on Enter would end at once instead of hanging the test
        .stdout(std::process::Stdio::null())
        .stderr(said) // WHAT: its errors kept in a file. WHY: a failure below says why the server stopped
        .spawn()
        .unwrap();
    let config = config(&data);
    let mut seen = None; // WHAT: the began time and the account's time. WHY: compared below
    for _ in 0..300 {
        std::thread::sleep(std::time::Duration::from_millis(100)); // WHAT: a pause. WHY: the process starts on its own
        if let Ok(db) = rusqlite::Connection::open_with_flags(config.database(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
            let made = db.query_row("SELECT created FROM people LIMIT 1", [], |r| r.get::<_, String>(0)).ok(); // WHAT: when the account was made, if there is one. WHY: read without opening the server's database for writing
            let began = db.query_row("SELECT value FROM settings WHERE key = ?1", params![devices::settings::BEGAN], |r| r.get::<_, String>(0)).ok(); // WHAT: when the device list began, if noted. WHY: likewise
            if let (Some(made), Some(began)) = (made, began) {
                seen = Some((began, made));
                break;
            }
        }
    }
    let _ = child.kill(); // WHAT: the test's own server stopped. WHY: it serves until told
    let _ = child.wait(); // WHAT: reaped. WHY: nothing left behind
    let Some((began, made)) = seen else {
        panic!("the headless server made its administrator and noted the start; it said: {}", std::fs::read_to_string(data.join("server-stderr.txt")).unwrap_or_default()); // WHAT: the failure, with the server's own words. WHY: the reason is in the child, not in the test
    };
    assert!(parsed(&began) <= parsed(&made), "the start ({began}) is written before the account ({made})");
}

#[test]
fn a_0_6_x_database_is_upgraded_even_when_its_licensing_start_ran_ahead() {
    let data = folder("l2-marker-only");
    let _ = as_0_6_31_left_it(&data);
    let db = rusqlite::Connection::open(config(&data).database()).unwrap(); // WHAT: the database before the server opens it. WHY: its clock-ahead start is set as 0.6.x left it
    db.execute("UPDATE settings SET value = ?1 WHERE key = 'licensing_began'", params![now_text(3)]).unwrap(); // WHAT: licensing's start in the future. WHY: a 0.6.x clock that ran ahead; the licensing mark alone would call it fresh
    drop(db); // WHAT: closed. WHY: the server opens it next
    let office = start(&data);
    assert!(office.setting(devices::settings::GRACE_UNTIL).is_some(), "the schema's own mark is enough: no device table, with people");
}
