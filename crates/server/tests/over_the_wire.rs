//! The server, run for real and talked to over a socket by the real client.
//!
//! Handlers tested in isolation prove the handlers work. These prove the thing
//! works: a person signs in, a drawing goes up, markups come back, and the
//! takeoff says what the viewer would say — over HTTP, through the same client
//! the desktop uses.

use std::sync::Arc;

use hub::update::{Channel, Release};
use hub::Client;
use hyperview_server::api::{self, Server};
use hyperview_server::{auth, Config, Store};
use rusqlite::params;

/// A scratch directory that cleans up after itself.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let n: u64 = rand::random();
        let path = std::env::temp_dir().join(format!("hyperview-wire-{n:016x}"));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A running server, and how to reach it.
struct Running {
    base: String,
    _scratch: Scratch,
    _runtime: tokio::runtime::Runtime,
}

fn start() -> Running {
    start_with(true)
}

/// A server exactly as it arrives: running, reachable, and with nobody on it.
fn start_empty() -> Running {
    start_with(false)
}

fn start_with(people: bool) -> Running {
    let scratch = Scratch::new();
    let config = Config {
        name: "Example Steel".into(),
        data: scratch.0.clone(),
        // Port zero: the operating system picks a free one, so these tests can
        // run beside each other without fighting over a number.
        listen: "127.0.0.1:0".parse().unwrap(),
        ..Config::default()
    };
    let store = Store::open(&config.database(), &config.blobs()).expect("a store");

    // One administrator and one ordinary seat, so both sides of the permission
    // checks have somebody to be.
    for (name, email, password, role) in if people {
        [
        ("Sam", "sam@example.com", "brace-gusset-purlin-shim-42", "admin"),
        ("Estimator", "est@example.com", "camber-weld-joist-plate-19", "estimator"),
        ("Looker", "look@example.com", "channel-stud-truss-web-77", "viewer"),
        ]
        .as_slice()
    } else {
        [].as_slice()
    } {
        let hashed = auth::hash_password(password).unwrap();
        let now = api::now();
        store
            .with(|db| {
                db.execute(
                    "INSERT INTO people (id, name, email, password, role, created)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![api::fresh_id("usr"), name, email, hashed, role, now],
                )?;
                Ok(())
            })
            .unwrap();
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");

    let listener = runtime
        .block_on(tokio::net::TcpListener::bind(config.listen))
        .expect("a socket");
    let address = listener.local_addr().expect("an address");
    let app = api::router(Arc::new(Server::new(store, config)));
    runtime.spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    Running {
        base: format!("http://{address}"),
        _scratch: scratch,
        _runtime: runtime,
    }
}

fn signed_in(server: &Running, who: &str, password: &str) -> Client {
    let mut client = Client::new(&server.base);
    client.sign_in(who, password).expect("sign in");
    client
}

/// A small but real PDF: two pages, so a drawing set has sheets.
fn a_drawing() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets = Vec::new();
    let add = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &str| {
        offsets.push(out.len());
        out.extend_from_slice(body.as_bytes());
    };
    add(&mut out, &mut offsets, "1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n");
    add(
        &mut out,
        &mut offsets,
        "2 0 obj\n<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>\nendobj\n",
    );
    add(
        &mut out,
        &mut offsets,
        "3 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]>>\nendobj\n",
    );
    add(
        &mut out,
        &mut offsets,
        "4 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]>>\nendobj\n",
    );
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// Uploads a drawing set the way a desktop would, with a multipart body built
/// by hand so the test exercises the real parsing.
fn upload(server: &Running, token: &str, project: &str, bytes: &[u8]) -> hub::DrawingSet {
    let boundary = "----hyperviewtest";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"project\"\r\n\r\n{project}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nStructural\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"structural.pdf\"\r\nContent-Type: application/pdf\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let response = ureq::post(&format!("{}/api/v1/sets", server.base))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body)
        .expect("upload");
    response.into_json().expect("a drawing set")
}

/// A W12x26 with its weight in the column his chest uses.
fn a_beam(length_points: f64) -> String {
    use base64::Engine;
    let mut m = annot::Markup::new(annot::Subtype::Line);
    m.set("IT", pdf::Object::name("LineDimension"));
    m.set_subject("W12x26");
    m.set(
        "BSIColumnData",
        pdf::Object::Array(vec![
            pdf::Object::text("26"),
            pdf::Object::text(""),
            pdf::Object::text(""),
            pdf::Object::text(""),
            pdf::Object::text(""),
            pdf::Object::text(""),
        ]),
    );
    m.set_line([0.0, 0.0], [length_points, 0.0]);
    let mut bytes = Vec::new();
    pdf::write::write_object(&pdf::Object::Dict(m.dict), &mut bytes);
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn set_scale(server: &Running, set: &str, page: usize, scale: Option<&str>) {
    // The desktop sets a sheet's scale as part of saving; a test reaches in.
    let path = std::path::Path::new(&server._scratch.0).join("hyperview.sqlite");
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute(
        "UPDATE sheets SET scale = ?1 WHERE set_id = ?2 AND page = ?3",
        params![scale, set, page as i64],
    )
    .unwrap();
}

// ---- the tests -----------------------------------------------------------

#[test]
fn a_server_says_what_it_is_before_anybody_signs_in() {
    let server = start();
    let client = Client::new(&server.base);
    let health = client.health().expect("health");
    assert!(health.ok);
    assert_eq!(health.name, "Example Steel");
    assert_eq!(health.api_version, hub::API_VERSION);
}

#[test]
fn a_wrong_password_and_an_unknown_address_are_told_apart_by_nobody() {
    let server = start();
    let mut client = Client::new(&server.base);
    let wrong = client
        .sign_in("sam@example.com", "not the password")
        .expect_err("it must refuse");
    let unknown = client
        .sign_in("nobody@example.com", "not the password")
        .expect_err("it must refuse");
    assert_eq!(
        wrong.to_string(),
        unknown.to_string(),
        "telling somebody which half they got right is telling them half of it"
    );
}

/// The one that matters once a server is on the internet.
///
/// Unit tests prove the counter counts. This proves the door is actually
/// shut: a real client, over a real socket, guessing at a real account, gets
/// told to wait -- and the right password does not get them in either, which
/// is what "wait" has to mean or it is only a suggestion.
#[test]
fn guessing_at_a_password_stops_working_after_a_while() {
    let server = start();
    let mut client = Client::new(&server.base);

    // Five are free, because people mistype their own passwords.
    for attempt in 1..=5 {
        let said = client
            .sign_in("sam@example.com", "not the password")
            .expect_err("it must refuse")
            .to_string();
        assert!(
            !said.contains("Too many"),
            "attempt {attempt} should still be free: {said}"
        );
    }

    let said = client
        .sign_in("sam@example.com", "not the password")
        .expect_err("it must refuse")
        .to_string();
    assert!(said.contains("Too many wrong tries"), "{said}");
    assert!(said.contains("administrator"), "it says what to do: {said}");
    assert!(
        !said.to_lowercase().contains("left"),
        "never a hint about how many tries remain: {said}"
    );

    // And the real password is refused too, for as long as the wait lasts.
    // A limit that the right answer walks straight past is not a limit; it
    // is a way to find out when you have guessed right.
    let even_right = client
        .sign_in("sam@example.com", "brace-gusset-purlin-shim-42")
        .expect_err("held off, even with the right password")
        .to_string();
    assert!(even_right.contains("Too many wrong tries"), "{even_right}");

    // Somebody else's account is untouched. One person guessing must not
    // lock out the shop.
    let mut other = Client::new(&server.base);
    other
        .sign_in("est@example.com", "camber-weld-joist-plate-19")
        .expect("a different account is not held up by somebody else's guessing");
}

#[test]
fn nothing_is_readable_without_a_token() {
    let server = start();
    let client = Client::new(&server.base);
    assert!(client.projects().is_err());
    assert!(client.takeoff("set_whatever").is_err());
}

#[test]
fn a_drawing_goes_up_and_the_sheets_come_back() {
    let server = start();
    let client = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let project = client.create_project("6742", "Elm West Theatre").expect("project");
    assert_eq!(project.number, "6742");

    let bytes = a_drawing();
    let set = upload(&server, client.token().unwrap(), &project.id, &bytes);
    assert_eq!(set.sheets, 2);
    assert_eq!(set.bytes, bytes.len() as u64);

    let sheets = client.sheets(&set.id).expect("sheets");
    assert_eq!(sheets.len(), 2);
    assert_eq!(sheets[0].width, 3024.0, "42 inches across");
    assert!(sheets[0].scale.is_none(), "no scale until somebody sets one");

    // The same bytes come back down.
    let down = client.download(&set.id, None).expect("download").expect("bytes");
    assert_eq!(down, bytes);

    // And a client that already has them is told so instead.
    let again = client.download(&set.id, Some(&set.digest)).expect("download");
    assert!(again.is_none(), "it should not send sixty megabytes twice");
}

#[test]
fn a_viewer_can_read_a_drawing_and_cannot_mark_it_up() {
    let server = start();
    let admin = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let project = admin.create_project("6742", "Elm West").expect("project");
    let set = upload(&server, admin.token().unwrap(), &project.id, &a_drawing());

    let looker = signed_in(&server, "look@example.com", "channel-stud-truss-web-77");
    assert!(looker.sheets(&set.id).is_ok(), "a viewer may look");
    let refused = looker
        .push_markups(
            &set.id,
            &[hub::NewMarkup {
                page: 0,
                dictionary: a_beam(720.0),
                replaces: None,
            }],
        )
        .expect_err("a viewer may not draw");
    assert!(refused.to_string().contains("not allowed"), "{refused}");
}

#[test]
fn markups_go_up_and_the_takeoff_matches_what_the_viewer_would_say() {
    let server = start();
    let client = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");
    let admin = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let project = admin.create_project("6742", "Elm West").expect("project");
    let set = upload(&server, admin.token().unwrap(), &project.id, &a_drawing());
    set_scale(&server, &set.id, 0, Some("1/4\" = 1'-0\""));

    // Two forty-foot beams: 720 points at quarter inch scale.
    let page = client
        .push_markups(
            &set.id,
            &[
                hub::NewMarkup { page: 0, dictionary: a_beam(720.0), replaces: None },
                hub::NewMarkup { page: 0, dictionary: a_beam(720.0), replaces: None },
            ],
        )
        .expect("push");
    assert_eq!(page.markups.len(), 2);
    assert_eq!(page.revision, 1);
    assert_eq!(page.markups[0].subject, "W12x26");
    assert_eq!(page.markups[0].author, "Estimator", "attribution follows the seat");

    let takeoff = client.takeoff(&set.id).expect("takeoff");
    // Eighty feet of W12x26 is 2,080 lb — his number, off his own tool.
    assert_eq!(takeoff.pounds, Some(2080.0));
    assert_eq!(takeoff.tons, Some(1.04));
    assert_eq!(takeoff.left_out, 0);
    assert!(!takeoff.is_short());
    assert_eq!(takeoff.rows[0].caption, "40'-0\"");
    assert_eq!(takeoff.groups.len(), 1);
    assert_eq!(takeoff.groups[0].subject, "W12x26");
}

#[test]
fn a_takeoff_off_an_unscaled_sheet_is_reported_short_rather_than_counted_as_nothing() {
    let server = start();
    let admin = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let project = admin.create_project("6742", "Elm West").expect("project");
    let set = upload(&server, admin.token().unwrap(), &project.id, &a_drawing());
    // Page one has a scale. Page two does not.
    set_scale(&server, &set.id, 0, Some("1/4\" = 1'-0\""));

    admin
        .push_markups(
            &set.id,
            &[
                hub::NewMarkup { page: 0, dictionary: a_beam(720.0), replaces: None },
                hub::NewMarkup { page: 1, dictionary: a_beam(720.0), replaces: None },
            ],
        )
        .expect("push");

    let takeoff = admin.takeoff(&set.id).expect("takeoff");
    assert!(takeoff.is_short(), "the totals are short and must say so");
    assert_eq!(takeoff.left_out, 1);
    assert_eq!(takeoff.sheets_without_a_scale, vec!["Sheet 2".to_string()]);
    // Only the scaled one counted. Not two thousand and eighty.
    assert_eq!(takeoff.pounds, Some(1040.0));

    let unscaled = takeoff.rows.iter().find(|r| r.page == 1).unwrap();
    assert!(!unscaled.scaled);
    assert_eq!(unscaled.length_feet, None, "absent, never zero");
    assert_eq!(unscaled.pounds, None, "absent, never zero");
}

#[test]
fn a_markup_comes_back_as_the_annotation_it_went_up_as() {
    use base64::Engine;
    let server = start();
    let client = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let project = client.create_project("6742", "Elm West").expect("project");
    let set = upload(&server, client.token().unwrap(), &project.id, &a_drawing());

    let sent = a_beam(720.0);
    client
        .push_markups(
            &set.id,
            &[hub::NewMarkup { page: 0, dictionary: sent.clone(), replaces: None }],
        )
        .expect("push");

    let page = client.markups(&set.id, 0).expect("markups");
    assert_eq!(page.markups.len(), 1);
    // Byte for byte. There is no lossy middle format for fidelity to leak out
    // of, which is the whole reason a markup travels as its own dictionary.
    assert_eq!(page.markups[0].dictionary, sent);

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(page.markups[0].dictionary.as_bytes())
        .unwrap();
    let back = pdf::Reader::new(&bytes).object().unwrap();
    let dict = back.as_dict().unwrap().clone();
    let markup = annot::Markup { dict, picture: None };
    assert_eq!(markup.subject(), "W12x26");
    assert_eq!(markup.points().len(), 2);
}

#[test]
fn syncing_from_a_revision_brings_back_only_what_is_new() {
    let server = start();
    let client = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let project = client.create_project("6742", "Elm West").expect("project");
    let set = upload(&server, client.token().unwrap(), &project.id, &a_drawing());

    let first = client
        .push_markups(&set.id, &[hub::NewMarkup { page: 0, dictionary: a_beam(720.0), replaces: None }])
        .expect("push");
    let second = client
        .push_markups(&set.id, &[hub::NewMarkup { page: 0, dictionary: a_beam(360.0), replaces: None }])
        .expect("push");
    assert_eq!(second.revision, first.revision + 1);

    let everything = client.markups(&set.id, 0).expect("markups");
    assert_eq!(everything.markups.len(), 2);

    let only_new = client.markups(&set.id, first.revision).expect("markups");
    assert_eq!(only_new.markups.len(), 1, "just the second one");
    assert_eq!(only_new.revision, second.revision);
}

#[test]
fn a_removal_travels_as_a_record_so_an_old_client_learns_about_it() {
    let server = start();
    let client = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let project = client.create_project("6742", "Elm West").expect("project");
    let set = upload(&server, client.token().unwrap(), &project.id, &a_drawing());

    let first = client
        .push_markups(&set.id, &[hub::NewMarkup { page: 0, dictionary: a_beam(720.0), replaces: None }])
        .expect("push");
    let original = first.markups[0].id.clone();

    let replaced = client
        .push_markups(
            &set.id,
            &[hub::NewMarkup {
                page: 0,
                dictionary: a_beam(360.0),
                replaces: Some(original.clone()),
            }],
        )
        .expect("replace");

    // Somebody who had only seen the first revision learns both halves: the
    // one that went away and the one that took its place.
    let caught_up = client.markups(&set.id, first.revision).expect("markups");
    let gone = caught_up.markups.iter().find(|m| m.id == original).expect("the removal");
    assert!(gone.removed, "a removal is a record, not an absence");
    assert!(caught_up.markups.iter().any(|m| !m.removed));
    assert_eq!(replaced.revision, first.revision + 1);

    // And it is out of the takeoff.
    let takeoff = client.takeoff(&set.id).expect("takeoff");
    assert_eq!(takeoff.picks, 1);
}

#[test]
fn a_batch_with_one_bad_markup_in_it_saves_none_of_them() {
    let server = start();
    let client = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let project = client.create_project("6742", "Elm West").expect("project");
    let set = upload(&server, client.token().unwrap(), &project.id, &a_drawing());

    let refused = client
        .push_markups(
            &set.id,
            &[
                hub::NewMarkup { page: 0, dictionary: a_beam(720.0), replaces: None },
                hub::NewMarkup { page: 0, dictionary: "bm90IGEgZGljdGlvbmFyeQ==".into(), replaces: None },
            ],
        )
        .expect_err("it must refuse the batch");
    assert!(refused.to_string().contains("Nothing was saved"), "{refused}");

    let page = client.markups(&set.id, 0).expect("markups");
    assert!(page.markups.is_empty(), "the good one must not be left behind");
}

#[test]
fn something_that_is_not_a_pdf_is_refused_with_a_reason() {
    let server = start();
    let client = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let project = client.create_project("6742", "Elm West").expect("project");

    let boundary = "----x";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"project\"\r\n\r\n{}\r\n",
            project.id
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"notes.txt\"\r\n\r\nthis is not a drawing\r\n--{boundary}--\r\n"
        )
        .as_bytes(),
    );
    let result = ureq::post(&format!("{}/api/v1/sets", server.base))
        .set("Authorization", &format!("Bearer {}", client.token().unwrap()))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body);
    match result {
        Err(ureq::Error::Status(400, response)) => {
            let problem: hub::Problem = response.into_json().unwrap();
            assert!(problem.message.contains("file of record"), "{}", problem.message);
        }
        other => panic!("it should have been refused: {other:?}"),
    }
}

#[test]
fn the_api_describes_itself_without_anybody_signing_in() {
    let server = start();
    let document: serde_json::Value = ureq::get(&format!("{}/api/v1/openapi.json", server.base))
        .call()
        .expect("the description")
        .into_json()
        .expect("json");
    assert_eq!(document["openapi"], "3.1.0");
    assert!(document["paths"]["/sets/{id}/takeoff"].is_object());
    let overview = document["info"]["description"].as_str().unwrap();
    assert!(overview.contains("left_out"), "the warning has to be where people read it");
}

// ---- updates -------------------------------------------------------------

fn publish(server: &Running, token: &str, release: &Release, installer: &[u8]) -> ureq::Response {
    let boundary = "----rel";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"manifest\"\r\n\r\n{}\r\n",
            serde_json::to_string(release).unwrap()
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"setup.exe\"\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(installer);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    ureq::post(&format!("{}/api/v1/admin/releases", server.base))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body)
        .expect("publish")
}

fn a_release(installer: &[u8], version: &str) -> Release {
    use sha2::{Digest, Sha256};
    Release {
        version: version.into(),
        channel: Channel::Stable,
        platform: "windows-x64".into(),
        published: "2026-09-18T12:00:00Z".into(),
        notes: "Dynamic Fill.".into(),
        download: String::new(),
        bytes: installer.len() as u64,
        digest: hub::update::hex(&Sha256::digest(installer)),
        // Signed elsewhere. The server does not check this — the seats do —
        // but it will not accept a release with nothing in it at all.
        signature: "00".repeat(64),
        key: "mesafab-2026".into(),
        minimum_api_version: 1,
    }
}

#[test]
fn a_release_is_not_offered_until_somebody_says_it_is_ready() {
    let server = start();
    let admin = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let installer = b"pretend this is an installer".to_vec();
    let release = a_release(&installer, "1.5.0");
    publish(&server, admin.token().unwrap(), &release, &installer);

    // Published, but nobody has tried it yet.
    let offer = admin
        .update_offer("1.4.2", Channel::Stable, "windows-x64")
        .expect("offer");
    assert!(offer.release.is_none(), "the office carries on working");

    ureq::post(&format!("{}/api/v1/admin/releases/1.5.0/ready", server.base))
        .set("Authorization", &format!("Bearer {}", admin.token().unwrap()))
        .send_json(serde_json::json!({ "platform": "windows-x64", "ready": true }))
        .expect("mark ready");

    let offer = admin
        .update_offer("1.4.2", Channel::Stable, "windows-x64")
        .expect("offer");
    let release = offer.release.expect("now it is offered");
    assert_eq!(release.version, "1.5.0");
    assert!(release.download.contains("/update/1.5.0/download"));

    // And a seat already on it is not told to install it again.
    let offer = admin
        .update_offer("1.5.0", Channel::Stable, "windows-x64")
        .expect("offer");
    assert!(offer.release.is_none());
}

#[test]
fn a_pinned_office_stays_where_it_is_put() {
    let server = start();
    let admin = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let token = admin.token().unwrap().to_string();
    for version in ["1.5.0", "1.6.0"] {
        let installer = format!("installer for {version}").into_bytes();
        publish(&server, &token, &a_release(&installer, version), &installer);
        ureq::post(&format!("{}/api/v1/admin/releases/{version}/ready", server.base))
            .set("Authorization", &format!("Bearer {token}"))
            .send_json(serde_json::json!({ "ready": true }))
            .expect("ready");
    }

    // Unpinned, the newest wins.
    let offer = admin.update_offer("1.4.2", Channel::Stable, "windows-x64").unwrap();
    assert_eq!(offer.release.map(|r| r.version), Some("1.6.0".to_string()));

    // Pinned, the office gets exactly what it was told to have — nothing
    // newer, however keen the server is about it.
    ureq::post(&format!("{}/api/v1/admin/pin", server.base))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(serde_json::json!({ "version": "1.5.0" }))
        .expect("pin");
    let offer = admin.update_offer("1.4.2", Channel::Stable, "windows-x64").unwrap();
    assert_eq!(offer.pinned_to, Some("1.5.0".to_string()));
    assert_eq!(offer.release.map(|r| r.version), Some("1.5.0".to_string()));

    // And unpinning lets them move again.
    ureq::post(&format!("{}/api/v1/admin/pin", server.base))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(serde_json::json!({ "version": "" }))
        .expect("unpin");
    let offer = admin.update_offer("1.4.2", Channel::Stable, "windows-x64").unwrap();
    assert_eq!(offer.release.map(|r| r.version), Some("1.6.0".to_string()));
}

#[test]
fn an_installer_that_does_not_match_its_manifest_is_refused_at_the_door() {
    let server = start();
    let admin = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let installer = b"the real installer".to_vec();
    let release = a_release(&installer, "1.5.0");
    let meddled = b"a different installer".to_vec();

    let boundary = "----rel";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"manifest\"\r\n\r\n{}\r\n",
            serde_json::to_string(&release).unwrap()
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"s.exe\"\r\n\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(&meddled);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let result = ureq::post(&format!("{}/api/v1/admin/releases", server.base))
        .set("Authorization", &format!("Bearer {}", admin.token().unwrap()))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body);
    match result {
        Err(ureq::Error::Status(400, response)) => {
            let problem: hub::Problem = response.into_json().unwrap();
            assert!(problem.message.contains("does not match"), "{}", problem.message);
        }
        other => panic!("it should have been refused: {other:?}"),
    }
}

#[test]
fn an_ordinary_seat_cannot_publish_a_release() {
    let server = start();
    let estimator = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");
    let installer = b"anything".to_vec();
    let release = a_release(&installer, "9.9.9");

    let boundary = "----rel";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"manifest\"\r\n\r\n{}\r\n--{boundary}--\r\n",
            serde_json::to_string(&release).unwrap()
        )
        .as_bytes(),
    );
    let result = ureq::post(&format!("{}/api/v1/admin/releases", server.base))
        .set("Authorization", &format!("Bearer {}", estimator.token().unwrap()))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body);
    assert!(
        matches!(result, Err(ureq::Error::Status(403, _))),
        "an estimator must not be able to push software to the whole office"
    );
}

#[test]
fn a_short_password_is_refused_when_a_seat_is_created() {
    let server = start();
    let admin = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let result = ureq::post(&format!("{}/api/v1/people", server.base))
        .set("Authorization", &format!("Bearer {}", admin.token().unwrap()))
        .send_json(serde_json::json!({
            "name": "New Hire",
            "email": "new@example.com",
            "password": "shop2026",
            "role": "estimator"
        }));
    match result {
        Err(ureq::Error::Status(400, response)) => {
            let problem: hub::Problem = response.into_json().unwrap();
            assert!(problem.message.contains("twelve"), "{}", problem.message);
        }
        other => panic!("a short password should be refused: {other:?}"),
    }
}

#[test]
fn two_seats_marking_up_the_same_drawing_each_end_up_with_both_lots() {
    // The thing six seats need to be able to do. One estimator marks up a
    // sheet, another marks up the same sheet, and each ends up holding the
    // other's work — without either of them losing their own.
    let server = start();
    let admin = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let project = admin.create_project("6742", "Elm West").expect("project");
    let set = upload(&server, admin.token().unwrap(), &project.id, &a_drawing());
    set_scale(&server, &set.id, 0, Some("1/4\" = 1'-0\""));

    let sam = admin;
    let other = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");

    // Both start level.
    let start_revision = sam.markups(&set.id, 0).expect("markups").revision;

    // Sam puts up two beams.
    let his = sam
        .push_markups(
            &set.id,
            &[
                hub::NewMarkup { page: 0, dictionary: a_beam(720.0), replaces: None },
                hub::NewMarkup { page: 0, dictionary: a_beam(360.0), replaces: None },
            ],
        )
        .expect("push");

    // The other seat puts up one, not having seen his.
    let theirs = other
        .push_markups(
            &set.id,
            &[hub::NewMarkup { page: 0, dictionary: a_beam(180.0), replaces: None }],
        )
        .expect("push");
    assert!(theirs.revision > his.revision);

    // Each asks for what has happened since they last looked.
    let to_sam = sam.markups(&set.id, his.revision).expect("sync");
    assert_eq!(to_sam.markups.len(), 1, "just the other seat's");
    assert_eq!(to_sam.markups[0].author, "Estimator");

    let to_other = other.markups(&set.id, start_revision).expect("sync");
    assert_eq!(to_other.markups.len(), 3, "all three, having seen none");

    // And the takeoff is the same number for both of them, because it is the
    // same markups measured by the same engine.
    let his_total = sam.takeoff(&set.id).expect("takeoff");
    let their_total = other.takeoff(&set.id).expect("takeoff");
    assert_eq!(his_total.pounds, their_total.pounds);
    assert_eq!(his_total.picks, 3);
    // 40 + 20 + 10 feet of W12x26 at 26 lb/ft.
    assert_eq!(his_total.pounds, Some(70.0 * 26.0));
    assert!(!his_total.is_short());
}

#[test]
fn a_markup_keeps_the_name_it_was_given_all_the_way_round() {
    use base64::Engine;
    // The name in the dictionary is what makes a markup the same markup on
    // another machine. If the server were to lose it or change it, a sync
    // would take somebody's work as new every time it came back.
    let server = start();
    let client = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let project = client.create_project("6742", "Elm West").expect("project");
    let set = upload(&server, client.token().unwrap(), &project.id, &a_drawing());

    let mut m = annot::Markup::new(annot::Subtype::Line);
    m.set("IT", pdf::Object::name("LineDimension"));
    m.set_subject("W12x26");
    m.set_name("xhv-0000000000000001-abcdef0123456789");
    m.set_line([0.0, 0.0], [720.0, 0.0]);
    let named = m.name();
    let mut bytes = Vec::new();
    pdf::write::write_object(&pdf::Object::Dict(m.dict), &mut bytes);
    let dictionary = base64::engine::general_purpose::STANDARD.encode(bytes);

    client
        .push_markups(&set.id, &[hub::NewMarkup { page: 0, dictionary, replaces: None }])
        .expect("push");

    let page = client.markups(&set.id, 0).expect("markups");
    let back = base64::engine::general_purpose::STANDARD
        .decode(page.markups[0].dictionary.as_bytes())
        .unwrap();
    let object = pdf::Reader::new(&back).object().unwrap();
    let markup = annot::Markup {
        dict: object.as_dict().unwrap().clone(),
        picture: None,
    };
    assert_eq!(markup.name(), named, "the name has to survive the round trip");
}

// ---- the first person in, and everybody after ------------------------------
//
// This is the part nobody should need help with. A shop downloads one program,
// somebody sets it up, and the other five make their own accounts. Every one of
// these is that path, over a socket, through the real client.

#[test]
fn a_fresh_server_says_plainly_that_nobody_has_claimed_it() {
    let server = start_empty();
    let health = Client::new(&server.base).health().expect("health");
    assert!(!health.claimed, "nobody has set this one up yet");

    // And a server that somebody is already using says the opposite, which is
    // what stops a seat offering to take over the company's server.
    let used = start();
    assert!(Client::new(&used.base).health().expect("health").claimed);
}

#[test]
fn the_first_person_in_sets_it_up_and_is_signed_in_when_they_are_done() {
    let server = start_empty();
    let mut client = Client::new(&server.base);
    let claimed = client
        .claim(&hub::Setup {
            company: "Example Steel".into(),
            name: "Sam".into(),
            email: "sam@example.com".into(),
            password: "brace-gusset-purlin-shim-42".into(),
        })
        .expect("claim it");

    assert_eq!(claimed.company, "Example Steel");
    assert_eq!(claimed.session.user.role, hub::Role::Admin);
    assert!(!claimed.join_code.trim().is_empty(), "something to hand round");

    // Signed in already: no second trip to a sign-in box.
    assert_eq!(client.me().expect("me").email, "sam@example.com");

    // And the server now calls itself what the office calls itself, which is
    // what every seat's title bar will read.
    let health = Client::new(&server.base).health().expect("health");
    assert_eq!(health.name, "Example Steel");
    assert!(health.claimed);
    assert_eq!(health.joining, "code");
}

#[test]
fn a_server_can_only_be_claimed_once() {
    let server = start_empty();
    let mut first = Client::new(&server.base);
    first
        .claim(&hub::Setup {
            company: "Example Steel".into(),
            name: "Sam".into(),
            email: "sam@example.com".into(),
            password: "brace-gusset-purlin-shim-42".into(),
        })
        .expect("the first one works");

    let mut second = Client::new(&server.base);
    let refused = second.claim(&hub::Setup {
        company: "Somebody Else".into(),
        name: "Passer By".into(),
        email: "hello@example.com".into(),
        password: "channel-stud-truss-web-77".into(),
    });
    assert!(refused.is_err(), "a claimed server is not up for grabs");

    // Refused and *unchanged*: the company name is still theirs.
    assert_eq!(
        Client::new(&server.base).health().expect("health").name,
        "Example Steel"
    );
}

#[test]
fn setting_up_refuses_a_password_short_enough_to_guess() {
    let server = start_empty();
    let mut client = Client::new(&server.base);
    assert!(client
        .claim(&hub::Setup {
            company: "Example Steel".into(),
            name: "Sam".into(),
            email: "sam@example.com".into(),
            password: "steel".into(),
        })
        .is_err());
    // And nothing was half-done: the server is still unclaimed.
    assert!(!Client::new(&server.base).health().expect("health").claimed);
}

#[test]
fn everybody_else_makes_their_own_account_with_the_code() {
    let server = start_empty();
    let mut boss = Client::new(&server.base);
    let claimed = boss
        .claim(&hub::Setup {
            company: "Example Steel".into(),
            name: "Sam".into(),
            email: "sam@example.com".into(),
            password: "brace-gusset-purlin-shim-42".into(),
        })
        .expect("claim");

    let mut estimator = Client::new(&server.base);
    let session = estimator
        .join(&hub::Join {
            code: claimed.join_code.clone(),
            name: "Lee".into(),
            email: "lee@example.com".into(),
            password: "camber-weld-joist-plate-19".into(),
        })
        .expect("join");

    // An ordinary seat, not another administrator.
    assert_eq!(session.user.role, hub::Role::Estimator);
    assert_eq!(estimator.me().expect("me").name, "Lee");

    // They can do the job straight away — this is the whole point.
    let project = estimator
        .create_project("24-118", "Warehouse")
        .expect("start a project");
    assert_eq!(project.number, "24-118");

    // And they cannot quietly administer the place.
    assert!(estimator.joining().is_err());
    assert!(boss.joining().is_ok());
}

#[test]
fn the_code_survives_being_read_aloud_across_an_office() {
    let server = start_empty();
    let mut boss = Client::new(&server.base);
    let claimed = boss
        .claim(&hub::Setup {
            company: "Example Steel".into(),
            name: "Sam".into(),
            email: "sam@example.com".into(),
            password: "brace-gusset-purlin-shim-42".into(),
        })
        .expect("claim");

    // Shouted across a shop and typed back in capitals with spaces in it.
    let shouted = claimed.join_code.to_uppercase().replace('-', " ");
    let mut them = Client::new(&server.base);
    assert!(them
        .join(&hub::Join {
            code: shouted,
            name: "Lee".into(),
            email: "lee@example.com".into(),
            password: "camber-weld-joist-plate-19".into(),
        })
        .is_ok());
}

#[test]
fn a_wrong_code_gets_nobody_in() {
    let server = start_empty();
    let mut boss = Client::new(&server.base);
    boss.claim(&hub::Setup {
        company: "Example Steel".into(),
        name: "Sam".into(),
        email: "sam@example.com".into(),
        password: "brace-gusset-purlin-shim-42".into(),
    })
    .expect("claim");

    let mut chancer = Client::new(&server.base);
    assert!(chancer
        .join(&hub::Join {
            code: "beam-beam-beam-beam-beam-0000".into(),
            name: "Nobody".into(),
            email: "nobody@example.com".into(),
            password: "channel-stud-truss-web-77".into(),
        })
        .is_err());

    // Nothing was created on the way to being refused.
    assert!(boss
        .people()
        .expect("people")
        .iter()
        .all(|p| p.email != "nobody@example.com"));
}

#[test]
fn the_same_email_twice_is_refused_rather_than_making_a_second_account() {
    let server = start_empty();
    let mut boss = Client::new(&server.base);
    let claimed = boss
        .claim(&hub::Setup {
            company: "Example Steel".into(),
            name: "Sam".into(),
            email: "sam@example.com".into(),
            password: "brace-gusset-purlin-shim-42".into(),
        })
        .expect("claim");

    let join = hub::Join {
        code: claimed.join_code.clone(),
        name: "Lee".into(),
        email: "lee@example.com".into(),
        password: "camber-weld-joist-plate-19".into(),
    };
    assert!(Client::new(&server.base).join(&join).is_ok());
    assert!(Client::new(&server.base).join(&join).is_err());
    assert_eq!(
        boss.people()
            .expect("people")
            .iter()
            .filter(|p| p.email == "lee@example.com")
            .count(),
        1
    );
}

#[test]
fn a_new_code_stops_the_old_one_working() {
    // What somebody does the afternoon a person leaves.
    let server = start_empty();
    let mut boss = Client::new(&server.base);
    let claimed = boss
        .claim(&hub::Setup {
            company: "Example Steel".into(),
            name: "Sam".into(),
            email: "sam@example.com".into(),
            password: "brace-gusset-purlin-shim-42".into(),
        })
        .expect("claim");

    let fresh = boss
        .change_joining(None, None, true)
        .expect("issue a new code");
    assert_ne!(fresh.code, claimed.join_code);

    assert!(Client::new(&server.base)
        .join(&hub::Join {
            code: claimed.join_code,
            name: "Gone".into(),
            email: "gone@example.com".into(),
            password: "camber-weld-joist-plate-19".into(),
        })
        .is_err());
    assert!(Client::new(&server.base)
        .join(&hub::Join {
            code: fresh.code,
            name: "Lee".into(),
            email: "lee@example.com".into(),
            password: "camber-weld-joist-plate-19".into(),
        })
        .is_ok());
}

#[test]
fn a_shop_can_shut_joining_off_altogether() {
    let server = start_empty();
    let mut boss = Client::new(&server.base);
    let claimed = boss
        .claim(&hub::Setup {
            company: "Example Steel".into(),
            name: "Sam".into(),
            email: "sam@example.com".into(),
            password: "brace-gusset-purlin-shim-42".into(),
        })
        .expect("claim");

    boss.change_joining(Some("closed"), None, false).expect("shut it");
    assert_eq!(
        Client::new(&server.base).health().expect("health").joining,
        "closed"
    );
    assert!(Client::new(&server.base)
        .join(&hub::Join {
            code: claimed.join_code,
            name: "Lee".into(),
            email: "lee@example.com".into(),
            password: "camber-weld-joist-plate-19".into(),
        })
        .is_err());
}

#[test]
fn nobody_can_arrange_for_new_accounts_to_be_administrators() {
    let server = start_empty();
    let mut boss = Client::new(&server.base);
    boss.claim(&hub::Setup {
        company: "Example Steel".into(),
        name: "Sam".into(),
        email: "sam@example.com".into(),
        password: "brace-gusset-purlin-shim-42".into(),
    })
    .expect("claim");

    // Because a join code read out across a shop floor is not the thing that
    // should decide who administers the company's drawings.
    assert!(boss.change_joining(None, Some("admin"), false).is_err());
    assert_eq!(boss.joining().expect("joining").role, "estimator");
}

#[test]
fn a_claimed_server_cannot_be_set_up_by_somebody_who_wandered_onto_the_network() {
    // The ordinary case for the whole of a server's life: it is somebody's, and
    // the setup route is closed for good.
    let server = start();
    let mut passer_by = Client::new(&server.base);
    assert!(passer_by
        .claim(&hub::Setup {
            company: "Not Theirs".into(),
            name: "Passer By".into(),
            email: "hello@example.com".into(),
            password: "channel-stud-truss-web-77".into(),
        })
        .is_err());
    // And joining is off until an administrator turns it on, rather than open
    // because nobody thought about it.
    assert!(passer_by
        .join(&hub::Join {
            code: String::new(),
            name: "Passer By".into(),
            email: "hello@example.com".into(),
            password: "channel-stud-truss-web-77".into(),
        })
        .is_err());
}

// ---- the Fleet channel, end to end -----------------------------------------
//
// The thing this proves is the arrangement itself: the Fleet hands a shop's
// server a package it never looked inside, and the server decides for itself
// whether to install it — by checking the same signature a seat will check.
// A Fleet with a compromised shelf can offer only builds that were already
// signed by the key the publisher holds, and this is what says so.

/// A server with a maintenance key, which is what turns the Fleet channel on.
fn start_maintained(key: &str) -> Running {
    let scratch = Scratch::new();
    let config = Config {
        name: "Example Steel".into(),
        data: scratch.0.clone(),
        listen: "127.0.0.1:0".parse().unwrap(),
        maintenance_key: Some(key.to_string()),
        ..Config::default()
    };
    let store = Store::open(&config.database(), &config.blobs()).expect("a store");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind(config.listen))
        .expect("a socket");
    let address = listener.local_addr().expect("an address");
    let app = api::router(Arc::new(Server::new(store, config)));
    runtime.spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Running {
        base: format!("http://{address}"),
        _scratch: scratch,
        _runtime: runtime,
    }
}

fn get_with_key(server: &Running, path: &str, key: Option<&str>) -> (u16, String) {
    let mut request = ureq::get(&format!("{}{path}", server.base));
    if let Some(key) = key {
        request = request.set(hyperview_server::fleet::KEY_HEADER, key);
    }
    match request.call() {
        Ok(response) => {
            let code = response.status();
            (code, response.into_string().unwrap_or_default())
        }
        Err(ureq::Error::Status(code, response)) => {
            (code, response.into_string().unwrap_or_default())
        }
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn a_server_nobody_looks_after_has_no_maintenance_channel_at_all() {
    // Not "refuses" — absent. A shop that has not asked to be looked after
    // does not quietly grow a remote maintenance channel, and a scan of the
    // box should not find one to knock on.
    let server = start();
    let (code, _) = get_with_key(&server, "/api/admin/ops", Some("anything"));
    assert_eq!(code, 404, "there should be nothing there");
    let (code, _) = get_with_key(&server, "/api/admin/logtail", Some("anything"));
    assert_eq!(code, 404);
}

#[test]
fn the_version_route_is_public_and_says_what_is_running() {
    // The Fleet reads this back to decide whether a rollout worked, so it has
    // to say what is running rather than what was hoped for.
    let server = start_maintained("a-long-maintenance-key-nobody-guesses");
    let (code, body) = get_with_key(&server, "/api/version", None);
    assert_eq!(code, 200);
    assert!(body.contains("\"product\":\"hyperview\""), "{body}");
    assert!(body.contains(hyperview_server::VERSION), "{body}");
}

#[test]
fn the_ops_route_needs_the_key_and_tells_the_truth_about_backups() {
    let server = start_maintained("a-long-maintenance-key-nobody-guesses");
    let (code, _) = get_with_key(&server, "/api/admin/ops", None);
    assert_eq!(code, 401, "no key, no answer");
    let (code, _) = get_with_key(&server, "/api/admin/ops", Some("wrong"));
    assert_eq!(code, 401);

    let (code, body) =
        get_with_key(&server, "/api/admin/ops", Some("a-long-maintenance-key-nobody-guesses"));
    assert_eq!(code, 200, "{body}");
    // Never backed up is null, and null is not zero. A shop one dead disk away
    // from losing every takeoff it has done should show amber, not green.
    assert!(body.contains("\"backupDays\":null"), "{body}");
    assert!(body.contains("\"signingRequired\":true"), "{body}");
}

#[test]
fn a_release_signed_by_nobody_this_build_trusts_is_refused() {
    // The heart of it. The Fleet's key opens the door; it does not vouch for
    // what comes through it.
    let key = "a-long-maintenance-key-nobody-guesses";
    let server = start_maintained(key);

    let installer = b"this is not really an installer".to_vec();
    let release = Release {
        version: "9.9.9".into(),
        channel: Channel::Stable,
        platform: "windows-x64".into(),
        published: "2026-09-18T00:00:00Z".into(),
        notes: "Made up".into(),
        download: "/api/v1/update/9.9.9/download?platform=windows-x64".into(),
        bytes: installer.len() as u64,
        digest: "00".repeat(32),
        signature: "11".repeat(64),
        key: "nobody".into(),
        minimum_api_version: 1,
    };

    use base64::Engine;
    let body = serde_json::json!({
        "release": release,
        "installer": base64::engine::general_purpose::STANDARD.encode(&installer),
        "ready": true,
    });
    // The Fleet's contract: the answer says what was written and what was
    // skipped, and a non-empty skipped list is a failure rather than a
    // rollout. So the refusal has to show up there — written empty, skipped
    // saying why — rather than as an HTTP error the Fleet would not read.
    let said = ureq::post(&format!("{}/api/admin/update", server.base))
        .set(hyperview_server::fleet::KEY_HEADER, key)
        .send_json(body)
        .expect("the channel answers")
        .into_string()
        .expect("a body");
    let answer: serde_json::Value = serde_json::from_str(&said).expect("json");
    assert_eq!(
        answer["written"].as_array().map(|a| a.len()),
        Some(0),
        "nothing should have been written: {said}"
    );
    let skipped = answer["skipped"].as_array().expect("a skipped list");
    assert_eq!(skipped.len(), 1, "{said}");
    let why = skipped[0].as_str().unwrap_or_default().to_lowercase();
    assert!(
        why.contains("signature") || why.contains("key this program does not know"),
        "and it should say why: {said}"
    );
    assert_eq!(answer["restarting"], serde_json::Value::Bool(false));

    // And nothing is on offer to the seats afterwards.
    let (code, offered) = get_with_key(&server, "/api/v1/update/latest?platform=windows-x64", None);
    assert!(
        code == 404 || !offered.contains("9.9.9"),
        "an unsigned release must not end up on offer: {code} {offered}"
    );
}

#[test]
fn a_release_offered_without_the_maintenance_key_gets_nowhere() {
    let server = start_maintained("a-long-maintenance-key-nobody-guesses");
    let outcome = ureq::post(&format!("{}/api/admin/update", server.base))
        .send_json(serde_json::json!({"release": {}, "installer": "", "ready": true}));
    match outcome {
        Err(ureq::Error::Status(code, _)) => {
            assert!(code == 401 || code == 422, "{code}");
        }
        Ok(_) => panic!("it should not have been accepted"),
        Err(e) => panic!("{e}"),
    }
}

// ---- how big a push may be ---------------------------------------------------
//
// A real push is one platform's program in base64, tens of megabytes. axum's
// default 2 MB refused every one, and the caller saw only "broken pipe". These
// prove a real-sized push gets in, a too-big one is told so in words, and a
// caller without the key is turned away before the server reads its body.

/// A made-up release for an installer, signed by nobody this build trusts.
fn unsigned_release(installer: &[u8]) -> Release {
    Release {
        version: "9.9.9".into(), // WHAT: a version no real build has. WHY: nothing real is mistaken for it.
        channel: Channel::Stable,
        platform: "linux-x64".into(), // WHAT: the Linux build. WHY: it is the biggest push there is today.
        published: "2026-10-25T00:00:00Z".into(),
        notes: "Made up".into(),
        download: "/api/v1/update/9.9.9/download?platform=linux-x64".into(),
        bytes: installer.len() as u64, // WHAT: its real size. WHY: the manifest describes the file it came with.
        digest: "00".repeat(32),
        signature: "11".repeat(64),
        key: "nobody".into(), // WHAT: no key this build pins. WHY: it must be refused at the signature, after the size check.
        minimum_api_version: 1,
    }
}

/// A connection to a running server, with a generous read timeout.
fn a_socket(server: &Running) -> std::net::TcpStream {
    let address = server.base.trim_start_matches("http://"); // WHAT: host and port. WHY: these tests speak HTTP by hand.
    let stream = std::net::TcpStream::connect(address).expect("a connection"); // WHAT: a plain socket. WHY: so what the server does with the connection shows, not what a client library hides.
    stream.set_read_timeout(Some(std::time::Duration::from_secs(60))).unwrap(); // WHAT: a minute to answer. WHY: a server that waits for a body it should not read never answers, and the test must fail rather than hang.
    stream
}

/// Reads the server's answer: its status line and whatever body follows.
/// Panics if the connection was broken instead of answered.
fn the_answer(stream: &mut std::net::TcpStream) -> String {
    use std::io::Read;
    let mut got = Vec::new(); // WHAT: everything read so far. WHY: the answer may come in pieces.
    let mut buffer = [0u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break, // WHAT: the server closed after answering. WHY: the answer is all there.
            Ok(n) => got.extend_from_slice(&buffer[..n]), // WHAT: more of the answer. WHY: kept until it is whole.
            Err(e) => panic!("the connection broke instead of being answered: {e} (so far: {:?})", String::from_utf8_lossy(&got)), // WHAT: a reset or a timeout fails the test. WHY: that is the failure the caller saw before.
        }
        let text = String::from_utf8_lossy(&got).to_string(); // WHAT: the answer as text. WHY: to see whether it is whole yet.
        if let Some(end) = text.find("\r\n\r\n") {
            if text[end..].contains('}') {
                return text; // WHAT: the headers and the JSON body. WHY: that is all a test reads.
            }
        }
    }
    String::from_utf8_lossy(&got).to_string()
}

#[test]
fn a_push_the_size_of_a_real_build_gets_past_the_size_check() {
    let key = "a-long-maintenance-key-nobody-guesses";
    let server = start_maintained(key);
    let installer = vec![0x5a_u8; 60 * 1024 * 1024]; // WHAT: 60 MiB, the size of the Linux program. WHY: the biggest push there is today.
    use base64::Engine;
    let body = serde_json::to_vec(&serde_json::json!({
        "release": unsigned_release(&installer),
        "installer": base64::engine::general_purpose::STANDARD.encode(&installer),
        "ready": false,
    }))
    .unwrap(); // WHAT: the push, as the Fleet sends it. WHY: the size that matters is the base64 one.
    assert!(body.len() > 80 * 1024 * 1024, "{}", body.len()); // WHAT: over 80 MiB on the wire. WHY: the test is only worth anything at a real size.
    let said = ureq::post(&format!("{}/api/admin/update", server.base))
        .set(hyperview_server::fleet::KEY_HEADER, key)
        .set("Content-Type", "application/json")
        .send_bytes(&body)
        .expect("a push this size is answered, not refused") // WHAT: a 200. WHY: a 413 here is the bug this fixes.
        .into_string()
        .expect("a body");
    let answer: serde_json::Value = serde_json::from_str(&said).expect("json");
    let why = answer["skipped"][0].as_str().unwrap_or_default().to_lowercase(); // WHAT: why it was not taken. WHY: it must be the signature, which is checked only after the whole push is in.
    assert!(why.contains("signature"), "it should have got as far as the signature: {said}");
}

#[test]
fn a_push_bigger_than_the_server_takes_is_told_so_and_not_cut_off() {
    use std::io::Write;
    let key = "a-long-maintenance-key-nobody-guesses";
    let server = start_maintained(key);
    let size = hyperview_server::fleet::LARGEST_PUSH + 1; // WHAT: one byte over. WHY: the limit itself is what is tested.
    let mut stream = a_socket(&server);
    write!(
        stream,
        "POST /api/admin/update HTTP/1.1\r\nHost: test\r\n{}: {key}\r\nContent-Type: application/json\r\nContent-Length: {size}\r\n\r\n",
        hyperview_server::fleet::KEY_HEADER
    )
    .unwrap(); // WHAT: the headers, with the right key and the size. WHY: the way the Fleet sends a push, with no Expect.
    let chunk = vec![b'a'; 1024 * 1024]; // WHAT: a megabyte at a time. WHY: sent the way a real upload goes, without holding it all.
    let mut sent = 0usize;
    while sent < size {
        let n = chunk.len().min(size - sent); // WHAT: the last piece is whatever is left. WHY: exactly the size it said.
        stream.write_all(&chunk[..n]).unwrap_or_else(|e| panic!("the server cut the push off after {sent} bytes: {e}")); // WHAT: a broken pipe fails the test. WHY: that was all the Fleet ever saw.
        sent += n;
    }
    let answer = the_answer(&mut stream);
    assert!(answer.starts_with("HTTP/1.1 413"), "{answer}"); // WHAT: 413. WHY: too big is a clear refusal.
    assert!(answer.contains("192 MiB"), "and it should say what the limit is: {answer}");
}

#[test]
fn a_push_of_no_stated_size_is_told_so_once_it_passes_the_limit() {
    use std::io::Write;
    let key = "a-long-maintenance-key-nobody-guesses";
    let server = start_maintained(key);
    let mut stream = a_socket(&server);
    write!(
        stream,
        "POST /api/admin/update HTTP/1.1\r\nHost: test\r\n{}: {key}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n",
        hyperview_server::fleet::KEY_HEADER
    )
    .unwrap(); // WHAT: a push that says no size. WHY: the limit must hold when it is found out by counting.
    let chunk = vec![b'a'; 1024 * 1024];
    let mut sent = 0usize;
    while sent <= hyperview_server::fleet::LARGEST_PUSH + 32 * 1024 * 1024 { // WHAT: 32 MiB past the limit. WHY: more than the socket's buffers hold, so a server that stopped reading at the limit would cut it off.
        write!(stream, "{:x}\r\n", chunk.len()).unwrap(); // WHAT: the chunk's size line. WHY: how a body of no stated size is sent.
        stream.write_all(&chunk).unwrap_or_else(|e| panic!("the server cut the push off after {sent} bytes: {e}"));
        stream.write_all(b"\r\n").unwrap();
        sent += chunk.len();
    }
    stream.write_all(b"0\r\n\r\n").unwrap(); // WHAT: the end of the body. WHY: well past the limit, and done.
    let answer = the_answer(&mut stream);
    assert!(answer.starts_with("HTTP/1.1 413"), "{answer}");
}

#[test]
fn a_push_that_waits_to_be_told_to_go_on_is_refused_before_it_is_sent() {
    use std::io::Write;
    let key = "a-long-maintenance-key-nobody-guesses";
    let server = start_maintained(key);
    let mut stream = a_socket(&server);
    write!(
        stream,
        "POST /api/admin/update HTTP/1.1\r\nHost: test\r\n{}: {key}\r\nContent-Type: application/json\r\nExpect: 100-continue\r\nContent-Length: {}\r\n\r\n",
        hyperview_server::fleet::KEY_HEADER,
        10u64 << 30
    )
    .unwrap(); // WHAT: 10 GiB announced, nothing sent. WHY: a caller that waits must hear no at once, not be asked to send it.
    let answer = the_answer(&mut stream);
    assert!(answer.starts_with("HTTP/1.1 413"), "{answer}");
}

#[test]
fn a_push_far_too_big_is_read_past_only_as_far_as_the_cap() {
    use std::io::Write;
    let key = "a-long-maintenance-key-nobody-guesses";
    let server = start_maintained(key);
    let cap = hyperview_server::fleet::READ_PAST_AT_MOST; // WHAT: the most the server throws away. WHY: past that, a caller with the key still cannot keep it reading.
    let declared = cap * 3; // WHAT: three times the cap. WHY: a server that read whatever it was told would take all of it.
    let mut stream = a_socket(&server);
    write!(
        stream,
        "POST /api/admin/update HTTP/1.1\r\nHost: test\r\n{}: {key}\r\nContent-Type: application/json\r\nContent-Length: {declared}\r\n\r\n",
        hyperview_server::fleet::KEY_HEADER
    )
    .unwrap(); // WHAT: the right key, and a size far over the cap. WHY: the cap is about callers that have the key.
    let chunk = vec![b'a'; 1024 * 1024];
    let mut sent = 0u64; // WHAT: how much the server took before it stopped reading. WHY: that is what the cap limits.
    while sent < declared {
        if stream.write_all(&chunk).is_err() {
            break; // WHAT: the server stopped reading and closed. WHY: expected, once it has read as far as the cap.
        }
        sent += chunk.len() as u64;
    }
    let slack = 64 * 1024 * 1024; // WHAT: room for what the sockets' buffers hold. WHY: those bytes are written but never read by the server.
    assert!(sent >= cap, "it should have read as far as the cap, not stopped at {sent}"); // WHAT: it did read past, up to the cap. WHY: that is what gives an honest caller a readable answer.
    assert!(sent <= cap + slack, "it read {sent} bytes, past the cap of {cap}"); // WHAT: and no further. WHY: a caller must not be able to make it read without end.
}

#[test]
fn a_big_push_without_the_key_is_turned_away_before_it_is_read() {
    use std::io::Write;
    let server = start_maintained("a-long-maintenance-key-nobody-guesses");
    for key in [None, Some("a-wrong-maintenance-key-of-some-length")] {
        let mut stream = a_socket(&server);
        let line = key.map(|k| format!("{}: {k}\r\n", hyperview_server::fleet::KEY_HEADER)).unwrap_or_default(); // WHAT: no key, or a wrong one. WHY: both must be turned away the same.
        write!(
            stream,
            "POST /api/admin/update HTTP/1.1\r\nHost: test\r\n{line}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            150 * 1024 * 1024
        )
        .unwrap(); // WHAT: says 150 MiB, under the limit, and sends none of it. WHY: a server that read the body before the key would sit waiting for it, and never answer.
        let answer = the_answer(&mut stream);
        assert!(answer.starts_with("HTTP/1.1 401"), "{key:?}: {answer}"); // WHAT: refused at once. WHY: the key is checked before a byte of the body is read.
    }
}

#[test]
fn the_fleet_can_ask_who_is_connected_and_the_answer_names_the_seat() {
    // A maintained server with one seat on it, so the question has an answer.
    let key = "a-long-maintenance-key-nobody-guesses";
    let scratch = Scratch::new();
    let config = Config {
        name: "Example Steel Shop".into(),
        data: scratch.0.clone(),
        listen: "127.0.0.1:0".parse().unwrap(),
        maintenance_key: Some(key.to_string()),
        ..Config::default()
    };
    let store = Store::open(&config.database(), &config.blobs()).expect("a store");
    let hashed = auth::hash_password("brace-gusset-purlin-shim-42").unwrap();
    let now = api::now();
    store
        .with(|db| {
            db.execute(
                "INSERT INTO people (id, name, email, password, role, created)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![api::fresh_id("usr"), "Robin", "robin@example.com", hashed, "estimator", now],
            )?;
            Ok(())
        })
        .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind(config.listen))
        .expect("a socket");
    let address = listener.local_addr().expect("an address");
    let app = api::router(Arc::new(Server::new(store, config)));
    runtime.spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let server = Running {
        base: format!("http://{address}"),
        _scratch: scratch,
        _runtime: runtime,
    };

    // The roster is gated like everything else on the maintenance surface.
    let (code, _) = get_with_key(&server, "/api/admin/presence", None);
    assert_eq!(code, 401, "no key, no roster — it names people and addresses");

    // Before anybody signs in it answers, and says when the ledger was born,
    // so empty reads as "fresh ledger", never "nobody ever connects".
    let (code, body) = get_with_key(&server, "/api/admin/presence", Some(key));
    assert_eq!(code, 200, "{body}");
    assert!(body.contains("\"sinceBoot\""), "{body}");

    // A seat signs in and works; the ledger notices without being asked.
    let signed: serde_json::Value = ureq::post(&format!("{}/api/v1/auth/token", server.base))
        .send_json(serde_json::json!({"email": "robin@example.com", "password": "brace-gusset-purlin-shim-42"}))
        .expect("sign in")
        .into_json()
        .expect("a session");
    let token = signed["token"].as_str().expect("a token").to_string();
    let me = ureq::get(&format!("{}/api/v1/me", server.base))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .expect("me");
    assert_eq!(me.status(), 200);

    let (code, body) = get_with_key(&server, "/api/admin/presence", Some(key));
    assert_eq!(code, 200, "{body}");
    assert!(body.contains("\"who\":\"Robin\""), "the seat is on the roster: {body}");
    assert!(body.contains("\"company\":\"Example Steel Shop\""), "seats group under the shop: {body}");
    assert!(body.contains("\"kind\":\"office\""), "{body}");

    // And a token nobody recognises records nothing.
    let _ = ureq::get(&format!("{}/api/v1/me", server.base))
        .set("Authorization", "Bearer not-a-real-token-at-all")
        .call();
    let (_, body) = get_with_key(&server, "/api/admin/presence", Some(key));
    assert!(
        !body.contains("not-a-real"),
        "the ledger lists people this server let in, not strings that knocked"
    );
}

#[test]
fn a_takeoff_copy_leaves_the_issued_set_alone() {
    let server = start();
    let admin = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let project = admin.create_project("6742", "Elm West").expect("project");
    let issued = upload(&server, admin.token().unwrap(), &project.id, &a_drawing());
    set_scale(&server, &issued.id, 0, Some("1/4\" = 1'-0\""));

    let est = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");
    let copy = est.copy_set(&issued.id, "").expect("a copy to take off on");

    assert_ne!(copy.id, issued.id, "it is its own set");
    assert_eq!(copy.name, format!("{} — Takeoff", issued.name));
    assert_eq!(copy.digest, issued.digest, "the same file, not a second upload");
    assert_eq!(copy.sheets, issued.sheets);
    assert_eq!(copy.uploaded_by, "Estimator", "whoever made it");
    assert!(copy.supersedes.is_none(), "a working copy is not another issue");
    assert!(copy.superseded_by.is_none());

    // The scales somebody already calibrated came across. Taking off on a
    // copy that had forgotten them would be worse than no copy at all.
    let sheets = est.sheets(&copy.id).expect("sheets");
    assert_eq!(sheets[0].scale.as_deref(), Some("1/4\" = 1'-0\""));

    // Marking up the copy does nothing to the issued set.
    est.push_markups(
        &copy.id,
        &[
            hub::NewMarkup { page: 0, dictionary: a_beam(720.0), replaces: None },
            hub::NewMarkup { page: 0, dictionary: a_beam(720.0), replaces: None },
        ],
    )
    .expect("push");

    let on_the_copy = est.markups(&copy.id, 0).expect("the copy's markups");
    assert_eq!(on_the_copy.markups.len(), 2);
    let on_the_issue = est.markups(&issued.id, 0).expect("the issued set's markups");
    assert!(on_the_issue.markups.is_empty(), "the issued set is still clean");

    // And the issued set is still the one in the project: a copy must never
    // take its place.
    let listed = admin.sets(&project.id).expect("sets");
    let still_there = listed.iter().find(|s| s.id == issued.id).expect("the issued set");
    assert!(still_there.superseded_by.is_none(), "a copy did not supersede it");
}

#[test]
fn a_copy_can_be_named_and_a_viewer_cannot_make_one() {
    let server = start();
    let admin = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let project = admin.create_project("6742", "Elm West").expect("project");
    let issued = upload(&server, admin.token().unwrap(), &project.id, &a_drawing());

    let est = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");
    let named = est.copy_set(&issued.id, "Bid 6742 — steel").expect("a named copy");
    assert_eq!(named.name, "Bid 6742 — steel");

    let looker = signed_in(&server, "look@example.com", "channel-stud-truss-web-77");
    let refused = looker.copy_set(&issued.id, "").expect_err("a viewer may not");
    assert!(refused.to_string().contains("not allowed"), "{refused}");
}

// ---- people who leave ---------------------------------------------------------
//
// A shop that cannot take somebody's access away does not have access control,
// it has a suggestion. These check the two things that matter: that it works
// at once, and that it cannot be used to lock the shop out of its own server.

fn call(server: &Running, method: &str, path: &str, token: &str, body: serde_json::Value)
    -> Result<serde_json::Value, u16>
{
    let url = format!("{}/api/v1{path}", server.base);
    let request = match method {
        "POST" => ureq::post(&url),
        _ => ureq::get(&url),
    }
    .set("Authorization", &format!("Bearer {token}"));
    let outcome = if method == "POST" { request.send_json(body) } else { request.call() };
    match outcome {
        Ok(response) => Ok(response.into_json().unwrap_or(serde_json::Value::Null)),
        Err(ureq::Error::Status(code, _)) => Err(code),
        Err(e) => panic!("{e}"),
    }
}

fn person_called(server: &Running, token: &str, email: &str) -> Option<serde_json::Value> {
    let people = call(server, "GET", "/people", token, serde_json::Value::Null).expect("the list");
    people
        .as_array()?
        .iter()
        .find(|p| p["email"] == email)
        .cloned()
}

#[test]
fn removing_somebody_ends_their_session_there_and_then() {
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let leaver = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");
    let boss_token = boss.token().expect("a token").to_string();
    let leaver_token = leaver.token().expect("a token").to_string();

    // They can read the shop's work, as they could all week.
    assert!(call(&server, "GET", "/projects", &leaver_token, serde_json::Value::Null).is_ok());

    let id = person_called(&server, &boss_token, "est@example.com").expect("them")["id"]
        .as_str()
        .unwrap()
        .to_string();
    call(&server, "POST", &format!("/people/{id}/remove"), &boss_token, serde_json::json!({}))
        .expect("removed");

    // Not at the next sign-in. Now.
    assert_eq!(
        call(&server, "GET", "/projects", &leaver_token, serde_json::Value::Null),
        Err(401),
        "a removed person's token still worked"
    );
    assert!(person_called(&server, &boss_token, "est@example.com").is_none());
}

#[test]
fn changing_what_somebody_may_do_takes_effect_on_their_next_request() {
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let hand = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");
    let boss_token = boss.token().expect("a token").to_string();
    let hand_token = hand.token().expect("a token").to_string();

    // An estimator may start a job.
    let made = call(&server, "POST", "/projects", &hand_token,
                    serde_json::json!({"number": "2575", "name": "Water Tower"}));
    assert!(made.is_ok(), "an estimator should be able to make a project: {made:?}");

    let id = person_called(&server, &boss_token, "est@example.com").expect("them")["id"]
        .as_str()
        .unwrap()
        .to_string();
    call(&server, "POST", &format!("/people/{id}/role"), &boss_token,
         serde_json::json!({"role": "viewer"})).expect("changed");

    // Same token, same second, and they may no longer write.
    assert_eq!(
        call(&server, "POST", "/projects", &hand_token,
             serde_json::json!({"number": "2560", "name": "Nope"})),
        Err(403),
        "they were demoted and could still write"
    );
    // And can still read, because that is what a viewer is.
    assert!(call(&server, "GET", "/projects", &hand_token, serde_json::Value::Null).is_ok());
}

#[test]
fn the_last_administrator_cannot_be_removed_or_demoted() {
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let token = boss.token().expect("a token").to_string();
    let me = person_called(&server, &token, "sam@example.com").expect("me")["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Not by removing yourself, which is the way somebody actually does it.
    assert_eq!(
        call(&server, "POST", &format!("/people/{me}/remove"), &token, serde_json::json!({})),
        Err(400)
    );

    // Nor by a second administrator taking the badge off the only other one,
    // once they have made themselves one and then changed their mind.
    let other = person_called(&server, &token, "est@example.com").expect("them")["id"]
        .as_str()
        .unwrap()
        .to_string();
    call(&server, "POST", &format!("/people/{other}/role"), &token,
         serde_json::json!({"role": "admin"})).expect("promoted");
    let second = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");
    let second_token = second.token().expect("a token").to_string();
    call(&server, "POST", &format!("/people/{me}/remove"), &second_token, serde_json::json!({}))
        .expect("the first one can go now there are two");
    assert_eq!(
        call(&server, "POST", &format!("/people/{other}/role"), &second_token,
             serde_json::json!({"role": "estimator"})),
        Err(400),
        "the last administrator demoted themselves and locked the shop out"
    );

    // And they are still an administrator afterwards, not half-changed.
    assert!(call(&server, "GET", "/people", &second_token, serde_json::Value::Null).is_ok());
}

#[test]
fn signing_out_actually_ends_the_session() {
    let server = start();
    let client = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");
    let token = client.token().expect("a token").to_string();
    assert!(call(&server, "GET", "/projects", &token, serde_json::Value::Null).is_ok());

    call(&server, "POST", "/signout", &token, serde_json::json!({})).expect("signed out");

    assert_eq!(
        call(&server, "GET", "/projects", &token, serde_json::Value::Null),
        Err(401),
        "the token still worked after signing out"
    );
}

#[test]
fn an_ordinary_seat_cannot_remove_anybody() {
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let hand = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");
    let boss_token = boss.token().expect("a token").to_string();
    let hand_token = hand.token().expect("a token").to_string();
    let looker = person_called(&server, &boss_token, "look@example.com").expect("them")["id"]
        .as_str()
        .unwrap()
        .to_string();

    assert_eq!(
        call(&server, "POST", &format!("/people/{looker}/remove"), &hand_token, serde_json::json!({})),
        Err(403)
    );
    assert_eq!(
        call(&server, "POST", &format!("/people/{looker}/role"), &hand_token,
             serde_json::json!({"role": "admin"})),
        Err(403)
    );
    // And the list itself was never theirs to read.
    assert_eq!(
        call(&server, "GET", "/people", &hand_token, serde_json::Value::Null),
        Err(403)
    );
}

#[test]
fn what_a_person_made_outlives_them() {
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let hand = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");
    let boss_token = boss.token().expect("a token").to_string();
    let hand_token = hand.token().expect("a token").to_string();

    let project = call(&server, "POST", "/projects", &hand_token,
                       serde_json::json!({"number": "2575", "name": "Water Tower"}))
        .expect("a project");
    let set = upload(&server, &hand_token, project["id"].as_str().unwrap(), &a_drawing());

    let id = person_called(&server, &boss_token, "est@example.com").expect("them")["id"]
        .as_str()
        .unwrap()
        .to_string();
    call(&server, "POST", &format!("/people/{id}/remove"), &boss_token, serde_json::json!({}))
        .expect("removed");

    // The drawing set is still there and still says who put it up. A takeoff
    // does not lose its history because somebody left the company.
    let still = call(&server, "GET", &format!("/sets/{}", set.id), &boss_token, serde_json::Value::Null)
        .expect("the set is still there");
    assert_eq!(still["uploaded_by"], "Estimator", "{still}");
}

#[test]
fn what_happens_to_an_account_is_in_the_log() {
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let token = boss.token().expect("a token").to_string();

    let looker = person_called(&server, &token, "look@example.com").expect("them")["id"]
        .as_str()
        .unwrap()
        .to_string();
    call(&server, "POST", &format!("/people/{looker}/role"), &token,
         serde_json::json!({"role": "estimator"})).expect("changed");
    call(&server, "POST", &format!("/people/{looker}/remove"), &token, serde_json::json!({}))
        .expect("removed");

    let log = call(&server, "GET", "/audit", &token, serde_json::Value::Null).expect("the log");
    let text = log.to_string();
    for expected in ["changed what a person may do", "removed a person", "signed in"] {
        assert!(text.contains(expected), "the log does not mention {expected}: {text}");
    }
}

#[test]
fn a_refused_sign_in_is_recorded_even_when_the_address_is_unknown() {
    let server = start();
    // Somebody guessing addresses, which is the case an administrator most
    // wants to see and the one that used to go unrecorded entirely.
    let mut stranger = Client::new(&server.base);
    assert!(stranger.sign_in("nobody@example.com", "not-even-close-to-right").is_err());

    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let token = boss.token().expect("a token").to_string();
    let log = call(&server, "GET", "/audit", &token, serde_json::Value::Null).expect("the log");
    let text = log.to_string();
    assert!(text.contains("no account with that address"), "{text}");
    assert!(text.contains("nobody@example.com"), "{text}");
}

// ---- who can see which job ------------------------------------------------

/// A shop is one room until somebody says otherwise.
///
/// That is how every server has worked and how most shops want it, so the
/// tests that matter most here are the ones about *not* changing anything
/// for a shop that never uses this.
#[test]
fn a_project_with_nobody_named_on_it_is_everybodys() {
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let hand = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");

    let project = boss.create_project("6742", "Elm West").expect("project");
    assert!(boss.who_is_on(&project.id).expect("who").is_empty());

    let seen = hand.projects().expect("list");
    assert!(
        seen.iter().any(|p| p.id == project.id),
        "a project nobody was named on should be everybody's"
    );
    assert!(hand.project(&project.id).is_ok(), "and openable");
}

#[test]
fn naming_one_person_takes_it_away_from_everybody_else() {
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let hand = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");

    let project = boss.create_project("6742", "Elm West").expect("project");
    let boss_id = boss.me().expect("me").id;
    boss.change_who_is_on(&project.id, &boss_id, true).expect("put on");

    let seen = hand.projects().expect("list");
    assert!(
        !seen.iter().any(|p| p.id == project.id),
        "somebody not on the job should not see it listed"
    );
}

#[test]
fn a_job_somebody_cannot_see_cannot_be_opened_by_guessing_its_id() {
    // The failure that matters. Filtering a list and leaving the direct
    // fetch open is not access control, it is a tidier list -- and the id is
    // in every link anybody was ever sent.
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let hand = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");

    let project = boss.create_project("6742", "Elm West").expect("project");
    let boss_id = boss.me().expect("me").id;
    boss.change_who_is_on(&project.id, &boss_id, true).expect("put on");

    assert!(hand.project(&project.id).is_err(), "opened by id anyway");
    assert!(hand.sets(&project.id).is_err(), "its sets were listed anyway");
    assert!(hand.who_is_on(&project.id).is_err(), "it said who was on it");
}

#[test]
fn the_drawings_on_a_job_go_with_it() {
    // A set is reached by its own id, not the project's, so the project
    // filter alone would leave every drawing openable by anybody.
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let hand = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");

    let project = boss.create_project("6742", "Elm West").expect("project");
    let set = boss
        .upload_set(&project.id, "Issue for bid", "S-101.pdf", &a_drawing())
        .expect("upload");

    // Everybody's, so far.
    assert!(hand.set(&set.id).is_ok());
    assert!(hand.sheets(&set.id).is_ok());
    assert!(hand.markups(&set.id, 0).is_ok());

    let boss_id = boss.me().expect("me").id;
    boss.change_who_is_on(&project.id, &boss_id, true).expect("put on");

    assert!(hand.set(&set.id).is_err(), "the set was still readable");
    assert!(hand.sheets(&set.id).is_err(), "its sheets were still readable");
    assert!(hand.markups(&set.id, 0).is_err(), "its markups were still readable");
    assert!(hand.takeoff(&set.id).is_err(), "its takeoff was still readable");
}

#[test]
fn being_put_on_a_job_gives_it_back() {
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let hand = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");

    let project = boss.create_project("6742", "Elm West").expect("project");
    let boss_id = boss.me().expect("me").id;
    let hand_id = hand.me().expect("me").id;
    boss.change_who_is_on(&project.id, &boss_id, true).expect("put on");
    assert!(hand.project(&project.id).is_err());

    boss.change_who_is_on(&project.id, &hand_id, true).expect("put on");
    assert!(hand.project(&project.id).is_ok(), "they were put on the job");
}

#[test]
fn taking_the_last_person_off_makes_it_everybodys_again() {
    // The rule read backwards, and worth knowing before doing it: an empty
    // list does not mean "nobody", it means "no list".
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let hand = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");

    let project = boss.create_project("6742", "Elm West").expect("project");
    let boss_id = boss.me().expect("me").id;
    boss.change_who_is_on(&project.id, &boss_id, true).expect("put on");
    assert!(hand.project(&project.id).is_err());

    boss.change_who_is_on(&project.id, &boss_id, false).expect("take off");
    assert!(
        hand.project(&project.id).is_ok(),
        "with nobody named it is everybody's again"
    );
}

#[test]
fn an_administrator_sees_a_job_they_are_not_on() {
    // Somebody has to be able to find a job that the only person on it left
    // the company over.
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let hand = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");

    let project = boss.create_project("6742", "Elm West").expect("project");
    let hand_id = hand.me().expect("me").id;
    boss.change_who_is_on(&project.id, &hand_id, true).expect("put on");

    assert!(
        boss.project(&project.id).is_ok(),
        "an administrator was shut out of a job on their own server"
    );
    assert!(boss.projects().expect("list").iter().any(|p| p.id == project.id));
}

#[test]
fn only_an_administrator_decides_who_is_on_a_job() {
    // Not whoever happens to be on it already: that is how a list quietly
    // becomes a club.
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let hand = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");

    let project = boss.create_project("6742", "Elm West").expect("project");
    let hand_id = hand.me().expect("me").id;
    assert!(
        hand.change_who_is_on(&project.id, &hand_id, true).is_err(),
        "an estimator put themselves on a job"
    );
}

// ---- inviting one person --------------------------------------------------

/// The join code is one server-wide secret that names nobody, never expires
/// and is not used up. An invitation is the opposite of all four, and these
/// are the four tests that say so.
#[test]
fn an_invitation_gets_somebody_in_and_gives_them_the_role_it_says() {
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let invite = boss
        .invite("newhand@example.com", "New Hand", "estimator", None)
        .expect("invite");
    assert!(!invite.code.is_empty());
    assert_eq!(invite.role, "estimator");

    let mut joining = Client::new(&server.base);
    let session = joining
        .join(&hub::Join {
            code: invite.code.clone(),
            name: "New Hand".into(),
            email: "newhand@example.com".into(),
            password: "girder-splice-camber-bolt-88".into(),
        })
        .expect("join with an invitation");
    assert_eq!(session.user.role, hub::Role::Estimator);
}

#[test]
fn an_invitation_works_once_and_then_never_again() {
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let invite = boss
        .invite("newhand@example.com", "New Hand", "estimator", None)
        .expect("invite");

    let mut first = Client::new(&server.base);
    first
        .join(&hub::Join {
            code: invite.code.clone(),
            name: "New Hand".into(),
            email: "newhand@example.com".into(),
            password: "girder-splice-camber-bolt-88".into(),
        })
        .expect("the first use");

    // Somebody who was forwarded the same message gets nothing.
    let mut second = Client::new(&server.base);
    assert!(
        second
            .join(&hub::Join {
                code: invite.code,
                name: "Somebody Else".into(),
                email: "else@example.com".into(),
                password: "purlin-shim-weld-stud-21".into(),
            })
            .is_err(),
        "an invitation was used twice"
    );
}

#[test]
fn an_invitation_is_for_the_address_it_was_made_out_to() {
    // Otherwise one forwarded email is a spare account for whoever got it.
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let invite = boss
        .invite("newhand@example.com", "New Hand", "estimator", None)
        .expect("invite");

    let mut wrong = Client::new(&server.base);
    assert!(
        wrong
            .join(&hub::Join {
                code: invite.code,
                name: "Somebody Else".into(),
                email: "else@example.com".into(),
                password: "purlin-shim-weld-stud-21".into(),
            })
            .is_err(),
        "somebody else used an invitation made out to a colleague"
    );
}

#[test]
fn an_invitation_cannot_make_an_administrator() {
    // One forwarded email away from handing out the keys.
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    assert!(
        boss.invite("newhand@example.com", "New Hand", "admin", None).is_err(),
        "an invitation was allowed to make an administrator"
    );
}

#[test]
fn only_an_administrator_can_invite() {
    let server = start();
    let hand = signed_in(&server, "est@example.com", "camber-weld-joist-plate-19");
    assert!(hand.invite("newhand@example.com", "New Hand", "estimator", None).is_err());
}

#[test]
fn an_invitation_works_on_a_server_that_takes_nobody_otherwise() {
    // The point of inviting: the front door is shut and this person was
    // deliberately let in.
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    boss.change_joining(Some("closed"), None, false).expect("close it");

    let invite = boss
        .invite("newhand@example.com", "New Hand", "estimator", None)
        .expect("invite");
    let mut joining = Client::new(&server.base);
    assert!(
        joining
            .join(&hub::Join {
                code: invite.code,
                name: "New Hand".into(),
                email: "newhand@example.com".into(),
                password: "girder-splice-camber-bolt-88".into(),
            })
            .is_ok(),
        "an invitation should work whatever the front door is set to"
    );
}

#[test]
fn a_used_invitation_stops_being_listed_and_one_can_be_thrown_away() {
    let server = start();
    let boss = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let keep = boss
        .invite("one@example.com", "One", "estimator", None)
        .expect("invite");
    let toss = boss
        .invite("two@example.com", "Two", "estimator", None)
        .expect("invite");
    assert_eq!(boss.invites().expect("list").len(), 2);

    let left = boss.drop_invite(&toss.code).expect("drop");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].code, keep.code);

    let mut joining = Client::new(&server.base);
    joining
        .join(&hub::Join {
            code: keep.code,
            name: "One".into(),
            email: "one@example.com".into(),
            password: "girder-splice-camber-bolt-88".into(),
        })
        .expect("join");
    assert!(boss.invites().expect("list").is_empty(), "a used one was still listed");
}

/// Shares a tool chest the way Studio's "Share one" does.
fn share_chest(server: &Running, token: &str, filename: &str, bytes: &[u8]) {
    let boundary = "----hyperviewchest";
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    ureq::post(&format!("{}/api/v1/chests", server.base))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body)
        .expect("share a chest");
}

/// An `.evtools` chest with one beam, and the column names given.
fn an_evtools(columns: &[(usize, &str)]) -> Vec<u8> {
    use base64::Engine;
    let template = base64::engine::general_purpose::STANDARD
        .encode(b"<< /Type /Annot /Subtype /Line /Subj (W12x26) /IT /LineDimension >>");
    let columns: Vec<serde_json::Value> = columns
        .iter()
        .map(|(slot, name)| serde_json::json!({ "slot": slot, "name": name, "kind": "Number", "decimals": 2 }))
        .collect();
    serde_json::to_vec(&serde_json::json!({
        "format": "excalibur-view-tools",
        "version": 1,
        "name": "Steel",
        "columns": columns,
        "sets": [{ "title": "Beams", "tools": [{
            "subject": "W12x26", "kind": "Length", "colour": "#ff0000",
            "columns": ["26", "", "", "", "", ""], "template": template,
        }]}],
    }))
    .unwrap()
}

#[test]
fn a_chest_shared_from_studio_names_the_columns_in_the_markups_list() {
    let server = start();
    let admin = signed_in(&server, "sam@example.com", "brace-gusset-purlin-shim-42");
    let token = admin.token().unwrap().to_string();
    let project = admin.create_project("6743", "Elm West").expect("project");
    let set = upload(&server, &token, &project.id, &a_drawing());
    set_scale(&server, &set.id, 0, Some("1/4\" = 1'-0\""));
    admin
        .push_markups(&set.id, &[hub::NewMarkup { page: 0, dictionary: a_beam(720.0), replaces: None }])
        .expect("push");

    let columns = |server: &Running| -> serde_json::Map<String, serde_json::Value> {
        let list: serde_json::Value =
            ureq::get(&format!("{}/api/v1/projects/{}/markuplist", server.base, project.id))
                .set("Authorization", &format!("Bearer {token}"))
                .call()
                .expect("markuplist")
                .into_json()
                .unwrap();
        list.pointer("/results/0/markups/Markups/0/ExtendedProperties")
            .and_then(|v| v.as_object())
            .cloned()
            .expect("the beam's columns")
    };

    // Nothing shared: the slot has no name but its number.
    assert_eq!(columns(&server).get("Custom 1").and_then(|v| v.as_str()), Some("26"));

    // Share one: an `.evtools` is what Studio puts on the server.
    share_chest(&server, &token, "Steel.evtools", &an_evtools(&[(0, "LBS Per FT")]));
    let named = columns(&server);
    assert_eq!(named.get("LBS Per FT").and_then(|v| v.as_str()), Some("26"), "{named:?}");
    assert!(named.get("Custom 1").is_none());

    // A newer chest that names nothing, like a Revu tool set, doesn't take
    // the names away.
    share_chest(&server, &token, "Misc.evtools", &an_evtools(&[]));
    let still = columns(&server);
    assert_eq!(still.get("LBS Per FT").and_then(|v| v.as_str()), Some("26"), "{still:?}");
}
