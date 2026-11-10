//! The Office Server's cad part over the wire: there only when the office's
//! signed licence switches it on, and then CAD's check-out and check-in work
//! for the office's own people (docs/OFFICE-SERVER.md, step 5).

use std::sync::Arc;

use ed25519_dalek::Signer;
use hyperview_server::api::{self, Server};
use hyperview_server::{auth, Config, Store};
use rusqlite::params;
use serde_json::Value;

const KEY: &str = "cad-test-2026"; // WHAT: this test's licence key's name. WHY: named in the licence

fn licensing() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[8u8; 32]) // WHAT: a made-up key. WHY: only this test's servers trust it
}

/// A licence with `modules`, signed with the test's key.
fn license(modules: &[&str]) -> String {
    let mut l = hub::license::License {
        id: "evl_cad".into(),
        company: "Example Steel".into(),
        edition: hub::license::Edition::Office,
        users: 0,
        updates_through: "2099-12-31".into(),
        issued: "2026-11-10".into(),
        note: String::new(),
        modules: modules.iter().map(|m| m.to_string()).collect(),
        key: KEY.into(),
        signature: String::new(),
    };
    l.signature = hub::update::hex(&licensing().sign(&l.signing_payload()).to_bytes());
    serde_json::to_string(&l).unwrap()
}

struct Office {
    base: String,
    sam: String, // WHAT: an administrator's session. WHY: one person
    eve: String, // WHAT: an estimator's session. WHY: another
    val: String, // WHAT: a viewer's session. WHY: reads only
    _runtime: tokio::runtime::Runtime,
}

/// An office with three people and the licence given, served on a free port.
fn an_office(licence: Option<String>) -> Office {
    let n: u64 = rand::random();
    let data = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cad-part-{n:016x}"));
    let config = Config {
        data,
        listen: "127.0.0.1:0".parse().unwrap(),
        license_keys_for_tests: vec![(KEY.into(), licensing().verifying_key().to_bytes())],
        ..Config::default()
    };
    let store = Store::open(&config.database(), &config.blobs()).unwrap();
    let hashed = auth::hash_password("brace-gusset-purlin-shim-42").unwrap();
    let mut tokens = Vec::new();
    store
        .with(|db| {
            for (id, email, role) in [("usr_sam", "sam@example.com", "admin"), ("usr_eve", "eve@example.com", "estimator"), ("usr_val", "val@example.com", "viewer")] {
                db.execute("INSERT INTO people (id, name, email, password, role, created) VALUES (?1, ?1, ?2, ?3, ?4, '2026-09-01T00:00:00Z')", params![id, email, hashed, role])?;
                let (token, digest) = auth::new_token();
                db.execute("INSERT INTO sessions (token, person, issued, expires) VALUES (?1, ?2, '2026-01-01T00:00:00Z', '2999-01-01T00:00:00Z')", params![digest, id])?;
                tokens.push(token);
            }
            Ok(())
        })
        .unwrap();
    let server = Arc::new(Server::new(store, config.clone()));
    if let Some(text) = licence {
        hyperview_server::license::install(&server, &text).expect("the licence");
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let listener = runtime.block_on(tokio::net::TcpListener::bind(config.listen)).unwrap();
    let address = listener.local_addr().unwrap();
    let app = api::router(server);
    runtime.spawn(async move {
        let _ = axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await;
    });
    Office { base: format!("http://{address}/api/v1"), sam: tokens.remove(0), eve: tokens.remove(0), val: tokens.remove(0), _runtime: runtime }
}

/// One request: its status and body.
fn ask(office: &Office, method: &str, path: &str, token: &str, body: Option<&[u8]>) -> (u16, String) {
    let request = ureq::request(method, &format!("{}{path}", office.base)).set("Authorization", &format!("Bearer {token}"));
    let answer = match body {
        Some(bytes) => request.set("Content-Type", "application/octet-stream").send_bytes(bytes),
        None => request.call(),
    };
    match answer {
        Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(code, r)) => (code, r.into_string().unwrap_or_default()),
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn without_the_licence_the_cad_part_is_not_there() {
    for licence in [None, Some(license(&[])), Some(license(&["citadel"]))] {
        let office = an_office(licence);
        assert_eq!(ask(&office, "GET", "/cad/drawings", &office.sam, None).0, 404, "not even for an administrator");
        assert_eq!(ask(&office, "POST", "/cad/drawings?name=x.excad", &office.sam, Some(b"x")).0, 404);
        let parts: Value = serde_json::from_str(&ask(&office, "GET", "/office", &office.sam, None).1).unwrap();
        assert!(parts["parts"].as_array().unwrap().iter().any(|p| p["name"] == "cad" && p["state"] == "off"));
    }
}

#[test]
fn the_office_checks_cad_drawings_out_and_in_with_its_own_accounts() {
    let office = an_office(Some(license(&["cad"])));
    let parts: Value = serde_json::from_str(&ask(&office, "GET", "/office", &office.val, None).1).unwrap();
    assert!(parts["parts"].as_array().unwrap().iter().any(|p| p["name"] == "cad" && p["state"] == "running"));

    let (code, made) = ask(&office, "POST", "/cad/drawings?folder=Jobs/2640&name=Base%20plate.excad", &office.eve, Some(b"v1"));
    assert_eq!(code, 200, "{made}");
    let id = serde_json::from_str::<Value>(&made).unwrap()["id"].as_i64().unwrap();
    assert_eq!(ask(&office, "POST", "/cad/drawings?name=x.excad", &office.val, Some(b"x")).0, 403, "a viewer opens, never adds");

    assert_eq!(ask(&office, "POST", &format!("/cad/drawings/{id}/lock"), &office.eve, None).0, 200);
    let (code, said) = ask(&office, "POST", &format!("/cad/drawings/{id}/lock"), &office.sam, None);
    assert_eq!(code, 409, "one person at a time");
    assert!(said.contains("eve@example.com"), "and says who: {said}");
    assert_eq!(ask(&office, "PUT", &format!("/cad/drawings/{id}/file?base=1"), &office.sam, Some(b"not theirs")).0, 409, "only who has it out saves");
    assert_eq!(ask(&office, "PUT", &format!("/cad/drawings/{id}/file?base=1&note=first"), &office.eve, Some(b"v2")).0, 200);
    assert_eq!(ask(&office, "PUT", &format!("/cad/drawings/{id}/file?base=1"), &office.eve, Some(b"old copy")).0, 409, "an old copy never overwrites a newer one");
    assert_eq!(ask(&office, "PUT", &format!("/cad/drawings/{id}/file"), &office.eve, Some(b"no base")).0, 400);

    assert_eq!(ask(&office, "GET", &format!("/cad/drawings/{id}/file"), &office.val, None).1, "v2", "everyone reads the newest");
    assert_eq!(ask(&office, "GET", &format!("/cad/drawings/{id}/file?version=1"), &office.val, None).1, "v1", "every version kept");
    let file = ureq::get(&format!("{}/cad/drawings/{id}/file", office.base)).set("Authorization", &format!("Bearer {}", office.val)).call().unwrap(); // WHAT: the newest, with its headers. WHY: Excalibur CAD reads which version came
    assert_eq!(file.header("x-version"), Some("2"), "the version, as CAD's own server says it");

    assert_eq!(ask(&office, "DELETE", &format!("/cad/drawings/{id}/lock?force=true"), &office.eve, None).0, 403, "forcing is an administrator's");
    assert_eq!(ask(&office, "DELETE", &format!("/cad/drawings/{id}/lock?force=true"), &office.sam, None).0, 200);
    assert_eq!(ask(&office, "POST", &format!("/cad/drawings/{id}/archive"), &office.eve, None).0, 403);
    assert_eq!(ask(&office, "POST", &format!("/cad/drawings/{id}/archive"), &office.sam, None).0, 200);
    assert_eq!(ask(&office, "GET", "/cad/drawings", &office.val, None).1, "[]", "archived is hidden");
    assert_eq!(ask(&office, "GET", "/cad/drawings/not-a-number", &office.val, None).0, 404);
}

#[test]
fn a_licence_signed_by_anyone_else_switches_nothing_on() {
    let office = an_office(None);
    let mut forged: hub::license::License = serde_json::from_str(&license(&["cad"])).unwrap(); // WHAT: a real licence's text. WHY: copied and changed
    let stranger = ed25519_dalek::SigningKey::from_bytes(&[6u8; 32]); // WHAT: someone else's key. WHY: not one this server trusts
    forged.signature = hub::update::hex(&stranger.sign(&forged.signing_payload()).to_bytes()); // WHAT: signed by them under our key's name. WHY: the forgery under test
    let (code, said) = {
        let r = ureq::post(&format!("{}/license", office.base)).set("Authorization", &format!("Bearer {}", office.sam)).send_json(serde_json::json!({ "license": serde_json::to_string(&forged).unwrap() }));
        match r {
            Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
            Err(ureq::Error::Status(c, r)) => (c, r.into_string().unwrap_or_default()),
            Err(e) => panic!("{e}"),
        }
    };
    assert_eq!(code, 400, "refused: {said}");
    let mut added: hub::license::License = serde_json::from_str(&license(&[])).unwrap(); // WHAT: a real licence without parts. WHY: then a part added after signing
    added.modules = vec!["cad".into()];
    let (code, _) = match ureq::post(&format!("{}/license", office.base)).set("Authorization", &format!("Bearer {}", office.sam)).send_json(serde_json::json!({ "license": serde_json::to_string(&added).unwrap() })) {
        Ok(r) => (r.status(), ()),
        Err(ureq::Error::Status(c, _)) => (c, ()),
        Err(e) => panic!("{e}"),
    };
    assert_eq!(code, 400, "a part added to a signed licence breaks its signature");
    assert_eq!(ask(&office, "GET", "/cad/drawings", &office.sam, None).0, 404, "and CAD stays off");
}

/// A request that says a huge drawing is coming and sends none of it: the
/// answer, or None when the server sat waiting for the body.
fn answered_before_the_body(office: &Office, path: &str, token: Option<&str>) -> Option<String> {
    answered_before("POST", office, path, token)
}

fn answered_before_the_body_put(office: &Office, path: &str, token: Option<&str>) -> Option<String> {
    answered_before("PUT", office, path, token)
}

fn answered_before(method: &str, office: &Office, path: &str, token: Option<&str>) -> Option<String> {
    use std::io::{Read, Write};
    let address = office.base.trim_start_matches("http://").split('/').next().unwrap().to_string(); // WHAT: host:port. WHY: a raw socket
    let mut tcp = std::net::TcpStream::connect(&address).unwrap();
    tcp.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap(); // WHAT: five seconds. WHY: a server reading the body first would still be waiting
    let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    write!(tcp, "{method} /api/v1{path} HTTP/1.1\r\nHost: {address}\r\n{auth}Content-Type: application/octet-stream\r\nContent-Length: 200000000\r\n\r\n").unwrap(); // WHAT: 200 MB promised. WHY: none of it sent
    let mut first = [0u8; 64];
    let n = tcp.read(&mut first).ok()?;
    Some(String::from_utf8_lossy(&first[..n]).to_string())
}

#[test]
fn a_drawing_is_never_read_before_the_server_knows_who_sent_it() {
    let off = an_office(None);
    let said = answered_before_the_body(&off, "/cad/drawings?name=x.excad", Some(&off.sam)).expect("answered without waiting for the body");
    assert!(said.starts_with("HTTP/1.1 404"), "no cad part: {said}");
    let on = an_office(Some(license(&["cad"])));
    let said = answered_before_the_body(&on, "/cad/drawings?name=x.excad", None).expect("answered without waiting for the body");
    assert!(said.starts_with("HTTP/1.1 401"), "a stranger: {said}");
    let said = answered_before_the_body_put(&on, "/cad/drawings/1/file?base=1", Some(&on.val)).expect("answered without waiting for the body");
    assert!(said.starts_with("HTTP/1.1 403"), "a viewer: {said}");
    assert!(answered_before_the_body(&on, "/cad/drawings?name=x.excad&folder=", None).is_some_and(|s| s.starts_with("HTTP/1.1 401")), "bad values are refused after the door, too");
}
