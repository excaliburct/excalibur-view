//! Sign-offs, run for real over sockets: the office (or Fabwire with its
//! key) asks for signatures on a shop drawing; the shop signs on View; the
//! program reads the signatures back. The same job as the folder tests:
//! Sid is a viewer in Shop, Fay in Field (both see 10 SHOP DRAWINGS), Nan in
//! no group (sees neither), Eve an estimator. Only a person signs, a
//! signature is final, and a line on a set someone may not see is not there
//! for them.

#![allow(dead_code)] // WHAT: the folder tests' office, used in part. WHY: the same job, so the same rules are tested.

use std::io::Read; // WHAT: reading a download's bytes. WHY: files and drawings come back as bytes.
use std::sync::Arc; // WHAT: shared ownership of the server. WHY: the router takes it as an Arc.

use hyperview_server::api::{self, Server}; // WHAT: the router and the server state. WHY: the tests run the real thing.
use hyperview_server::{auth, Config, Store}; // WHAT: passwords, configuration and storage. WHY: to set up an office with people in it.
use rusqlite::params; // WHAT: bound SQL parameters. WHY: people are put in the database directly, as the other wire tests do.
use serde_json::{json, Value}; // WHAT: JSON values. WHY: requests and answers.

/// A folder that is gone when the test is.
struct Scratch(std::path::PathBuf); // WHAT: the server's data folder. WHY: each test gets its own.

impl Scratch {
    fn new() -> Scratch {
        let n: u64 = rand::random(); // WHAT: a random number. WHY: a folder name no other test is using.
        let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("hyperview-signoffs-{n:016x}")); // WHAT: the folder. WHY: under cargo's own folder for tests, not the shared system one.
        std::fs::create_dir_all(&path).expect("a scratch directory"); // WHAT: made. WHY: the store writes into it.
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0); // WHAT: removed. WHY: tests leave nothing behind.
    }
}

const PASSWORD: &str = "brace-gusset-purlin-shim-42"; // WHAT: everybody's password. WHY: long enough for the server's rules.
const FAY: &str = "usr_fay"; // WHAT: a viewer in the Field group. WHY: the person the rules are for.
const SID: &str = "usr_sid"; // WHAT: a viewer in the Shop group. WHY: a second group.
const NAN: &str = "usr_nan"; // WHAT: a viewer in no group. WHY: sees only what everyone sees.

/// A running office, signed in as each of its people.
struct Office {
    base: String,     // WHAT: http://127.0.0.1:port. WHY: every request goes there.
    scratch: Scratch, // WHAT: its data folder. WHY: kept alive while it runs, and read for the scale.
    admin: String,    // WHAT: Sam's session, the administrator. WHY: sets the job up.
    eve: String,      // WHAT: Eve's session, an estimator. WHY: the office sees everything.
    fay: String,      // WHAT: Fay's session, Field. WHY: under test.
    sid: String,      // WHAT: Sid's session, Shop. WHY: under test.
    nan: String,      // WHAT: Nan's session, no group. WHY: under test.
    _runtime: tokio::runtime::Runtime, // WHAT: the async runtime. WHY: dropping it stops the server.
}

fn an_office() -> Office {
    let scratch = Scratch::new(); // WHAT: a fresh folder. WHY: a fresh office.
    let config = Config {
        name: "Example Steel".into(), // WHAT: the office's name. WHY: any name.
        data: scratch.0.clone(),      // WHAT: where it keeps everything. WHY: the scratch folder.
        listen: "127.0.0.1:0".parse().unwrap(), // WHAT: any free port. WHY: tests run side by side.
        ..Config::default()
    };
    let store = Store::open(&config.database(), &config.blobs()).expect("a store"); // WHAT: the database. WHY: the people go in it.
    let hashed = auth::hash_password(PASSWORD).unwrap(); // WHAT: hashed once. WHY: Argon2 is slow on purpose; everybody shares it.
    store
        .with(|db| {
            for (id, name, email, role) in [
                ("usr_sam", "Sam", "sam@example.com", "admin"), // WHAT: the administrator. WHY: sets the job up.
                ("usr_eve", "Eve", "eve@example.com", "estimator"), // WHAT: an estimator. WHY: the office.
                (FAY, "Fay", "fay@example.com", "viewer"), // WHAT: the field. WHY: under test.
                (SID, "Sid", "sid@example.com", "viewer"), // WHAT: the shop. WHY: under test.
                (NAN, "Nan", "nan@example.com", "viewer"), // WHAT: nobody's group. WHY: under test.
            ] {
                db.execute(
                    "INSERT INTO people (id, name, email, password, role, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![id, name, email, hashed, role, "2026-09-01T00:00:00Z"],
                )?; // WHAT: one person, before licensing began. WHY: a founding install, so sharing is never paused.
            }
            Ok(())
        })
        .expect("the people");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime"); // WHAT: a small runtime. WHY: enough for one test.
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind(config.listen))
        .expect("a socket"); // WHAT: the socket. WHY: its port is the address.
    let address = listener.local_addr().expect("an address"); // WHAT: the port. WHY: the base address.
    let app = api::router(Arc::new(Server::new(store, config))); // WHAT: the real router. WHY: the gates under test are in it.
    runtime.spawn(async move {
        let _ = axum::serve(listener, app).await; // WHAT: serving. WHY: until the runtime is dropped.
    });
    let base = format!("http://{address}"); // WHAT: where it is. WHY: every request.
    let session = |email: &str| {
        let mut client = hub::Client::new(&base); // WHAT: the program's own client. WHY: signs in the way a seat does.
        client.sign_in(email, PASSWORD).expect("sign in"); // WHAT: signed in. WHY: a session to ask with.
        client.token().unwrap().to_string()
    };
    Office {
        admin: session("sam@example.com"),
        eve: session("eve@example.com"),
        fay: session("fay@example.com"),
        sid: session("sid@example.com"),
        nan: session("nan@example.com"),
        base,
        scratch,
        _runtime: runtime,
    }
}

impl Office {
    /// A request as `token`: the status and the answer's JSON (or its text).
    fn ask(&self, token: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let url = if path.starts_with("/mcp") {
            format!("{}{path}", self.base)
        } else {
            format!("{}/api/v1{path}", self.base)
        }; // WHAT: the address. WHY: the API, or the assistant's endpoint.
        let request = ureq::request(method, &url).set("Authorization", &format!("Bearer {token}")); // WHAT: as whoever. WHY: the gate under test.
        let answer = match body {
            Some(body) => request.send_json(body), // WHAT: with a body. WHY: the writes.
            None => request.call(),                // WHAT: without. WHY: the reads.
        };
        let read = |r: ureq::Response| {
            let text = r.into_string().unwrap_or_default(); // WHAT: the whole answer. WHY: some are CSV, not JSON.
            serde_json::from_str(&text).unwrap_or(Value::String(text)) // WHAT: JSON if it is. WHY: either is checked.
        };
        match answer {
            Ok(r) => (r.status(), read(r)), // WHAT: done. WHY: checked by the caller.
            Err(ureq::Error::Status(code, r)) => (code, read(r)), // WHAT: refused. WHY: checked by the caller.
            Err(e) => panic!("{method} {path}: {e}"), // WHAT: nothing answered. WHY: a broken test, not a refusal.
        }
    }

    fn get(&self, token: &str, path: &str) -> (u16, Value) {
        self.ask(token, "GET", path, None) // WHAT: a read. WHY: most of what is tested.
    }

    fn post(&self, token: &str, path: &str, body: Value) -> Value {
        let (status, said) = self.ask(token, "POST", path, Some(body)); // WHAT: a write expected to work. WHY: setting the job up.
        assert_eq!(status, 200, "POST {path}: {said}"); // WHAT: it worked. WHY: the rest depends on it.
        said
    }

    /// Bytes from a download, and its status.
    fn bytes(&self, token: &str, path: &str) -> (u16, Vec<u8>) {
        match ureq::get(&format!("{}/api/v1{path}", self.base))
            .set("Authorization", &format!("Bearer {token}"))
            .call()
        {
            Ok(r) => {
                let mut out = Vec::new(); // WHAT: the bytes. WHY: compared.
                r.into_reader().read_to_end(&mut out).unwrap(); // WHAT: read whole. WHY: a download.
                (200, out)
            }
            Err(ureq::Error::Status(code, r)) => {
                (code, r.into_string().unwrap_or_default().into_bytes())
            } // WHAT: refused, and what it said. WHY: compared with a missing one.
            Err(e) => panic!("GET {path}: {e}"),
        }
    }

    /// A multipart upload to `path` with `fields` and a file.
    fn form(
        &self,
        token: &str,
        path: &str,
        fields: &[(&str, &str)],
        filename: &str,
        file: &[u8],
    ) -> (u16, Value) {
        let boundary = "----hyperviewfolders"; // WHAT: the boundary. WHY: nothing in the body looks like it.
        let mut body = Vec::new(); // WHAT: the form. WHY: built by hand.
        for (name, value) in fields {
            body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
            // WHAT: one field. WHY: the form.
        }
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes()); // WHAT: the file's head. WHY: the server reads the file field.
        body.extend_from_slice(file); // WHAT: the file. WHY: uploaded.
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes()); // WHAT: the end. WHY: closes the form.
        let read = |r: ureq::Response| {
            serde_json::from_str(&r.into_string().unwrap_or_default()).unwrap_or(Value::Null)
        }; // WHAT: the JSON answer. WHY: checked.
        match ureq::post(&format!("{}/api/v1{path}", self.base))
            .set("Authorization", &format!("Bearer {token}"))
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&body)
        {
            Ok(r) => (r.status(), read(r)), // WHAT: taken. WHY: checked.
            Err(ureq::Error::Status(code, r)) => (code, read(r)), // WHAT: refused. WHY: checked.
            Err(e) => panic!("upload: {e}"),
        }
    }

    /// A drawing set put up into `folder`: its id.
    fn drawing(&self, token: &str, project: &str, name: &str, folder: &str, mark: &str) -> String {
        let (status, set) = self.form(
            token,
            "/sets",
            &[("project", project), ("name", name), ("folder", folder)],
            &format!("{name}.pdf"),
            &a_drawing(mark),
        ); // WHAT: the upload. WHY: a set in a folder.
        assert_eq!(status, 200, "{set}"); // WHAT: taken. WHY: the rest depends on it.
        set["id"].as_str().unwrap().to_string()
    }

    /// A file that isn't a drawing put up into `folder`: the answer.
    fn file(
        &self,
        token: &str,
        project: &str,
        folder: &str,
        name: &str,
        bytes: &[u8],
    ) -> (u16, Value) {
        self.form(
            token,
            "/files",
            &[
                ("project", project),
                ("folder", folder),
                ("modified", "2026-10-27T15:04:05.1234567Z"),
            ],
            name,
            bytes,
        ) // WHAT: the upload. WHY: a file in a folder.
    }

    /// One assistant tool's answer, asked with `token`.
    fn tool(&self, token: &str, name: &str, arguments: Value) -> String {
        let (status, said) = self.ask(token, "POST", "/mcp", Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": arguments } }))); // WHAT: a tool call. WHY: how an assistant asks.
        assert_eq!(status, 200, "{said}"); // WHAT: the endpoint answered. WHY: refusals come back inside.
        said.pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string() // WHAT: what the assistant is told. WHY: checked for what it names.
    }

    /// A key made by whoever `token` is.
    fn key(&self, token: &str, purpose: &str) -> String {
        self.post(
            token,
            "/me/keys",
            json!({ "purpose": purpose, "name": purpose }),
        )["key"]
            .as_str()
            .unwrap()
            .to_string() // WHAT: the key, shown once. WHY: used as a bearer.
    }

    /// Every sheet of `set` given a scale, so its lengths count.
    fn scaled(&self, set: &str) {
        let db = rusqlite::Connection::open(self.scratch.0.join("hyperview.sqlite")).unwrap(); // WHAT: the database. WHY: a scale is set by a seat in real life.
        db.execute(
            "UPDATE sheets SET scale = '1/4\" = 1''-0\"' WHERE set_id = ?1",
            params![set],
        )
        .unwrap(); // WHAT: a quarter-inch scale. WHY: a takeoff with numbers in it.
    }
}

/// A one-page drawing; `mark` makes its bytes its own.
fn a_drawing(mark: &str) -> Vec<u8> {
    let mut out = format!("%PDF-1.7\n%{mark}\n").into_bytes(); // WHAT: the header and a mark. WHY: two drawings are two files.
    let mut offsets = Vec::new(); // WHAT: where each object starts. WHY: the table needs them.
    for body in [
        "1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n",
        "2 0 obj\n<</Type/Pages/Kids[3 0 R]/Count 1>>\nendobj\n",
        "3 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]>>\nendobj\n",
    ] {
        offsets.push(out.len()); // WHAT: this object's offset. WHY: for the table.
        out.extend_from_slice(body.as_bytes()); // WHAT: the object. WHY: the file.
    }
    let xref = out.len(); // WHAT: where the table starts. WHY: the trailer points at it.
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    ); // WHAT: the table's head. WHY: PDF's layout.
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes()); // WHAT: one entry. WHY: PDF's layout.
    }
    out.extend_from_slice(
        format!(
            "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    ); // WHAT: the trailer. WHY: a reader starts there.
    out
}

/// A W12x26 beam ten feet long at a quarter-inch scale, base64, as a seat sends it.
fn a_beam(subject: &str) -> Value {
    use base64::Engine; // WHAT: base64. WHY: how a markup travels.
    let mut m = annot::Markup::new(annot::Subtype::Line); // WHAT: a length. WHY: something with a quantity.
    m.set("IT", pdf::Object::name("LineDimension")); // WHAT: a measured line. WHY: counted in the takeoff.
    m.set_subject(subject); // WHAT: what it is. WHY: named in the takeoff, so a leak would show.
    m.set_line([100.0, 100.0], [820.0, 100.0]); // WHAT: 720 points. WHY: ten feet at a quarter inch.
    let mut bytes = Vec::new(); // WHAT: the dictionary written out. WHY: as it would be in the file.
    pdf::write::write_object(&pdf::Object::Dict(m.dict), &mut bytes);
    json!([{ "page": 0, "dictionary": base64::engine::general_purpose::STANDARD.encode(bytes) }])
    // WHAT: one markup to push. WHY: the body /markups takes.
}

/// The job: who is in which group, its three folders and their rules, a
/// drawing and a file in each of the restricted ones, and a beam on each
/// drawing. The project's id, and the ids of the contract set, the shop set,
/// the contract file and the shop file.
struct Job {
    project: String,  // WHAT: the job. WHY: every route under it.
    contract: String, // WHAT: the drawing in the office's folder. WHY: must never reach Fay.
    shop: String,     // WHAT: the drawing in the field's folder. WHY: must reach Fay.
    open: String,     // WHAT: the drawing in the folder with no rule. WHY: everybody's, as before.
    estimate: String, // WHAT: the file in the office's folder. WHY: must never reach Fay.
    cut_list: String, // WHAT: the file in the field's folder. WHY: must reach Fay.
}

fn a_job(office: &Office) -> Job {
    let admin = &office.admin; // WHAT: Sam. WHY: sets it all up.
    let project = office.post(
        admin,
        "/projects",
        json!({ "number": "26-500", "name": "Lakeside", "reference": "program:job:500" }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string(); // WHAT: a job filed under a program's reference. WHY: a sync key may reach it too.
    for (name, person) in [("Field", FAY), ("Shop", SID)] {
        let group = office.post(admin, "/groups", json!({ "name": name }))["id"]
            .as_str()
            .unwrap()
            .to_string(); // WHAT: the group. WHY: the folders are given to it.
        office.post(
            admin,
            &format!("/groups/{group}/people"),
            json!({ "person": person }),
        ); // WHAT: its person put in. WHY: they see its folders.
    }
    let contract = office.drawing(admin, &project, "Contract", "01 CONTRACT", "contract"); // WHAT: the contract. WHY: the office's.
    let shop = office.drawing(
        admin,
        &project,
        "S-101",
        "10 SHOP DRAWINGS/10.4 FOR FABRICATION",
        "shop",
    ); // WHAT: a shop drawing, a folder down. WHY: the field's, by its parent's rule.
    let open = office.drawing(admin, &project, "A-101", "07 CONTRACT DRAWINGS", "open"); // WHAT: an architectural sheet. WHY: in a folder with no rule.
    let (_, estimate) = office.file(
        admin,
        &project,
        "01 CONTRACT",
        "Estimate.xlsx",
        b"PK the office's prices",
    ); // WHAT: the estimate. WHY: the office's.
    let (_, cut_list) = office.file(
        admin,
        &project,
        "10 SHOP DRAWINGS",
        "Cut list.xlsx",
        b"PK the cut list",
    ); // WHAT: a cut list. WHY: the field's.
    office.post(
        admin,
        &format!("/projects/{project}/folders"),
        json!({ "folders": [
            { "folder": "01 CONTRACT", "see": "office" },
            { "folder": "10 SHOP DRAWINGS", "see": ["Shop", "Field"] }
        ] }),
    ); // WHAT: the rules. WHY: the shape the shop asked for; 07 is left with none.
    for set in [&contract, &shop, &open] {
        office.scaled(set); // WHAT: a scale on it. WHY: its beam counts.
    }
    office.post(
        admin,
        &format!("/sets/{contract}/markups"),
        a_beam("SECRET CONTRACT BEAM"),
    ); // WHAT: a beam on the contract. WHY: a takeoff that must not leak.
    office.post(admin, &format!("/sets/{shop}/markups"), a_beam("W12x26")); // WHAT: a beam on the shop drawing. WHY: a takeoff the field sees.
    Job {
        project,
        contract,
        shop,
        open,
        estimate: estimate["id"].as_str().unwrap().to_string(),
        cut_list: cut_list["id"].as_str().unwrap().to_string(),
    }
}

fn names(list: &Value, field: &str) -> Vec<String> {
    list.as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|x| x[field].as_str().unwrap_or_default().to_string())
        .collect() // WHAT: one field of each. WHY: what a list names.
}

/// A signature drawn on the pad: two strokes.
fn a_signature(name: &str) -> Value {
    json!({ "name": name, "strokes": [[[20.0, 100.0], [80.0, 30.0], [140.0, 100.0]], [[160.0, 60.0], [300.0, 70.0]]] })
    // WHAT: a name and two strokes. WHY: what the pad sends.
}

/// The two lines Eve asks for on the shop drawing: their ids, in order.
fn asked(office: &Office, job: &Job) -> Vec<String> {
    let lines = office.post(
        &office.eve,
        &format!("/sets/{}/signoffs", job.shop),
        json!([
            { "label": "B12 · WELD", "group": "Stair 1", "ref": "trav:500:B12:weld", "page": 0 },
            { "label": "B12 · FINAL INSPECTION", "group": "Stair 1", "ref": "trav:500:B12:final" }
        ]),
    ); // WHAT: two stations of one mark. WHY: a traveler's lines.
    lines
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn the_shop_signs_what_it_can_see_and_the_asker_reads_it_back() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: its shop drawing carries the lines.
    let ids = asked(&office, &job); // WHAT: two lines asked for. WHY: to sign.
    assert_eq!(ids.len(), 2);
    let (status, seen) = office.get(&office.sid, &format!("/sets/{}/signoffs", job.shop)); // WHAT: as Sid, Shop. WHY: he sees the shop folder.
    assert_eq!(status, 200, "{seen}");
    assert_eq!(names(&seen, "status"), ["open", "open"]); // WHAT: both waiting. WHY: nobody has signed.
    assert_eq!(
        names(&seen, "label"),
        ["B12 · WELD", "B12 · FINAL INSPECTION"]
    ); // WHAT: in the order asked. WHY: the form's order.
    let signed = office.post(
        &office.sid,
        &format!("/signoffs/{}/sign", ids[0]),
        a_signature("Sid Ortiz"),
    ); // WHAT: Sid signs the weld. WHY: under test.
    assert_eq!(signed["status"], "signed");
    assert_eq!(signed["signer"], "Sid Ortiz"); // WHAT: the name written. WHY: printed on the form.
    assert_eq!(signed["signed_by"], SID); // WHAT: his account. WHY: who really signed.
                                          // The asker reads it back from the start, and then from where it left off.
    let (status, changes) = office.get(&office.eve, "/signoffs?since=0"); // WHAT: every change. WHY: the program catching up.
    assert_eq!(status, 200, "{changes}");
    let lines = &changes["signoffs"];
    assert_eq!(lines.as_array().unwrap().len(), 2, "{changes}"); // WHAT: both lines (one signed since). WHY: each appears once, as it is now.
    let latest = changes["latest"].as_i64().unwrap(); // WHAT: how far it read. WHY: asked from next time.
    let (_, after) = office.get(&office.eve, &format!("/signoffs?since={latest}")); // WHAT: nothing new since. WHY: nothing read twice.
    assert_eq!(after["signoffs"].as_array().unwrap().len(), 0, "{after}");
    // The signature as a picture: a PNG, 400 by 140.
    let (status, png) = office.bytes(&office.eve, &format!("/signoffs/{}/signature.png", ids[0]));
    assert_eq!(status, 200);
    assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
    let (status, _) = office.bytes(&office.eve, &format!("/signoffs/{}/signature.png", ids[1])); // WHAT: the unsigned line's. WHY: nothing to draw.
    assert_eq!(status, 404);
}

#[test]
fn a_signature_is_final_and_only_a_person_signs() {
    let office = an_office();
    let job = a_job(&office);
    let ids = asked(&office, &job);
    office.post(
        &office.fay,
        &format!("/signoffs/{}/sign", ids[0]),
        a_signature("Fay"),
    ); // WHAT: Fay, Field, signs. WHY: she sees the shop folder too.
    let (status, said) = office.ask(
        &office.sid,
        "POST",
        &format!("/signoffs/{}/sign", ids[0]),
        Some(a_signature("Sid")),
    ); // WHAT: signing it again. WHY: final.
    assert_eq!(status, 409, "{said}");
    assert!(
        said.to_string().contains("already signed, by Fay"),
        "{said}"
    );
    let (status, said) = office.ask(
        &office.eve,
        "POST",
        &format!("/signoffs/{}/withdraw", ids[0]),
        None,
    ); // WHAT: withdrawing a signed line. WHY: a signature stays.
    assert_eq!(status, 409, "{said}");
    // A program's key asks and reads, but never signs.
    let key = office.key(&office.admin, "integration"); // WHAT: a key for another program, made by the administrator. WHY: Fabwire's kind.
    let (status, said) = office.ask(
        &key,
        "POST",
        &format!("/signoffs/{}/sign", ids[1]),
        Some(a_signature("A program")),
    ); // WHAT: the key signing. WHY: refused.
    assert_eq!(status, 403, "{said}");
    let (status, _) = office.get(&key, "/signoffs?since=0"); // WHAT: the key reading back. WHY: what it's for.
    assert_eq!(status, 200);
    // A signature that wasn't drawn on the pad isn't kept.
    let (status, _) = office.ask(
        &office.sid,
        "POST",
        &format!("/signoffs/{}/sign", ids[1]),
        Some(json!({ "name": "Sid", "strokes": [] })),
    ); // WHAT: nothing drawn. WHY: refused.
    assert_eq!(status, 400);
    let (status, _) = office.ask(
        &office.sid,
        "POST",
        &format!("/signoffs/{}/sign", ids[1]),
        Some(json!({ "name": "Sid", "strokes": [[[1.0, 1.0], [9999.0, 1.0]]] })),
    ); // WHAT: off the pad. WHY: refused.
    assert_eq!(status, 400);
    // An unsigned line withdrawn can't then be signed.
    office.post(
        &office.eve,
        &format!("/signoffs/{}/withdraw", ids[1]),
        json!({}),
    );
    let (status, said) = office.ask(
        &office.sid,
        "POST",
        &format!("/signoffs/{}/sign", ids[1]),
        Some(a_signature("Sid")),
    );
    assert_eq!(status, 409, "{said}");
}

#[test]
fn a_line_on_a_set_someone_may_not_see_is_not_there_for_them() {
    let office = an_office();
    let job = a_job(&office);
    let ids = asked(&office, &job);
    let (status, said) = office.get(&office.nan, &format!("/sets/{}/signoffs", job.shop)); // WHAT: Nan, in no group. WHY: the shop folder isn't hers.
    assert_eq!(status, 404, "{said}");
    let (status, said) = office.ask(
        &office.nan,
        "POST",
        &format!("/signoffs/{}/sign", ids[0]),
        Some(a_signature("Nan")),
    ); // WHAT: signing it anyway. WHY: refused as missing.
    assert_eq!(status, 404, "{said}");
    let (status, _) = office.bytes(&office.nan, &format!("/signoffs/{}/signature.png", ids[0]));
    assert_eq!(status, 404);
    let (status, _) = office.ask(
        &office.nan,
        "POST",
        "/signoffs/so_nothing/sign",
        Some(a_signature("Nan")),
    ); // WHAT: a line that doesn't exist. WHY: answered the same way.
    assert_eq!(status, 404);
    // A viewer can't ask for sign-offs, or read every change.
    let (status, _) = office.ask(
        &office.sid,
        "POST",
        &format!("/sets/{}/signoffs", job.shop),
        Some(json!([{ "label": "x", "ref": "x" }])),
    );
    assert_eq!(status, 403);
    let (status, _) = office.get(&office.sid, "/signoffs?since=0");
    assert_eq!(status, 403);
}

#[test]
fn asking_again_makes_no_doubles_and_a_sync_key_keeps_to_its_own_projects() {
    let office = an_office();
    let job = a_job(&office);
    asked(&office, &job);
    let again = asked(&office, &job); // WHAT: the same lines asked for again. WHY: a program after a restart.
    assert_eq!(again.len(), 2, "no doubles"); // WHAT: still two. WHY: the same references.
    let (status, said) = office.ask(
        &office.eve,
        "POST",
        &format!("/sets/{}/signoffs", job.shop),
        Some(json!([{ "label": "\u{7}bell", "ref": "x" }])),
    ); // WHAT: a control character. WHY: refused.
    assert_eq!(status, 400, "{said}");
    // A sync key reaches its program's own project (this job was filed under a program's reference).
    let sync = office.key(&office.admin, "sync"); // WHAT: a sync key, made by the administrator. WHY: how Fabwire connects.
    let (status, said) = office.ask(
        &sync,
        "POST",
        &format!("/sets/{}/signoffs", job.shop),
        Some(json!([{ "label": "Load 1 · Received by", "ref": "ship:1:received" }])),
    );
    assert_eq!(status, 200, "{said}");
    let (status, _) = office.get(&sync, "/signoffs?since=0");
    assert_eq!(status, 200);
}

#[test]
fn another_askers_line_is_never_handed_back_and_a_withdrawal_is_on_the_log() {
    let office = an_office();
    let job = a_job(&office);
    let ids = asked(&office, &job); // WHAT: Eve's two lines. WHY: hers.
    let theirs = office.post(
        &office.admin,
        &format!("/sets/{}/signoffs", job.shop),
        json!([{ "label": "B12 · PAINT", "ref": "trav:500:B12:weld" }]),
    ); // WHAT: Sam asks with Eve's reference and other words. WHY: as another program with a key might.
    let mine: Vec<&Value> = theirs
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["ref"] == "trav:500:B12:weld")
        .collect(); // WHAT: every line with that reference. WHY: Sam's own is new, Eve's untouched.
    assert_eq!(mine.len(), 2, "{theirs}"); // WHAT: two lines, one each. WHY: a line is never handed to another asker as theirs.
    assert!(mine
        .iter()
        .any(|l| l["label"] == "B12 · PAINT" && l["id"] != ids[0].as_str())); // WHAT: Sam's is his own. WHY: the words he asked.
    let (status, taken) = office.ask(
        &office.eve,
        "POST",
        &format!("/signoffs/{}/withdraw", ids[1]),
        None,
    ); // WHAT: Eve takes one back. WHY: logged.
    assert_eq!(status, 200, "{taken}");
    office.ask(
        &office.eve,
        "POST",
        &format!("/signoffs/{}/withdraw", ids[1]),
        None,
    ); // WHAT: taken back again. WHY: nothing happens, so nothing is logged.
    let (_, log) = office.get(&office.admin, "/audit"); // WHAT: the log. WHY: checked.
    assert_eq!(
        log.to_string().matches("withdrew a sign-off").count(),
        1,
        "{log}"
    ); // WHAT: there once. WHY: who took a line back can be found, and a repeat adds no noise.
}

#[test]
fn signatures_already_made_are_read_back_after_an_office_trial_ends() {
    let office = an_office();
    let job = a_job(&office);
    let ids = asked(&office, &job); // WHAT: two lines. WHY: one is signed before the trial ends.
    office.post(
        &office.sid,
        &format!("/signoffs/{}/sign", ids[0]),
        a_signature("Sid Ortiz"),
    ); // WHAT: signed. WHY: must still be read back.
    {
        let db = rusqlite::Connection::open(office.scratch.0.join("hyperview.sqlite")).unwrap(); // WHAT: the server's database. WHY: the trial ended long ago.
        db.execute("DELETE FROM settings WHERE key = 'founding_install'", [])
            .unwrap(); // WHAT: no longer a founding install. WHY: an ordinary office.
        db.execute(
            "UPDATE settings SET value = '2026-01-01T00:00:00Z' WHERE key = 'licensing_began'",
            [],
        )
        .unwrap(); // WHAT: its trial began in January. WHY: long over.
    }
    let (status, changes) = office.get(&office.eve, "/signoffs?since=0"); // WHAT: the signatures read back. WHY: never behind the license.
    assert_eq!(status, 200, "{changes}");
    assert!(changes.to_string().contains("Sid Ortiz"), "{changes}"); // WHAT: the signature made before. WHY: nothing already signed is lost to a lapsed license.
}
