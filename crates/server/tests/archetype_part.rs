//! The Office Server's archetype part over the wire: there only when the
//! office's signed licence switches it on, and then Archetype's jobs check out
//! and in for the office's own people (docs/OFFICE-SERVER.md, step 6).

use std::sync::Arc;

use ed25519_dalek::Signer;
use hyperview_server::api::{self, Server};
use hyperview_server::{auth, Config, Store};
use rusqlite::params;
use serde_json::Value;

const KEY: &str = "arch-test-2026"; // WHAT: this test's licence key's name. WHY: named in the licence

fn licensing() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[8u8; 32]) // WHAT: a made-up key. WHY: only this test's servers trust it
}

/// A licence with `modules`, signed with the test's key.
fn license(modules: &[&str]) -> String {
    let mut l = hub::license::License {
        id: "evl_arch".into(),
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
    data: String, // WHAT: the server's data folder. WHY: a test puts a View file in its store

    sam: String, // WHAT: an administrator's session. WHY: one person
    eve: String, // WHAT: an estimator's session. WHY: another
    val: String, // WHAT: a viewer's session. WHY: reads only
    _runtime: tokio::runtime::Runtime,
}

/// An office with three people and the licence given, served on a free port.
fn an_office(licence: Option<String>) -> Office {
    let n: u64 = rand::random();
    let data = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("arch-part-{n:016x}"));
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
    Office { data: config.data.display().to_string(), base: format!("http://{address}/api/v1"), sam: tokens.remove(0), eve: tokens.remove(0), val: tokens.remove(0), _runtime: runtime }
}

/// One request: its status and body.
fn ask(office: &Office, method: &str, path: &str, token: &str, body: Option<&[u8]>) -> (u16, String) {
    let request = ureq::request(method, &format!("{}{path}", office.base)).set("Authorization", &format!("Bearer {token}"));
    let answer = match body {
        Some(bytes) => request.set("Content-Type", if bytes.starts_with(b"{") { "application/json" } else { "application/octet-stream" }).send_bytes(bytes),
        None => request.call(),
    };
    match answer {
        Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(code, r)) => (code, r.into_string().unwrap_or_default()),
        Err(e) => panic!("{e}"),
    }
}


fn sha(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect() // WHAT: a file's SHA-256 in hex. WHY: Archetype names files by it
}

#[test]
fn without_the_licence_the_archetype_part_is_not_there() {
    for licence in [None, Some(license(&[])), Some(license(&["cad"]))] {
        let office = an_office(licence);
        assert_eq!(ask(&office, "GET", "/archetype/jobs", &office.sam, None).0, 404, "not even for an administrator");
        assert_eq!(ask(&office, "PUT", &format!("/archetype/files/{}", sha(b"x")), &office.sam, Some(b"x")).0, 404);
    }
}

#[test]
fn the_office_checks_archetype_jobs_out_and_in_and_never_hands_out_view_s_files() {
    let office = an_office(Some(license(&["archetype"])));
    let (code, made) = ask(&office, "POST", "/archetype/jobs", &office.eve, Some(br#"{"name":"Akron Awning","number":"2640","machine":"EVE-PC"}"#));
    assert_eq!(code, 200, "{made}");
    let job: Value = serde_json::from_str(&made).unwrap();
    let id = job["id"].as_str().unwrap().to_string();
    assert_eq!(job["holder"]["name"], "usr_eve", "put up checked out to whoever put it there");
    assert_eq!(ask(&office, "POST", "/archetype/jobs", &office.eve, Some(br#"{"name":"akron awning","number":"2640"}"#)).0, 409, "one job by that name");
    assert_eq!(ask(&office, "POST", "/archetype/jobs", &office.val, Some(br#"{"name":"Viewer's job"}"#)).0, 403, "a viewer opens, never adds");

    let (model, under) = (b"the model".to_vec(), b"an underlay".to_vec());
    let missing = format!(r#"{{"digests":["{}","{}"]}}"#, sha(&model), sha(&under));
    let (_, said) = ask(&office, "POST", "/archetype/files/missing", &office.eve, Some(missing.as_bytes()));
    assert_eq!(serde_json::from_str::<Value>(&said).unwrap()["digests"].as_array().unwrap().len(), 2, "neither sent yet");
    assert_eq!(ask(&office, "PUT", &format!("/archetype/files/{}", sha(&model)), &office.eve, Some(b"not the model")).0, 400, "kept only if it is what it says");
    for bytes in [&model, &under] {
        assert_eq!(ask(&office, "PUT", &format!("/archetype/files/{}", sha(bytes)), &office.eve, Some(bytes)).0, 200);
    }
    let files = format!(r#"[{{"path":"job.frame","digest":"{}","bytes":9}},{{"path":"underlays/S-201.pdf","digest":"{}","bytes":11}}]"#, sha(&model), sha(&under));
    let checkin = format!(r#"{{"base_revision":0,"comment":"first","files":{files},"machine":"EVE-PC"}}"#);
    assert_eq!(ask(&office, "POST", &format!("/archetype/jobs/{id}/checkin"), &office.sam, Some(checkin.as_bytes())).0, 409, "only who has it checks in");
    let wrong_size = checkin.replace(r#""bytes":9"#, r#""bytes":999999"#); // WHAT: the model listed at a size it didn't arrive at. WHY: the job's sizes must be true
    assert_eq!(ask(&office, "POST", &format!("/archetype/jobs/{id}/checkin"), &office.eve, Some(wrong_size.as_bytes())).0, 400);
    let (code, said) = ask(&office, "POST", &format!("/archetype/jobs/{id}/checkin"), &office.eve, Some(checkin.as_bytes()));
    assert_eq!(code, 200, "{said}");
    assert_eq!(serde_json::from_str::<Value>(&said).unwrap()["revision"], 1);
    assert_eq!(ask(&office, "POST", &format!("/archetype/jobs/{id}/checkin"), &office.eve, Some(checkin.as_bytes())).0, 409, "given back after a check-in, so not again");

    assert_eq!(ask(&office, "GET", &format!("/archetype/files/{}", sha(&model)), &office.val, None).1.as_bytes(), model.as_slice(), "everyone reads a job's files");
    let (_, listed) = ask(&office, "GET", &format!("/archetype/jobs/{id}/files"), &office.val, None);
    assert_eq!(serde_json::from_str::<Value>(&listed).unwrap()["files"].as_array().unwrap().len(), 2);
    assert_eq!(ask(&office, "POST", &format!("/archetype/jobs/{id}/checkout"), &office.val, Some(br#"{"machine":"VAL-PC"}"#)).0, 403, "a viewer opens read-only, refused before anything is read");
    assert_eq!(ask(&office, "POST", &format!("/archetype/jobs/{id}/checkout"), &office.sam, Some(br#"{"machine":"SAM-PC"}"#)).0, 200);
    assert_eq!(ask(&office, "POST", &format!("/archetype/jobs/{id}/takeback"), &office.eve, None).0, 403, "taking back is an administrator's");
    assert_eq!(ask(&office, "POST", &format!("/archetype/jobs/{id}/takeback"), &office.sam, None).0, 200);
    let (_, history) = ask(&office, "GET", &format!("/archetype/jobs/{id}/history"), &office.val, None);
    let whats: Vec<String> = serde_json::from_str::<Vec<Value>>(&history).unwrap().iter().map(|h| h["what"].as_str().unwrap().to_string()).collect();
    assert_eq!(whats, ["took it back", "checked out", "checked in", "put on the server"], "newest first");

    // A View drawing set's file is in the same store; Archetype never hands it out by its fingerprint.
    let pdf = b"%PDF-1.4 a View drawing".to_vec();
    let digest = sha(&pdf);
    let blobs = std::path::PathBuf::from(&office.data).join("files").join(&digest[0..2]).join(&digest[2..4]); // WHAT: where the store keeps it. WHY: put there as View would
    std::fs::create_dir_all(&blobs).unwrap();
    std::fs::write(blobs.join(&digest), &pdf).unwrap();
    assert_eq!(ask(&office, "GET", &format!("/archetype/files/{digest}"), &office.sam, None).0, 404, "not one sent to Archetype");
    let (_, said) = ask(&office, "POST", "/archetype/files/missing", &office.eve, Some(format!(r#"{{"digests":["{digest}"]}}"#).as_bytes()));
    assert!(said.contains(&digest), "and nothing is learned about it: it reads as missing");
}

#[test]
fn a_file_is_never_read_before_the_server_knows_who_sent_it() {
    use std::io::{Read, Write};
    let office = an_office(Some(license(&["archetype"])));
    let address = office.base.trim_start_matches("http://").split('/').next().unwrap().to_string();
    let checkin = "POST /api/v1/archetype/jobs/job_none/checkin".to_string(); // WHAT: a check-in, from a viewer. WHY: refused before its list is read
    let put = format!("PUT /api/v1/archetype/files/{}", sha(b"x")); // WHAT: a file. WHY: the same
    for (line, token, expect) in [(&put, None, "401"), (&put, Some(office.val.clone()), "403"), (&checkin, Some(office.val.clone()), "403"), (&checkin, None, "401")] {
        let mut tcp = std::net::TcpStream::connect(&address).unwrap();
        tcp.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap(); // WHAT: five seconds. WHY: a server reading the body first would still be waiting
        let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
        write!(tcp, "{line} HTTP/1.1\r\nHost: {address}\r\n{auth}Content-Type: application/json\r\nContent-Length: 200000000\r\n\r\n").unwrap(); // WHAT: 200 MB promised, none sent. WHY: the door must answer first
        let mut first = [0u8; 32];
        let n = tcp.read(&mut first).expect("answered without waiting for the body");
        assert!(String::from_utf8_lossy(&first[..n]).starts_with(&format!("HTTP/1.1 {expect}")));
    }
}
