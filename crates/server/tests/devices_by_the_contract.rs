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
    let n: u64 = rand::random();
    let data = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("devices-{n:016x}")); // WHAT: a fresh folder. WHY: tests run side by side
    let config = Config {
        name: "LatCon".into(),
        data: data.clone(),
        listen: "127.0.0.1:0".parse().unwrap(),
        tls: true, // WHAT: HTTPS. WHY: proofs are bound to the pin
        devices: hyperview_server::config::Devices::Required, // WHAT: devices required. WHY: the thing under test
        ..Config::default()
    };
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
    let good = admin.sign("GET", "/api/v1/me", b"", now).unwrap();
    through(office.raw("GET", "/api/v1/me", b"", Some(&good), &[]), "approved");
    // The same signature again is a replay.
    refused(office.raw("GET", "/api/v1/me", b"", Some(&good), &[]), "a replay");
    // Nothing signed at all.
    refused(office.raw("GET", "/api/v1/me", b"", None, &[]), "unsigned");
    // A bad signature: another key's, under this device's id.
    let mut forged = signer(9).sign("GET", "/api/v1/me", b"", now).unwrap();
    forged.device = admin.key().id();
    refused(office.raw("GET", "/api/v1/me", b"", Some(&forged), &[]), "a bad signature");
    // Signed for another path.
    let elsewhere = admin.sign("GET", "/api/v1/projects", b"", now).unwrap();
    refused(office.raw("GET", "/api/v1/me", b"", Some(&elsewhere), &[]), "signed for another target");
    // Outside ±300 s, either way.
    refused(office.raw("GET", "/api/v1/me", b"", Some(&admin.sign("GET", "/api/v1/me", b"", now - 301).unwrap()), &[]), "too old");
    refused(office.raw("GET", "/api/v1/me", b"", Some(&admin.sign("GET", "/api/v1/me", b"", now + 301).unwrap()), &[]), "too far ahead");
    // An unknown device.
    refused(office.raw("GET", "/api/v1/me", b"", Some(&signer(7).sign("GET", "/api/v1/me", b"", now).unwrap()), &[]), "unknown");
    // Waiting, refused and removed devices.
    let other = signer(4);
    let id = other.key().id();
    assert_eq!(office.client(&other).ask_to_be_let_in("view", "Dana").unwrap().unwrap().state, "waiting");
    refused(office.raw("GET", "/api/v1/me", b"", Some(&other.sign("GET", "/api/v1/me", b"", now).unwrap()), &[]), "waiting");
    office.client(&admin).refuse_device(&id).unwrap();
    refused(office.raw("GET", "/api/v1/me", b"", Some(&other.sign("GET", "/api/v1/me", b"", now + 1).unwrap()), &[]), "refused");
    office.client(&admin).approve_device(&id, false).unwrap();
    through(office.raw("GET", "/api/v1/me", b"", Some(&other.sign("GET", "/api/v1/me", b"", now + 2).unwrap()), &[]), "approved after all");
    office.client(&admin).remove_device(&id).unwrap();
    refused(office.raw("GET", "/api/v1/me", b"", Some(&other.sign("GET", "/api/v1/me", b"", now + 3).unwrap()), &[]), "removed");
    assert_eq!(office.client(&other).device_me().unwrap().state, "removed", "a removed device is told so, and must ask again");
    // The body is signed: a changed body is refused.
    let body = br#"{"email":"a@b.test","password":"x"}"#;
    let signed = admin.sign("POST", "/api/v1/auth/token", body, now + 4).unwrap();
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

    let expired = session(&office, "admin", "2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z");
    let stale = office.client(&signer(13)).with_token(&expired).ask_to_be_let_in("view", "Old").unwrap().unwrap();
    assert_eq!(stale.state, "waiting", "a session that has run out carries nothing over");

    let new_since = session(&office, "admin", "2099-01-01T00:00:00Z", "2099-02-01T00:00:00Z");
    let fresh = office.client(&signer(14)).with_token(&new_since).ask_to_be_let_in("view", "New").unwrap().unwrap();
    assert_eq!(fresh.state, "waiting", "a session issued after devices began carries nothing over");

    let from_outside = office.raw("POST", "/api/v1/devices/request", &office.ask(&key(15), "view"), None, &[("cf-connecting-ip", "203.0.113.9"), ("Authorization", &format!("Bearer {admin_seat}"))]);
    assert_eq!(from_outside.1["state"], "approved", "a seat with a session from before comes over from outside too: {}", from_outside.1);
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
    let also = hub::secure::Also { shared: Some(&shared), office_keys: &[] };
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
    hub::secure::open_with(&plain, &book, hub::secure::Also { shared: Some(&shared), office_keys: &keys }).expect("verified");
    assert!(hub::office::read(&shared).unwrap().unwrap().verified, "marked verified for every app");

    hyperview_server::devices::store_identity(&office.store(), "").unwrap(); // WHAT: the certificate gone. WHY: as an impostor without it would answer
    match hub::secure::open_with(&plain, &book, hub::secure::Also { shared: Some(&shared), office_keys: &keys }) {
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
