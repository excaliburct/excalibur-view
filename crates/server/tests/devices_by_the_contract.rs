//! The office's device list, run for real over HTTPS and held to
//! docs/OFFICE-ENROLLMENT.md: asking, the first admin, signed requests,
//! approving, the migration of seats signed in before 0.7.0, joins from
//! outside, the Citadel cell's answers and the office's identity. Each of §7's
//! guards built here has its test below, named for it.

use std::sync::Arc;

use hub::device::{self, Ask, DeviceKey, Signer};
use hyperview_server::{api, auth, Config, Store};
use rusqlite::params;

/// A server started as an office runs it: HTTPS on, devices required.
struct Office {
    data: std::path::PathBuf, // WHAT: its data folder. WHY: the tests read its database beside it
    port: u16, // WHAT: its port. WHY: HTTPS and plain HTTP answer on it
    pin: String, // WHAT: its certificate's pin. WHY: proofs are bound to it
    _runtime: tokio::runtime::Runtime, // WHAT: the server's runtime. WHY: kept alive for the test
}

fn start() -> Office {
    start_with(|_| {}, rand::random())
}

/// [`start`], with the configuration changed first.
fn start_with(change: impl FnOnce(&mut Config), n: u64) -> Office {
    let data = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("devices-{n:016x}")); // WHAT: a fresh folder. WHY: tests run side by side
    let mut config = Config {
        name: "LatCon".into(),
        data: data.clone(),
        listen: "127.0.0.1:0".parse().unwrap(),
        tls: true, // WHAT: HTTPS. WHY: proofs are bound to the pin
        devices: hyperview_server::config::Devices::Required, // WHAT: devices required. WHY: the thing under test
        ..Config::default()
    };
    change(&mut config); // WHAT: the test's own settings. WHY: some tests need a bigger upload limit
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let (told, heard) = std::sync::mpsc::channel();
    runtime.spawn(async move {
        let _ = hyperview_server::run::serve(config, std::future::pending(), move |serving| {
            let _ = told.send(serving.address.port());
        })
        .await;
    });
    let port = heard.recv_timeout(std::time::Duration::from_secs(30)).expect("the server started");
    let cert = std::fs::read_to_string(data.join(hyperview_server::tls::CERT)).expect("its certificate");
    let pin = hyperview_server::tls::pin(&cert).unwrap();
    Office { data, port, pin, _runtime: runtime }
}

impl Office {
    fn base(&self) -> String {
        format!("https://127.0.0.1:{}", self.port) // WHAT: its HTTPS address. WHY: every call goes there, by the pin
    }

    /// The server's database, opened beside it.
    fn store(&self) -> Store {
        let config = Config { data: self.data.clone(), ..Config::default() }; // WHAT: the same data folder. WHY: the same database
        Store::open(&config.database(), &config.blobs()).unwrap()
    }

    /// A client for this server signed by `key`.
    fn client(&self, key: &Arc<Signer>) -> hub::Client {
        hub::Client::pinned(&self.base(), &self.pin).unwrap().with_device(key.clone())
    }

    /// A raw request, signed as given, with any extra headers.
    fn raw(&self, method: &str, path: &str, body: &[u8], signed: Option<&device::Signed>, extra: &[(&str, &str)]) -> (u16, serde_json::Value) {
        let (agent, _) = hub::pinned::agent(Some(&self.pin)).unwrap();
        let agent = agent.timeout(std::time::Duration::from_secs(10)).build();
        let mut request = agent.request(method, &format!("{}{path}", self.base()));
        if let Some(s) = signed {
            request = request.set(device::HEADER_DEVICE, &s.device).set(device::HEADER_TIME, &s.time).set(device::HEADER_SIGNATURE, &s.signature);
        }
        for (k, v) in extra {
            request = request.set(k, v);
        }
        let answer = if body.is_empty() && method == "GET" { request.call() } else { request.set("Content-Type", "application/json").send_bytes(body) };
        match answer {
            Ok(r) => (r.status(), r.into_json().unwrap_or(serde_json::Value::Null)),
            Err(ureq::Error::Status(code, r)) => (code, r.into_json().unwrap_or(serde_json::Value::Null)),
            Err(e) => panic!("{e}"),
        }
    }

    /// A raw request with a body, signed as given, the body hash declared or
    /// not; its status and error word (0 and "" when the connection was
    /// closed before an answer came).
    fn raw_body(&self, method: &str, path: &str, body: &[u8], s: &device::Signed, declare: bool) -> (u16, String) {
        let (agent, _) = hub::pinned::agent(Some(&self.pin)).unwrap();
        let agent = agent.timeout(std::time::Duration::from_secs(30)).build();
        let mut request = agent.request(method, &format!("{}{path}", self.base())).set(device::HEADER_DEVICE, &s.device).set(device::HEADER_TIME, &s.time).set(device::HEADER_SIGNATURE, &s.signature);
        if declare {
            request = request.set(device::HEADER_BODY, &s.body);
        }
        let answer = if body.is_empty() { request.call() } else { request.send_bytes(body) };
        let word = |r: ureq::Response| r.into_json::<serde_json::Value>().ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or_default(); // WHAT: the error's short name. WHY: tells the gate's refusal from View's own
        match answer {
            Ok(r) => (r.status(), word(r)),
            Err(ureq::Error::Status(code, r)) => (code, word(r)),
            Err(_) => (0, String::new()),
        }
    }

    /// A device's request to be let in, as JSON bytes.
    fn ask(&self, key: &DeviceKey, app: &str) -> Vec<u8> {
        serde_json::to_vec(&Ask::signed(key, &self.pin, app, "0.7.0", "DANA-LAPTOP", "dana", "Dana", "windows-x64", device::unix_now()).unwrap()).unwrap()
    }
}

fn key(seed: u8) -> DeviceKey {
    DeviceKey::from_secret(&[seed; 32])
}

fn signer(seed: u8) -> Arc<Signer> {
    Arc::new(Signer::new(key(seed)))
}

/// The first computer, approved as the admin.
fn first_admin(office: &Office) -> Arc<Signer> {
    let admin = signer(1);
    let answer = office.client(&admin).ask_to_be_let_in("view", "Mark").unwrap().unwrap();
    assert_eq!(answer.state, "approved", "the first computer is the admin");
    admin
}

fn state_of(office: &Office, id: &str) -> String {
    office.store().with(|db| Ok(db.query_row("SELECT state FROM devices WHERE id = ?1", params![id], |r| r.get::<_, String>(0))?)).unwrap()
}

#[test]
fn guard_1_a_device_request_from_outside_the_office_network_is_refused() {
    let office = start();
    let body = office.ask(&key(2), "latcon-estimating");
    let (status, said) = office.raw("POST", "/api/v1/devices/request", &body, None, &[("cf-connecting-ip", "203.0.113.9")]); // WHAT: marked as coming through the tunnel. WHY: the server's own mark of outside
    assert_eq!(status, 403, "{said}");
    assert_eq!(said["error"], "not_on_the_office_network");
    let (status, said) = office.raw("POST", "/api/v1/devices/request", &body, None, &[]);
    assert_eq!(status, 200, "from the office network it is taken: {said}");
}

#[test]
fn guard_2_the_first_computer_claims_the_server_only_while_unclaimed_and_within_the_window() {
    let office = start();
    let first = office.raw("POST", "/api/v1/devices/request", &office.ask(&key(1), "latcon-estimating"), None, &[]).1;
    assert_eq!(first["state"], "approved", "the first computer becomes the admin: {first}");
    let me = office.client(&signer(1)).device_me().unwrap();
    assert!(me.admin, "as an admin");
    let second = office.raw("POST", "/api/v1/devices/request", &office.ask(&key(2), "view"), None, &[]).1;
    assert_eq!(second["state"], "waiting", "once claimed, the next one waits: {second}");

    let late = start();
    late.store().set_setting("device_claim_until", "2020-01-01T00:00:00Z").unwrap(); // WHAT: the window long shut. WHY: a stranger a week later can't claim it
    let after = late.raw("POST", "/api/v1/devices/request", &late.ask(&key(3), "view"), None, &[]).1;
    assert_eq!(after["state"], "waiting", "outside the 24-hour window nobody is made admin: {after}");
}

#[test]
fn guard_3_only_an_approved_device_with_a_good_fresh_signature_gets_past_the_gate() {
    let office = start();
    let admin = first_admin(&office);
    let now = device::unix_now();
    let refused = |s: (u16, serde_json::Value), why: &str| {
        assert_eq!(s.0, 401, "{why}: {}", s.1);
        assert_eq!(s.1["error"], "device_unknown", "{why}");
    };
    let through = |s: (u16, serde_json::Value), why: &str| {
        assert_eq!(s.1["error"], "unauthorised", "{why}: the gate let it through to View's own sign-in check: {}", s.1);
    };

    // An approved device passes the gate (and meets View's own sign-in).
    let good = admin.sign(&office.pin, "GET", "/api/v1/me", b"", now).unwrap();
    through(office.raw("GET", "/api/v1/me", b"", Some(&good), &[]), "approved");
    // The same signature again is a replay.
    refused(office.raw("GET", "/api/v1/me", b"", Some(&good), &[]), "a replay");
    // Nothing signed at all.
    refused(office.raw("GET", "/api/v1/me", b"", None, &[]), "unsigned");
    // A bad signature: another key's, under this device's id.
    let mut forged = signer(9).sign(&office.pin, "GET", "/api/v1/me", b"", now).unwrap();
    forged.device = admin.key().id();
    refused(office.raw("GET", "/api/v1/me", b"", Some(&forged), &[]), "a bad signature");
    // Signed for another path.
    let elsewhere = admin.sign(&office.pin, "GET", "/api/v1/projects", b"", now).unwrap();
    refused(office.raw("GET", "/api/v1/me", b"", Some(&elsewhere), &[]), "signed for another target");
    // Outside ±300 s, either way.
    refused(office.raw("GET", "/api/v1/me", b"", Some(&admin.sign(&office.pin, "GET", "/api/v1/me", b"", now - 400).unwrap()), &[]), "too old");
    refused(office.raw("GET", "/api/v1/me", b"", Some(&admin.sign(&office.pin, "GET", "/api/v1/me", b"", now + 400).unwrap()), &[]), "too far ahead");
    // An unknown device.
    refused(office.raw("GET", "/api/v1/me", b"", Some(&signer(7).sign(&office.pin, "GET", "/api/v1/me", b"", now).unwrap()), &[]), "unknown");
    // Waiting, refused and removed devices.
    let other = signer(4);
    let id = other.key().id();
    assert_eq!(office.client(&other).ask_to_be_let_in("view", "Dana").unwrap().unwrap().state, "waiting");
    refused(office.raw("GET", "/api/v1/me", b"", Some(&other.sign(&office.pin, "GET", "/api/v1/me", b"", now).unwrap()), &[]), "waiting");
    office.client(&admin).refuse_device(&id).unwrap();
    refused(office.raw("GET", "/api/v1/me", b"", Some(&other.sign(&office.pin, "GET", "/api/v1/me", b"", now + 1).unwrap()), &[]), "refused");
    office.client(&admin).approve_device(&id, false).unwrap();
    through(office.raw("GET", "/api/v1/me", b"", Some(&other.sign(&office.pin, "GET", "/api/v1/me", b"", now + 2).unwrap()), &[]), "approved after all");
    office.client(&admin).remove_device(&id).unwrap();
    refused(office.raw("GET", "/api/v1/me", b"", Some(&other.sign(&office.pin, "GET", "/api/v1/me", b"", now + 3).unwrap()), &[]), "removed");
    assert_eq!(office.client(&other).device_me().unwrap().state, "removed", "a removed device is told so, and must ask again");
    // The body is signed: a changed body is refused.
    let body = br#"{"email":"a@b.test","password":"x"}"#;
    let signed = admin.sign(&office.pin, "POST", "/api/v1/auth/token", body, now + 4).unwrap();
    refused(office.raw("POST", "/api/v1/auth/token", br#"{"email":"c@d.test","password":"x"}"#, Some(&signed), &[]), "a changed body");
}

#[test]
fn guard_4_only_an_admin_device_runs_the_list_and_the_last_admin_is_never_lost() {
    let office = start();
    let admin = first_admin(&office);
    let dana = signer(5);
    let dana_id = dana.key().id();
    office.client(&dana).ask_to_be_let_in("latcon-estimating", "Dana").unwrap();
    let list = office.client(&admin).devices().unwrap();
    assert_eq!(list[0].id, dana_id, "newest waiting first");
    assert_eq!(list[0].state, "waiting");
    office.client(&admin).approve_device(&dana_id, false).unwrap();

    // An approved device that isn't an admin can do none of it.
    let not_admin = office.client(&dana);
    for refused in [
        not_admin.devices().map(|_| ()),
        not_admin.approve_device(&admin.key().id(), true).map(|_| ()),
        not_admin.refuse_device(&admin.key().id()).map(|_| ()),
        not_admin.remove_device(&admin.key().id()).map(|_| ()),
        not_admin.device_admin(&dana_id, true).map(|_| ()),
    ] {
        match refused {
            Err(hub::client::Trouble::Refused(p)) => assert_eq!(p.error, "not_an_admin"),
            other => panic!("a device that isn't an admin's was let through: {other:?}"),
        }
    }

    // The last admin can't be removed, refused or unmade.
    let me = admin.key().id();
    for lost in [office.client(&admin).remove_device(&me), office.client(&admin).refuse_device(&me), office.client(&admin).device_admin(&me, false), office.client(&admin).approve_device(&me, false)] {
        match lost {
            Err(hub::client::Trouble::Refused(p)) => assert_eq!(p.error, "last_admin", "{p}"),
            other => panic!("the last admin was lost: {other:?}"),
        }
    }
    // With a second admin, the first may step down.
    office.client(&admin).device_admin(&dana_id, true).unwrap();
    office.client(&admin).device_admin(&me, false).unwrap();
    assert!(!office.client(&admin).device_me().unwrap().admin);
    assert!(office.client(&dana).devices().is_ok(), "Dana runs the list now");

    // Every call is in the audit log.
    let log = office.store().audit_since(None, 100).unwrap();
    for action in ["the first computer was made the office's admin", "a computer asked to be let in", "approved a computer", "made a computer an admin", "made a computer not an admin"] {
        assert!(log.iter().any(|e| e.action == action), "{action} is written in the audit log");
    }
}

/// A person and a session for them, issued at `issued` and running out at
/// `expires`, written straight into the server's database.
fn session(office: &Office, role: &str, issued: &str, expires: &str) -> String {
    let store = office.store();
    let person = api::fresh_id("usr");
    let (token, hashed) = auth::new_token();
    store
        .with(|db| {
            db.execute(
                "INSERT INTO people (id, name, email, password, role, created) VALUES (?1, 'Sam', ?2, 'x', ?3, '2020-01-01T00:00:00Z')",
                params![person, format!("{person}@example.test"), role],
            )?;
            db.execute("INSERT INTO sessions (token, person, issued, expires) VALUES (?1, ?2, ?3, ?4)", params![hashed, person, issued, expires])?;
            Ok(())
        })
        .unwrap();
    token
}

#[test]
fn guard_8_a_seat_is_carried_over_only_with_a_session_from_before_that_is_still_valid() {
    let office = start();
    office.store().set_setting("device_claim_until", "2020-01-01T00:00:00Z").unwrap(); // WHAT: no claiming. WHY: only the migration may approve here
    let admin_seat = session(&office, "admin", "2026-01-01T00:00:00Z", "2099-01-01T00:00:00Z");
    let carried = office.client(&signer(11)).with_token(&admin_seat).ask_to_be_let_in("view", "Sam").unwrap().unwrap();
    assert_eq!(carried.state, "approved", "a seat signed in before 0.7.0 comes over");
    assert!(office.client(&signer(11)).device_me().unwrap().admin, "an administrator's seat as an admin device");

    let estimator = session(&office, "estimator", "2026-01-01T00:00:00Z", "2099-01-01T00:00:00Z");
    office.client(&signer(12)).with_token(&estimator).ask_to_be_let_in("view", "Est").unwrap();
    let me = office.client(&signer(12)).device_me().unwrap();
    assert!(me.state == "approved" && !me.admin, "an estimator's seat as an ordinary device");

    let again = office.raw("POST", "/api/v1/devices/request", &office.ask(&key(16), "view"), None, &[("Authorization", &format!("Bearer {admin_seat}"))]);
    assert_eq!(again.1["state"], "waiting", "a session carries one seat over, once: {}", again.1);

    let travelling = session(&office, "admin", "2026-01-01T00:00:00Z", "2099-01-01T00:00:00Z");
    let from_outside = office.raw("POST", "/api/v1/devices/request", &office.ask(&key(15), "view"), None, &[("cf-connecting-ip", "203.0.113.9"), ("Authorization", &format!("Bearer {travelling}"))]);
    assert_eq!(from_outside.1["state"], "approved", "a seat with a session from before comes over from outside too: {}", from_outside.1);
    assert!(!office.client(&signer(15)).device_me().unwrap().admin, "but never as an admin from outside");

    let expired = session(&office, "admin", "2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z");
    let stale = office.client(&signer(13)).with_token(&expired).ask_to_be_let_in("view", "Old").unwrap().unwrap();
    assert_eq!(stale.state, "waiting", "a session that has run out carries nothing over");

    let new_since = session(&office, "admin", "2099-01-01T00:00:00Z", "2099-02-01T00:00:00Z");
    let fresh = office.client(&signer(14)).with_token(&new_since).ask_to_be_let_in("view", "New").unwrap().unwrap();
    assert_eq!(fresh.state, "waiting", "a session issued after devices began carries nothing over");
}

#[test]
fn a_proof_is_bound_to_this_server_and_to_its_time() {
    let office = start();
    let elsewhere = Ask::signed(&key(2), &"0".repeat(64), "view", "", "PC", "dana", "", "", device::unix_now()).unwrap(); // WHAT: signed for another server's pin. WHY: a request made to an impostor
    let (status, said) = office.raw("POST", "/api/v1/devices/request", &serde_json::to_vec(&elsewhere).unwrap(), None, &[]);
    assert_eq!((status, said["error"].as_str()), (403, Some("bad_proof")));
    let old = Ask::signed(&key(2), &office.pin, "view", "", "PC", "dana", "", "", device::unix_now() - 1000).unwrap();
    let (status, _) = office.raw("POST", "/api/v1/devices/request", &serde_json::to_vec(&old).unwrap(), None, &[]);
    assert_eq!(status, 400, "a proof from long ago");
}

#[test]
fn a_join_from_outside_admits_the_computer_it_came_from() {
    let office = start();
    first_admin(&office);
    office.store().set_setting("joining", "code").unwrap();
    office.store().set_setting("join_code", "TRUSS-PLATE-42").unwrap();
    let ask = Ask::signed(&key(21), &office.pin, "view", "0.7.0", "TRUCK", "dana", "Dana", "windows-x64", device::unix_now()).unwrap();
    let typed = ["camber", "weld", "joist", "plate", "19"].join("-"); // WHAT: a made-up password, as the other tests use. WHY: long enough to be taken
    let body = serde_json::json!({ "code": "TRUSS-PLATE-42", "name": "Dana", "email": "dana@example.test", "password": typed, "device": ask });
    let (status, said) = office.raw("POST", "/api/v1/join", &serde_json::to_vec(&body).unwrap(), None, &[("cf-connecting-ip", "203.0.113.9")]);
    assert_eq!(status, 200, "{said}");
    assert_eq!(state_of(&office, &key(21).id()), "approved", "the code let the computer in too");
    let inside = office.raw("POST", "/api/v1/join", &serde_json::to_vec(&body).unwrap(), None, &[]);
    assert_eq!(inside.1["error"], "device_unknown", "inside the office a join needs an approved computer first");
}

#[test]
fn the_cell_is_told_only_about_approved_devices_and_carries_citadel_over_once() {
    let office = start();
    let admin = first_admin(&office);
    let store = office.store();
    let approved = hyperview_server::devices::for_the_cell(&store, &admin.key().id());
    assert_eq!(approved["state"], "approved");
    assert_eq!(approved["public_key"], admin.key().public_key_text());
    let waiting = signer(31);
    office.client(&waiting).ask_to_be_let_in("latcon-estimating", "Dana").unwrap();
    assert_eq!(hyperview_server::devices::for_the_cell(&store, &waiting.key().id()), serde_json::json!({"state":"waiting"}), "guard 7: never a key for a device not approved");
    assert_eq!(hyperview_server::devices::for_the_cell(&store, &"f".repeat(32)), serde_json::json!({"state":"unknown"}));

    let paired = vec![hyperview_server::devices::Paired { public_key: key(32).public_key_text(), name: "Mark".into(), machine: "MARK-PC".into(), paired_at: "2026-10-01T00:00:00Z".into(), paired_by: "Mark".into() }];
    assert_eq!(hyperview_server::devices::from_citadel(&store, &paired).unwrap(), Some(1));
    assert_eq!(state_of(&office, &key(32).id()), "approved");
    assert_eq!(hyperview_server::devices::from_citadel(&store, &paired).unwrap(), None, "sent once");
}

#[test]
fn the_office_identity_is_served_once_stored() {
    let office = start();
    let (status, _) = office.raw("GET", "/api/v1/office/identity", b"", None, &[]);
    assert_eq!(status, 404, "none yet");
    hyperview_server::devices::store_identity(&office.store(), "excalibur-office-cert/1\ncompany LatCon\n").unwrap();
    let (agent, _) = hub::pinned::agent(Some(&office.pin)).unwrap();
    let text = agent.build().get(&format!("{}/api/v1/office/identity", office.base())).call().unwrap().into_string().unwrap();
    assert!(text.starts_with("excalibur-office-cert/1"), "served as stored, with no device needed");
}

/// A pin book and a shared file in fresh places.
fn books() -> (hub::secure::PinBook, std::path::PathBuf) {
    let n: u64 = rand::random();
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("pins-{n:016x}"));
    (hub::secure::PinBook::at(dir.join("pins.json")), dir.join("office.json"))
}

#[test]
fn guard_5_an_app_never_accepts_a_changed_server_pin_even_one_another_app_kept() {
    let office = start();
    let (book, shared) = books();
    let plain = format!("http://127.0.0.1:{}", office.port);
    let also = hub::secure::Also { shared: Some(&shared), office_keys: &[], ..Default::default() };
    let first = hub::secure::open_with(&plain, &book, also).expect("a first contact on a local address pins");
    assert_eq!(first.pin(), Some(office.pin.as_str()));
    let kept = hub::office::read(&shared).unwrap().expect("shared with the other apps");
    assert_eq!(kept.pin, office.pin);

    let (fresh_book, _) = books(); // WHAT: a second app on the same PC, with no pin of its own. WHY: it must take the shared one
    hub::office::write(&shared, &hub::office::Shared { pin: "0".repeat(64), ..kept }).unwrap(); // WHAT: the shared pin made another server's. WHY: as if the certificate had changed
    match hub::secure::open_with(&plain, &fresh_book, also) {
        Err(hub::client::Trouble::Refused(p)) => assert_eq!(p.error, "certificate_changed"),
        other => panic!("a changed pin was accepted: {:?}", other.map(|c| c.pin().map(str::to_string))),
    }
}

#[test]
fn guard_6_a_server_once_seen_verified_is_never_accepted_unverified() {
    use ed25519_dalek::Signer as _;
    let office = start();
    let (book, shared) = books();
    let plain = format!("http://127.0.0.1:{}", office.port);
    let signing = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
    let public = hub::update::hex(&signing.verifying_key().to_bytes());
    let keys = [("office-test", public.as_str())];
    let statement = format!("excalibur-office-cert/1\ncompany LatCon\nlicense lic_1\npin {}\ncitadel-pin none\nissued 2026-11-11\n", office.pin);
    let cert = format!("{statement}signature office-test {}\n", hub::update::hex(&signing.sign(statement.as_bytes()).to_bytes()));
    hyperview_server::devices::store_identity(&office.store(), &cert).unwrap();
    hub::secure::open_with(&plain, &book, hub::secure::Also { shared: Some(&shared), office_keys: &keys, ..Default::default() }).expect("verified");
    assert!(hub::office::read(&shared).unwrap().unwrap().verified, "marked verified for every app");

    hyperview_server::devices::store_identity(&office.store(), "").unwrap(); // WHAT: the certificate gone. WHY: as an impostor without it would answer
    match hub::secure::open_with(&plain, &book, hub::secure::Also { shared: Some(&shared), office_keys: &keys, ..Default::default() }) {
        Err(hub::client::Trouble::Refused(p)) => assert_eq!(p.error, "office_unverified"),
        other => panic!("accepted unverified: {:?}", other.is_ok()),
    }
}

#[test]
fn a_first_contact_pins_only_on_a_local_address() {
    let (book, _) = books();
    match hub::secure::open("http://192.0.2.1:9", &book) {
        Err(_) => {} // WHAT: refused or unreachable. WHY: never pinned
        Ok(c) => assert!(c.pin().is_none(), "nothing outside was pinned"),
    }
    assert!(!hub::office::host_is_local("192.0.2.1", 8714), "a public address is never a first contact");
    assert!(hub::office::host_is_local("127.0.0.1", 8714));
}


// ---- the security review's findings, each kept as a regression test ----------

/// A key for another program, written straight into the database for the
/// person who owns `session_person`'s row; its token.
fn assistant_key(office: &Office, device: Option<&str>) -> (String, String) {
    let (key, digest) = auth::fresh_key();
    let id = api::fresh_id("key");
    let person = api::fresh_id("usr");
    office
        .store()
        .with(|db| {
            db.execute("INSERT INTO people (id, name, email, password, role, created) VALUES (?1, 'Bot', ?2, 'x', 'admin', '2020-01-01T00:00:00Z')", params![person, format!("{person}@example.test")])?;
            db.execute("INSERT INTO keys (id, digest, person, purpose, name, made, device) VALUES (?1, ?2, ?3, 'assistant', 'An assistant', '2026-11-11T00:00:00Z', ?4)", params![id, digest, person, device])?;
            Ok(())
        })
        .unwrap();
    (key, id)
}

#[test]
fn review_h1_the_assistant_door_needs_a_device_for_a_session_and_still_takes_a_key() {
    let office = start();
    first_admin(&office);
    let token = session(&office, "admin", &now_text(0), &now_text(30)); // WHAT: a session made since devices began, as if stolen. WHY: the attack
    let call = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_projects","arguments":{}}});
    let (status, said) = office.raw("POST", "/mcp", &serde_json::to_vec(&call).unwrap(), None, &[("Authorization", &format!("Bearer {token}")), ("cf-connecting-ip", "203.0.113.9")]);
    assert_eq!(status, 401, "a session with no device never reaches the assistant's door: {said}");
    assert_eq!(said["error"], "device_unknown");
    let (key, _) = assistant_key(&office, None);
    let (status, said) = office.raw("POST", "/mcp", &serde_json::to_vec(&call).unwrap(), None, &[("Authorization", &format!("Bearer {key}"))]);
    assert_eq!(status, 200, "an assistant's own key still works: {said}");
    assert!(said.get("result").is_some(), "{said}");
}

fn now_text(offset_days: i64) -> String {
    (time::OffsetDateTime::now_utc() + time::Duration::days(offset_days)).format(&time::format_description::well_known::Rfc3339).unwrap()
}

#[test]
fn review_h2_an_old_session_carries_one_seat_over_never_as_an_admin_from_outside_and_never_revives_one() {
    let office = start();
    office.store().set_setting("device_claim_until", "2020-01-01T00:00:00Z").unwrap(); // WHAT: no claiming. WHY: only the migration is under test
    let stolen = session(&office, "admin", "2026-01-01T00:00:00Z", "2099-01-01T00:00:00Z");
    let auth = format!("Bearer {stolen}");
    let outside = [("cf-connecting-ip", "203.0.113.9"), ("Authorization", auth.as_str())];
    let first = office.raw("POST", "/api/v1/devices/request", &office.ask(&key(40), "view"), None, &outside);
    assert_eq!(first.1["state"], "approved", "{}", first.1);
    assert!(!office.client(&signer(40)).device_me().unwrap().admin, "(d) never an admin from outside");
    let second = office.raw("POST", "/api/v1/devices/request", &office.ask(&key(41), "view"), None, &outside);
    assert_eq!(second.0, 403, "(a) the session carried its one seat over already: {}", second.1);

    let local_admin = session(&office, "admin", "2026-01-01T00:00:00Z", "2099-01-01T00:00:00Z");
    let admin = signer(42);
    office.client(&admin).with_token(&local_admin).ask_to_be_let_in("view", "Sam").unwrap();
    assert!(office.client(&admin).device_me().unwrap().admin, "an administrator's seat on the office network is an admin device");
    office.client(&admin).remove_device(&key(40).id()).unwrap();
    let back = office.raw("POST", "/api/v1/devices/request", &office.ask(&key(40), "view"), None, &[]);
    assert_eq!(back.1["state"], "removed", "(c) asking again never brings a removed computer back: {}", back.1);
    let fresh_session = session(&office, "admin", "2026-01-01T00:00:00Z", "2099-01-01T00:00:00Z");
    let migrated = office.raw("POST", "/api/v1/devices/request", &office.ask(&key(40), "view"), None, &[("Authorization", &format!("Bearer {fresh_session}"))]);
    assert_eq!(migrated.1["state"], "removed", "(c) nor does a session: {}", migrated.1);
    office.client(&admin).approve_device(&key(40).id(), false).unwrap();
    assert_eq!(state_of(&office, &key(40).id()), "approved", "only an admin's approval does");

    office.store().set_setting("devices_began", &now_text(-31)).unwrap(); // WHAT: a month and a day since devices began. WHY: (b)
    let late = session(&office, "admin", "2025-01-01T00:00:00Z", "2099-01-01T00:00:00Z");
    let closed = office.raw("POST", "/api/v1/devices/request", &office.ask(&key(43), "view"), None, &[("Authorization", &format!("Bearer {late}"))]);
    assert_eq!(closed.1["state"], "waiting", "(b) carrying over closes thirty days after devices began: {}", closed.1);
}

#[test]
fn review_h2e_the_update_doors_never_keep_a_session_alive() {
    let office = start();
    let ending = now_text(2); // WHAT: a session two days from its end. WHY: one a sliding check would push out to thirty days
    let token = session(&office, "estimator", "2026-01-01T00:00:00Z", &ending);
    let _ = office.raw("GET", "/api/v1/update/latest?platform=windows-x64", b"", None, &[("Authorization", &format!("Bearer {token}"))]);
    let after: String = office.store().with(|db| Ok(db.query_row("SELECT expires FROM sessions WHERE token = ?1", params![auth::token_hash(&token)], |r| r.get(0))?)).unwrap();
    assert_eq!(after, ending, "the open update door left the session exactly as it was");
}

#[test]
fn review_m1_computers_from_citadel_never_leave_the_office_without_an_admin_and_reach_only_the_cell() {
    let office = start();
    let paired = vec![hyperview_server::devices::Paired { public_key: key(50).public_key_text(), name: "Old".into(), machine: "PC".into(), paired_at: "2026-10-01T00:00:00Z".into(), paired_by: "Mark".into() }];
    assert_eq!(hyperview_server::devices::from_citadel(&office.store(), &paired).unwrap(), Some(1));
    let mark = office.raw("POST", "/api/v1/devices/request", &office.ask(&key(51), "view"), None, &[]);
    assert_eq!(mark.1["state"], "approved", "Mark's first ask still makes him the admin: {}", mark.1);
    assert!(office.client(&signer(51)).device_me().unwrap().admin);
    assert!(hyperview_server::devices::reopen_claim(&office.store()).is_err(), "claiming can't be reopened over an office that has an admin");
    assert_eq!(hyperview_server::devices::for_the_cell(&office.store(), &key(50).id())["state"], "approved", "the cell still takes it");
    let old = signer(50);
    let (status, said) = office.raw("GET", "/api/v1/me", b"", Some(&old.sign(&office.pin, "GET", "/api/v1/me", b"", device::unix_now()).unwrap()), &[]);
    assert_eq!((status, said["error"].as_str()), (401, Some("device_unknown")), "but View's API doesn't, until an admin widens it");
    office.client(&signer(51)).approve_device(&key(50).id(), false).unwrap();
    let (_, said) = office.raw("GET", "/api/v1/me", b"", Some(&old.sign(&office.pin, "GET", "/api/v1/me", b"", device::unix_now() + 1).unwrap()), &[]);
    assert_eq!(said["error"], "unauthorised", "widened: past the gate to View's own sign-in");
}

#[test]
fn review_m1_the_cell_migrates_only_on_its_first_connection_after_its_first_hello() {
    use hyperview_server::cell::Office as _;
    let n: u64 = rand::random();
    let data = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cell-{n:016x}"));
    let config = Config { data: data.clone(), ..Config::default() };
    let server = api::Server::new(Store::open(&config.database(), &config.blobs()).unwrap(), config);
    let paired = vec![hyperview_server::devices::Paired { public_key: key(55).public_key_text(), name: "Old".into(), machine: "PC".into(), paired_at: String::new(), paired_by: String::new() }];
    let before_hello = hyperview_server::cell::Answering::new(&server);
    assert_eq!(before_hello.migrate(&paired)["error"], "migrate_sealed", "never before the first hello");
    let crashed = hyperview_server::cell::Answering::new(&server);
    let _ = crashed.hello("1.1.0"); // WHAT: a hello, then the cell stops before migrating. WHY: review L5: the migration must not be lost
    let first = hyperview_server::cell::Answering::new(&server);
    let _ = first.hello("1.1.0");
    assert_eq!(first.migrate(&paired)["ok"], true, "the next connection after a hello still migrates");
    let later = hyperview_server::cell::Answering::new(&server);
    let _ = later.hello("1.1.0");
    assert_eq!(later.migrate(&paired)["error"], "migrate_sealed", "a later connection never does");
}

#[test]
fn review_m2_an_unproven_body_is_never_read_past_a_small_one() {
    let n: u64 = rand::random();
    let office = start_with(|c| c.largest_upload = 16 * 1024 * 1024, n); // WHAT: a big upload limit. WHY: the attack was buffering up to it
    let admin = first_admin(&office);
    let big = vec![b'x'; 3 * 1024 * 1024];
    let bogus = device::Signed { device: admin.key().id(), time: device::unix_now().to_string(), signature: "A".repeat(86) + "==", body: device::body_hash(&big) };
    let declared = office.raw_body("POST", "/api/v1/sets", &big, &bogus, true);
    assert!(declared == (401, "device_unknown".into()) || declared.0 == 0, "a declared hash with a bad signature is refused before the body is read: {declared:?}");
    let undeclared = office.raw_body("POST", "/api/v1/sets", &big, &bogus, false);
    assert!(undeclared == (401, "device_unknown".into()) || undeclared.0 == 0, "a body without its declared hash is refused, never read: {undeclared:?}");
    let good = admin.sign(&office.pin, "PUT", "/api/v1/cad/drawings/1/file", &big, device::unix_now()).unwrap();
    let proven = office.raw_body("PUT", "/api/v1/cad/drawings/1/file", &big, &good, true);
    assert!(proven.0 != 0 && proven.0 != 413 && proven.1 != "device_unknown", "a proven big body is read whole and passes the gate to the route's own checks: {proven:?}");
}

#[test]
fn review_m3_any_sign_of_a_proxy_is_outside_and_the_console_can_recover_an_office() {
    let office = start();
    for header in ["X-Forwarded-For", "X-Real-IP", "Forwarded", "Via"] {
        let (status, said) = office.raw("POST", "/api/v1/devices/request", &office.ask(&key(70), "view"), None, &[(header, "203.0.113.9")]);
        assert_eq!((status, said["error"].as_str()), (403, Some("not_on_the_office_network")), "{header}");
    }
    let admin = first_admin(&office);
    let dana = signer(71);
    office.client(&dana).ask_to_be_let_in("view", "Dana").unwrap();
    hyperview_server::devices::make_admin_here(&office.store(), &dana.key().id()).unwrap();
    assert!(office.client(&dana).device_me().unwrap().admin, "made an admin at the server");
    let sam = signer(73); // WHAT: a third computer. WHY: made an admin by the code its app shows
    office.client(&sam).ask_to_be_let_in("view", "Sam").unwrap(); // WHAT: it asks. WHY: on the list, waiting
    let code = hub::device::short_id(&sam.key().id()); // WHAT: its code as shown, "29AC 6B9D EFA9". WHY: what a person reads off the screen
    assert!(hyperview_server::devices::make_admin_here(&office.store(), "000000000000").is_err(), "a code that names nobody is refused"); // WHAT: refused. WHY: a typo is never silent
    assert_eq!(hyperview_server::devices::make_admin_here(&office.store(), &code).unwrap(), sam.key().id(), "the code with its spaces names the computer"); // WHAT: made by its spaced code. WHY: audit: the code as shown on screen
    assert_eq!(hyperview_server::devices::make_admin_here(&office.store(), &code.replace(' ', "").to_lowercase()).unwrap(), sam.key().id(), "and without them"); // WHAT: made by the code typed plain. WHY: likewise
    assert!(office.client(&sam).device_me().unwrap().admin, "made an admin by its code"); // WHAT: an admin now. WHY: the console did it
    for twin in ["abcdefabcdef00000000000000000001", "abcdefabcdef00000000000000000002"] {
        office.store().with(|db| Ok(db.execute("INSERT INTO devices (id, public_key, app, machine, os_user, state, admin, asked, changed, scope) VALUES (?1, ?2, 'view', 'PC', 'pat', 'waiting', 0, '2026-11-12T00:00:00Z', '2026-11-12T00:00:00Z', 'all')", params![twin, format!("key-of-{twin}")])?)).unwrap(); // WHAT: two computers sharing a code. WHY: one in 2^48, but never the wrong one made an admin
    }
    assert!(hyperview_server::devices::make_admin_here(&office.store(), "ABCD EFAB CDEF").unwrap_err().to_string().contains("more than one"), "a code that names two is refused"); // WHAT: refused. WHY: the full id is asked for
    hyperview_server::devices::reset_admins_here(&office.store()).unwrap();
    assert!(!office.client(&admin).device_me().unwrap().admin && !office.client(&dana).device_me().unwrap().admin && !office.client(&sam).device_me().unwrap().admin, "every admin unmade");
    let next = office.raw("POST", "/api/v1/devices/request", &office.ask(&key(72), "view"), None, &[]);
    assert_eq!(next.1["state"], "approved", "the next computer on the office network becomes the admin: {}", next.1);
    assert!(office.store().audit_since(None, 100).unwrap().iter().any(|e| e.email == "the server console"), "both are in the audit log");
}

#[test]
fn review_l1_head_meets_the_get_door() {
    let office = start();
    let admin = first_admin(&office);
    let dana = signer(60);
    office.client(&dana).ask_to_be_let_in("view", "Dana").unwrap();
    office.client(&admin).approve_device(&dana.key().id(), false).unwrap();
    let head = dana.sign(&office.pin, "HEAD", "/api/v1/devices", b"", device::unix_now()).unwrap();
    assert_eq!(office.raw_body("HEAD", "/api/v1/devices", b"", &head, false).0, 403, "HEAD on the admin list is refused like GET");
}

#[test]
fn review_l2_every_ask_counts_even_a_known_computers() {
    let office = start();
    let admin = first_admin(&office);
    let asker = key(80);
    let mut statuses = Vec::new();
    for _ in 0..6 {
        statuses.push(office.raw("POST", "/api/v1/devices/request", &office.ask(&asker, "view"), None, &[]).0); // WHAT: the same known key asking again and again. WHY: the flood
    }
    assert!(statuses.contains(&429), "asking again is counted: {statuses:?}");
    let _ = admin; // WHAT: the office's admin, made first. WHY: so the asker waits rather than claims
}

#[test]
fn review_l3_a_signature_is_bound_to_this_server_and_to_this_start() {
    let before = device::unix_now() - 2; // WHAT: a moment before the server starts. WHY: a signature from before its start
    let office = start();
    let admin = first_admin(&office);
    let elsewhere = admin.sign(&"0".repeat(64), "GET", "/api/v1/me", b"", device::unix_now()).unwrap();
    let (status, said) = office.raw("GET", "/api/v1/me", b"", Some(&elsewhere), &[]);
    assert_eq!((status, said["error"].as_str()), (401, Some("device_unknown")), "signed for another server's pin");
    let old = admin.sign(&office.pin, "GET", "/api/v1/me", b"", before).unwrap();
    let (status, said) = office.raw("GET", "/api/v1/me", b"", Some(&old), &[]);
    assert_eq!((status, said["error"].as_str()), (401, Some("clock_behind")), "signed before this server started: refused, and said why");
}

#[test]
fn review_l3_one_device_can_never_fill_the_replay_cache_for_everyone() {
    let replays = hyperview_server::devices::Replays::default();
    for i in 0..hyperview_server::devices::MOST_PER_DEVICE {
        assert!(replays.first_time("a", &format!("sig{i}"), 100, 100));
    }
    assert!(!replays.first_time("a", "one more", 100, 100), "its share is full");
    assert!(replays.first_time("b", "another device's", 100, 100), "another device is never locked out by it");
}

#[test]
fn review_l5_removing_a_computer_takes_back_the_keys_made_from_it() {
    let office = start();
    let admin = first_admin(&office);
    let dana = signer(90);
    office.client(&dana).ask_to_be_let_in("view", "Dana").unwrap();
    office.client(&admin).approve_device(&dana.key().id(), false).unwrap();
    let (key, _) = assistant_key(&office, Some(&dana.key().id()));
    office.client(&admin).remove_device(&dana.key().id()).unwrap();
    let call = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_projects","arguments":{}}});
    let (status, said) = office.raw("POST", "/mcp", &serde_json::to_vec(&call).unwrap(), None, &[("Authorization", &format!("Bearer {key}"))]);
    assert!(status == 401 || said.get("error").is_some(), "the key went with the computer: {status} {said}");
    assert_eq!(device::short_id("29ac6b9dfea94459862660c7ee55eebd"), "29AC 6B9D FEA9", "the code a person compares: twelve digits");
}


// ---- the re-review's findings ---------------------------------------------------

#[test]
fn rereview_m2_no_body_is_read_unproven_and_none_on_a_get() {
    let office = start();
    let admin = first_admin(&office);
    let waiting = signer(100);
    office.client(&waiting).ask_to_be_let_in("view", "Dana").unwrap();
    let body = vec![b'x'; 512 * 1024];
    let on_get = waiting.sign(&office.pin, "GET", "/api/v1/devices/me", &body, device::unix_now()).unwrap();
    let answer = office.raw_body("GET", "/api/v1/devices/me", &body, &on_get, true);
    assert!(answer == (401, "device_unknown".into()) || answer.0 == 0, "a waiting computer's GET with a body is refused unread: {answer:?}");
    let admin_get = admin.sign(&office.pin, "GET", "/api/v1/devices", &body, device::unix_now()).unwrap();
    let answer = office.raw_body("GET", "/api/v1/devices", &body, &admin_get, true);
    assert!(answer == (401, "device_unknown".into()) || answer.0 == 0, "no body on a GET, even an admin's: {answer:?}");
    let small = br#"{"admin":false}"#;
    let post = admin.sign(&office.pin, "POST", &format!("/api/v1/devices/{}/approve", waiting.key().id()), small, device::unix_now()).unwrap();
    let undeclared = office.raw_body("POST", &format!("/api/v1/devices/{}/approve", waiting.key().id()), small, &post, false);
    assert_eq!(undeclared, (401, "device_unknown".into()), "a body without its hash is refused, however small");
    let declared = office.raw_body("POST", &format!("/api/v1/devices/{}/approve", waiting.key().id()), small, &admin.sign(&office.pin, "POST", &format!("/api/v1/devices/{}/approve", waiting.key().id()), small, device::unix_now() + 1).unwrap(), true);
    assert_eq!(declared.0, 200, "the same body, declared, is taken: {declared:?}");
}

#[test]
fn rereview_low1_a_citadel_only_computer_never_holds_the_admin_seat() {
    let office = start();
    let paired = vec![hyperview_server::devices::Paired { public_key: key(110).public_key_text(), name: "Old".into(), machine: "PC".into(), paired_at: String::new(), paired_by: String::new() }];
    hyperview_server::devices::from_citadel(&office.store(), &paired).unwrap();
    office.store().with(|db| Ok(db.execute("UPDATE devices SET admin = 1 WHERE id = ?1", params![key(110).id()])?)).unwrap(); // WHAT: a Citadel-only computer marked admin. WHY: it must still not count
    let mark = office.raw("POST", "/api/v1/devices/request", &office.ask(&key(111), "view"), None, &[]);
    assert_eq!(mark.1["state"], "approved", "the office still counts as having no admin: {}", mark.1);
    let mark = signer(111);
    office.client(&mark).device_admin(&key(110).id(), true).unwrap();
    let list = office.client(&mark).devices().unwrap();
    let old = list.iter().find(|d| d.id == key(110).id()).unwrap();
    assert_eq!(old.scope, "all", "made an admin, it is approved for every app");
}

#[test]
fn rereview_low3_a_slow_clock_after_a_restart_is_told_so() {
    let before = device::unix_now() - 2;
    let office = start();
    let admin = first_admin(&office);
    let old = admin.sign(&office.pin, "GET", "/api/v1/me", b"", before).unwrap();
    let (status, said) = office.raw("GET", "/api/v1/me", b"", Some(&old), &[]);
    assert_eq!((status, said["error"].as_str()), (401, Some("clock_behind")), "{said}");
    assert!(said["message"].as_str().unwrap_or_default().contains("clock is behind"), "said plainly: {said}");
}

#[test]
fn rereview_low4_refusing_a_computer_takes_back_its_keys() {
    let office = start();
    let admin = first_admin(&office);
    let dana = signer(120);
    office.client(&dana).ask_to_be_let_in("view", "Dana").unwrap();
    office.client(&admin).approve_device(&dana.key().id(), false).unwrap();
    let (_, key_id) = assistant_key(&office, Some(&dana.key().id()));
    office.client(&admin).refuse_device(&dana.key().id()).unwrap();
    let left: i64 = office.store().with(|db| Ok(db.query_row("SELECT count(*) FROM keys WHERE id = ?1", params![key_id], |r| r.get(0))?)).unwrap();
    assert_eq!(left, 0, "the key went with the refusal");
}

// ---- the client review ------------------------------------------------------------

#[test]
fn client_review_another_name_for_a_server_never_re_pins_quietly_or_clears_verified() {
    let office = start();
    let (book, shared) = books();
    let alias = format!("http://localhost:{}", office.port); // WHAT: the same machine under another name. WHY: the attack reached it as localhost while the record said 127.0.0.1
    let known = hub::office::Shared { address: format!("https://127.0.0.1:{}", office.port), pin: "d".repeat(64), verified: true, other: serde_json::Map::new() }; // WHAT: the other apps know a server with another certificate, verified. WHY: as if an impostor answers here now
    hub::office::write(&shared, &known).unwrap();
    match hub::secure::open_with(&alias, &book, hub::secure::Also { shared: Some(&shared), ..Default::default() }) {
        Err(hub::client::Trouble::NewServer { seen, message }) => assert_eq!(seen, office.pin, "{message}"),
        other => panic!("re-pinned quietly: {:?}", other.map(|c| c.pin().map(str::to_string))),
    }
    assert_eq!(hub::office::read(&shared).unwrap(), Some(known.clone()), "the other apps' record is untouched, verified kept");
    assert!(!book.holds(&format!("localhost:{}", office.port)), "nothing kept for the alias either");

    let same = hub::office::Shared { address: format!("https://127.0.0.1:{}", office.port), pin: office.pin.clone(), verified: false, other: serde_json::Map::new() }; // WHAT: the record for this very server. WHY: reached by another name
    hub::office::write(&shared, &same).unwrap();
    let (book, _) = books();
    hub::secure::open_with(&alias, &book, hub::secure::Also { shared: Some(&shared), ..Default::default() }).expect("the same certificate under another name");
    assert_eq!(hub::office::read(&shared).unwrap().unwrap().address, same.address, "compared by pin: the record isn't re-addressed to the alias");

    hub::office::write(&shared, &known).unwrap();
    let (book, _) = books();
    match hub::secure::open_with(&alias, &book, hub::secure::Also { shared: Some(&shared), confirmed: Some(&office.pin), ..Default::default() }) {
        Err(hub::client::Trouble::Refused(p)) => assert_eq!(p.error, "office_unverified", "a verified record is never replaced by an unverified server, even confirmed"),
        other => panic!("a verified record was replaced: {:?}", other.is_ok()),
    }
    let unverified = hub::office::Shared { verified: false, ..known.clone() }; // WHAT: the same record, never verified. WHY: then a confirmed new server may take its place
    hub::office::write(&shared, &unverified).unwrap();
    let (book, _) = books();
    hub::secure::open_with(&alias, &book, hub::secure::Also { shared: Some(&shared), confirmed: Some(&office.pin), ..Default::default() }).expect("confirmed by the person, by its pin");
    assert_eq!(hub::office::read(&shared).unwrap().unwrap().pin, office.pin, "replaced only once confirmed");
}

#[test]
fn client_review_a_new_server_is_asked_about_even_when_only_this_app_knew_the_old_one() {
    let office = start();
    let n: u64 = rand::random();
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("book-only-{n:016x}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("pins.json"), format!("{{\"192.168.1.10:8714\":\"{}\"}}", "e".repeat(64))).unwrap(); // WHAT: this app linked to another server before, and no shared file. WHY: the app's own book counts too
    let book = hub::secure::PinBook::at(dir.join("pins.json"));
    match hub::secure::open_with(&format!("http://127.0.0.1:{}", office.port), &book, hub::secure::Also::default()) {
        Err(hub::client::Trouble::NewServer { seen, .. }) => assert_eq!(seen, office.pin),
        other => panic!("taken without asking: {:?}", other.is_ok()),
    }
    hub::secure::open_with(&format!("http://127.0.0.1:{}", office.port), &book, hub::secure::Also { confirmed: Some(&office.pin), ..Default::default() }).expect("once confirmed by its pin");
}

#[test]
fn cell_review_me_says_whether_the_office_can_still_be_claimed() {
    let office = start();
    let paired = vec![hyperview_server::devices::Paired { public_key: key(130).public_key_text(), name: "Old".into(), machine: "PC".into(), paired_at: String::new(), paired_by: String::new() }];
    hyperview_server::devices::from_citadel(&office.store(), &paired).unwrap(); // WHAT: a computer carried over from Citadel. WHY: it asks how it stands while the office has no admin
    let old = signer(130);
    assert!(office.client(&old).device_me().unwrap().claimable, "no admin yet, the window open");
    first_admin(&office);
    assert!(!office.client(&old).device_me().unwrap().claimable, "an admin now");
}
