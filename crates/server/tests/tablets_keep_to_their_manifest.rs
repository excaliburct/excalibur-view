//! The tablet manifest, run for real over sockets (docs/TABLET-MANIFEST.md).
//!
//! One job, its folders and their rules as Fabwire sets them: `01 CONTRACT`
//! for the office, `10 SHOP DRAWINGS` and `08 SHIPPING DOCS` for Shop and
//! Field, `` `OUT OF USE (OFFICE ONLY) `` for the office. Fay is a viewer in
//! Field, Sid in Shop, Nan in neither; Eve is an estimator. Fabwire's sync
//! key sets the manifest. Only the program's key may set it; it only ever
//! narrows what the folder rules allow; a tablet is served only released
//! sheets the manifest names; and the receiver's signature goes through the
//! existing sign-off line, the account that collected it kept.

use std::sync::Arc; // WHAT: shared ownership of the server. WHY: the router takes it as an Arc.

use hyperview_server::api::{self, Server}; // WHAT: the router and the server state. WHY: the tests run the real thing.
use hyperview_server::{auth, Config, Store}; // WHAT: passwords, configuration and storage. WHY: an office with people in it.
use rusqlite::params; // WHAT: bound SQL parameters. WHY: people are put in the database directly, as the other wire tests do.
use serde_json::{json, Value}; // WHAT: JSON values. WHY: requests and answers.

/// A folder that is gone when the test is.
struct Scratch(std::path::PathBuf); // WHAT: the server's data folder. WHY: each test gets its own.

impl Scratch {
    fn new() -> Scratch {
        let n: u64 = rand::random(); // WHAT: a random number. WHY: a folder name no other test is using.
        let path =
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("tablets-{n:016x}")); // WHAT: the folder. WHY: under the build's own scratch folder, never a shared one
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
const FAY: &str = "usr_fay"; // WHAT: a viewer in Field. WHY: the receiver's side.
const SID: &str = "usr_sid"; // WHAT: a viewer in Shop. WHY: the shop's side.
const NAN: &str = "usr_nan"; // WHAT: a viewer in neither. WHY: reaches nothing of a job with a manifest.

/// A running office, signed in as each of its people, and Fabwire's key.
struct Office {
    base: String,     // WHAT: http://127.0.0.1:port. WHY: every request goes there.
    scratch: Scratch, // WHAT: its data folder. WHY: kept alive, and the audit read from it.
    admin: String,    // WHAT: Sam's session, the administrator. WHY: sets the job up.
    eve: String,      // WHAT: Eve's session, an estimator. WHY: the office sees everything.
    fay: String,      // WHAT: Fay's session, Field. WHY: under test.
    sid: String,      // WHAT: Sid's session, Shop. WHY: under test.
    nan: String,      // WHAT: Nan's session, no group. WHY: under test.
    sync: String, // WHAT: Fabwire's sync key, made by Sam. WHY: the only one that sets a manifest.
    _runtime: tokio::runtime::Runtime, // WHAT: the async runtime. WHY: dropping it stops the server.
}

fn an_office() -> Office {
    let scratch = Scratch::new(); // WHAT: a fresh folder. WHY: a fresh office.
    let config = Config {
        name: "Example Steel".into(),
        data: scratch.0.clone(),
        listen: "127.0.0.1:0".parse().unwrap(),
        ..Config::default()
    }; // WHAT: an office on any free port. WHY: tests run side by side.
    let store = Store::open(&config.database(), &config.blobs()).expect("a store"); // WHAT: the database. WHY: the people go in it.
    let hashed = auth::hash_password(PASSWORD).unwrap(); // WHAT: hashed once. WHY: Argon2 is slow on purpose.
    store
        .with(|db| {
            for (id, name, email, role) in [("usr_sam", "Sam", "sam@example.com", "admin"), ("usr_eve", "Eve", "eve@example.com", "estimator"), (FAY, "Fay", "fay@example.com", "viewer"), (SID, "Sid", "sid@example.com", "viewer"), (NAN, "Nan", "nan@example.com", "viewer")] {
                db.execute("INSERT INTO people (id, name, email, password, role, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![id, name, email, hashed, role, "2026-09-01T00:00:00Z"])?; // WHAT: one person, before licensing began. WHY: a founding install, so sharing is never paused.
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
    let admin = session("sam@example.com"); // WHAT: Sam. WHY: makes the key below.
    let mut office = Office {
        eve: session("eve@example.com"),
        fay: session("fay@example.com"),
        sid: session("sid@example.com"),
        nan: session("nan@example.com"),
        admin,
        sync: String::new(),
        base,
        scratch,
        _runtime: runtime,
    }; // WHAT: the office. WHY: the key is made through it
    office.sync = office.post(
        &office.admin.clone(),
        "/me/keys",
        json!({ "purpose": "sync", "name": "Fabwire V2" }),
    )["key"]
        .as_str()
        .unwrap()
        .to_string(); // WHAT: Fabwire's sync key. WHY: as Fabwire makes it
    office
}

impl Office {
    /// A request as `token`: the status and the answer's JSON (or its text).
    fn ask(&self, token: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let request = ureq::request(method, &format!("{}/api/v1{path}", self.base))
            .set("Authorization", &format!("Bearer {token}")); // WHAT: as whoever. WHY: the gate under test.
        let answer = match body {
            Some(body) => request.send_json(body), // WHAT: with a body. WHY: the writes.
            None => request.call(),                // WHAT: without. WHY: the reads.
        };
        let read = |r: ureq::Response| {
            serde_json::from_str(&r.into_string().unwrap_or_default()).unwrap_or(Value::Null)
        }; // WHAT: the JSON answer. WHY: checked.
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

    fn put(&self, token: &str, project: &str, manifest: &Value) -> (u16, Value) {
        self.ask(
            token,
            "PUT",
            &format!("/projects/{project}/tablet"),
            Some(manifest.clone()),
        ) // WHAT: a manifest sent. WHY: the route under test.
    }

    /// A file put up into `folder` (a drawing when `name` ends .pdf): its id.
    fn upload(&self, project: &str, folder: &str, name: &str, bytes: &[u8]) -> String {
        let boundary = "----hyperviewtablets"; // WHAT: the boundary. WHY: nothing in the body looks like it.
        let pdf = std::path::Path::new(name)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf")); // WHAT: a drawing or a file. WHY: two routes.
        let mut body = Vec::new(); // WHAT: the form. WHY: built by hand.
        let fields: Vec<(&str, &str)> = if pdf {
            vec![
                ("project", project),
                ("name", name.trim_end_matches(".pdf")),
                ("folder", folder),
            ]
        } else {
            vec![
                ("project", project),
                ("folder", folder),
                ("modified", "2026-11-20T10:00:00Z"),
            ]
        }; // WHAT: the form's fields. WHY: each route's own
        for (k, v) in fields {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n"
                )
                .as_bytes(),
            ); // WHAT: one field. WHY: the form.
        }
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes()); // WHAT: the file's head. WHY: the server reads the file field.
        body.extend_from_slice(bytes); // WHAT: the file. WHY: uploaded.
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes()); // WHAT: the end. WHY: closes the form.
        let answer = ureq::post(&format!(
            "{}/api/v1{}",
            self.base,
            if pdf { "/sets" } else { "/files" }
        ))
        .set("Authorization", &format!("Bearer {}", self.admin))
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .send_bytes(&body)
        .expect("uploaded"); // WHAT: sent as Sam. WHY: the office puts it up
        answer.into_json::<Value>().unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// How many audit lines say `action`.
    fn audited(&self, action: &str) -> i64 {
        let db = rusqlite::Connection::open(self.scratch.0.join("hyperview.sqlite")).unwrap(); // WHAT: the database. WHY: the audit log is in it.
        db.query_row(
            "SELECT count(*) FROM audit WHERE action = ?1",
            params![action],
            |r| r.get(0),
        )
        .unwrap() // WHAT: counted. WHY: one line per change
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

/// The job, as Fabwire mirrors it: ids by their names in the shared examples.
struct Job {
    project: String,                  // WHAT: the job. WHY: every route under it.
    ids: Vec<(&'static str, String)>, // WHAT: each example id and the real id it stands for. WHY: the examples' ids are symbolic
    unreleased: String, // WHAT: a sheet still For Approval, in a folder the shop and field may see. WHY: the manifest must keep it off the tablets
    contract: String, // WHAT: the contract, in the office's folder. WHY: never reachable, whatever any manifest says
    other_job_set: String, // WHAT: a drawing on another job. WHY: a manifest can't name it
}

impl Job {
    fn id(&self, name: &str) -> &str {
        &self.ids.iter().find(|(n, _)| *n == name).unwrap().1 // WHAT: the real id. WHY: looked up by the example's name
    }
}

fn a_job(office: &Office) -> Job {
    let admin = &office.admin; // WHAT: Sam. WHY: sets it all up.
    let project = office.post(
        admin,
        "/projects",
        json!({ "number": "2601", "name": "TCU Campus 2", "reference": "fabwire:job:2601" }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string(); // WHAT: a job filed under Fabwire's reference. WHY: its sync key reaches it.
    let other = office.post(
        admin,
        "/projects",
        json!({ "number": "2614", "name": "Pueblo", "reference": "fabwire:job:2614" }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string(); // WHAT: another job. WHY: its drawings are never this job's.
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
    let fab = "10 SHOP DRAWINGS/10.4 FOR FABRICATION"; // WHAT: where released sheets are. WHY: as Fabwire files them
    let mut ids = Vec::new(); // WHAT: the example ids. WHY: filled below
    for (name, folder) in [
        ("set_s101", fab),
        ("set_s102", fab),
        ("set_e101", fab),
        ("set_a001", fab),
        ("set_ticket2", "08 SHIPPING DOCS"),
        ("set_ticket3", "08 SHIPPING DOCS"),
        ("set_traveler", "09 SHOP TRAVELER"),
        ("set_loadlog", "08 SHIPPING DOCS"),
        ("set_asi04", "03 ASI DOCS"),
    ] {
        ids.push((
            name,
            office.upload(&project, folder, &format!("{name}.pdf"), &a_drawing(name)),
        )); // WHAT: each set the steel example names. WHY: real ids to swap in
    }
    for (name, folder) in [
        ("file_logistics", "07 CONTRACT DRAWINGS"),
        ("file_boltform", "10 SHOP DRAWINGS"),
    ] {
        ids.push((
            name,
            office.upload(&project, folder, &format!("{name}.xlsx"), name.as_bytes()),
        )); // WHAT: each file it names. WHY: likewise
    }
    let unreleased = office.upload(
        &project,
        "10 SHOP DRAWINGS/10.1 FOR APPROVAL",
        "S-103.pdf",
        &a_drawing("unreleased"),
    ); // WHAT: a sheet not released. WHY: the shop and field folders hold it, the manifest doesn't
    let contract = office.upload(
        &project,
        "01 CONTRACT",
        "Contract.pdf",
        &a_drawing("contract"),
    ); // WHAT: the contract. WHY: office only
    let other_job_set = office.upload(&other, "10 SHOP DRAWINGS", "X-1.pdf", &a_drawing("other")); // WHAT: another job's sheet. WHY: never this job's
    office.post(
        admin,
        &format!("/projects/{project}/folders"),
        json!({ "folders": [
        { "folder": "01 CONTRACT", "see": "office" },
        { "folder": "10 SHOP DRAWINGS", "see": ["Shop", "Field"] },
        { "folder": "08 SHIPPING DOCS", "see": ["Shop", "Field"] },
        { "folder": "09 SHOP TRAVELER", "see": ["Shop"] },
        { "folder": "03 ASI DOCS", "see": ["Shop", "Field"] },
        { "folder": "`OUT OF USE (OFFICE ONLY)", "see": "office" }
    ] }),
    ); // WHAT: the folder rules, as Fabwire's starting rules with the shipping fix. WHY: the manifest narrows these
    Job {
        project,
        ids,
        unreleased,
        contract,
        other_job_set,
    }
}

/// The shared steel-job example, its symbolic ids swapped for the job's real ones.
fn steel(job: &Job) -> Value {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/tablet-manifest-vectors.json"); // WHAT: the shared file. WHY: Fabwire sends what it says
    let mut text = serde_json::from_str::<Value>(&std::fs::read_to_string(path).unwrap()).unwrap()
        ["valid"][0]["manifest"]
        .to_string(); // WHAT: the steel job, as text. WHY: ids swapped by name
    for (name, id) in &job.ids {
        text = text.replace(&format!("\"{name}\""), &format!("\"{id}\"")); // WHAT: each id swapped. WHY: the server checks they're this job's
    }
    serde_json::from_str(&text).unwrap()
}

#[test]
fn only_the_programs_key_sets_a_manifest_and_every_change_is_audited_once() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    let manifest = steel(&job); // WHAT: the shared example. WHY: what Fabwire sends.
    let (status, said) = office.put(&office.admin, &job.project, &manifest); // WHAT: an administrator tries. WHY: only Fabwire sets it
    assert_eq!(
        (status, said["error"].as_str()),
        (403, Some("program_only")),
        "{said}"
    ); // WHAT: refused. WHY: the program-key-only guard
    let integration = office.post(
        &office.admin,
        "/me/keys",
        json!({ "purpose": "integration", "name": "other" }),
    )["key"]
        .as_str()
        .unwrap()
        .to_string(); // WHAT: another program's key. WHY: not Fabwire's kind
    assert_eq!(office.put(&integration, &job.project, &manifest).0, 403); // WHAT: refused. WHY: likewise
    assert_eq!(office.audited("changed what a job's tablets show"), 0); // WHAT: nothing written. WHY: nothing changed
    let (status, said) = office.put(&office.sync, &job.project, &manifest); // WHAT: Fabwire's key. WHY: the one that may
    assert_eq!(status, 200, "{said}"); // WHAT: kept. WHY: the shared example is valid
    assert_eq!(
        (said["version"].as_i64(), said["changed"].as_bool()),
        (Some(1), Some(true))
    ); // WHAT: the first version. WHY: counted
    assert_eq!(office.audited("changed what a job's tablets show"), 1); // WHAT: one line. WHY: every change audited
    let (_, again) = office.put(&office.sync, &job.project, &manifest); // WHAT: the same again. WHY: Fabwire pushes every minute
    assert_eq!(
        (again["version"].as_i64(), again["changed"].as_bool()),
        (Some(1), Some(false))
    ); // WHAT: nothing changed. WHY: the same digest
    assert_eq!(office.audited("changed what a job's tablets show"), 1); // WHAT: no new line. WHY: nothing changed
    let (status, kept) = office.get(&office.sync, &format!("/projects/{}/tablet", job.project)); // WHAT: Fabwire reads it back. WHY: checks what it sent
    assert_eq!(status, 200);
    assert_eq!(kept["manifest"], manifest); // WHAT: as sent. WHY: nothing lost
}

#[test]
fn a_manifest_that_breaks_the_contract_is_refused_with_its_code() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    let code = |m: &Value| {
        let (status, said) = office.put(&office.sync, &job.project, m); // WHAT: sent. WHY: refused below
        assert_eq!(status, 400, "{said}"); // WHAT: a bad request. WHY: never kept
        said["message"]
            .as_str()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap()
            .to_string() // WHAT: the code before the colon. WHY: as the contract says
    };
    let mut m = steel(&job); // WHAT: the example. WHY: broken one way at a time
    m["sheets"][1]["released"] = json!(""); // WHAT: a sheet not released. WHY: the released-only guard
    assert_eq!(code(&m), "not_released");
    let mut m = steel(&job);
    m["sheets"][0]["set"] = json!(job.other_job_set); // WHAT: another job's drawing. WHY: a manifest can't reach past its own job
    assert_eq!(code(&m), "not_in_project");
    let mut m = steel(&job);
    m["sheets"][0]["set"] = json!("set_nothing"); // WHAT: no such set. WHY: likewise
    assert_eq!(code(&m), "not_in_project");
    let mut m = steel(&job);
    m["extra"] = json!(1); // WHAT: a field the contract doesn't name. WHY: never half-read
    assert_eq!(code(&m), "unknown_field");
    let mut m = steel(&job);
    m["loads"][0]["boxes"][2]["line"] = json!({ "id": "x", "status": "signed" }); // WHAT: the program claiming a box signed. WHY: only the server says so
    assert_eq!(code(&m), "unknown_field");
    let out = office.upload(
        &job.project,
        "10 SHOP DRAWINGS/`OUT OF USE (OFFICE ONLY)",
        "S-099.pdf",
        &a_drawing("old"),
    ); // WHAT: a sheet put out of use. WHY: never a tablet's
    let mut m = steel(&job);
    m["sheets"][0]["set"] = json!(out); // WHAT: named anyway. WHY: refused
    assert_eq!(code(&m), "out_of_use");
}

#[test]
fn a_tablet_account_is_served_only_the_manifests_released_sheets_and_pins() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    let unreleased = format!("/sets/{}", job.unreleased); // WHAT: the sheet still For Approval. WHY: its folder is the field's
    assert_eq!(
        office.get(&office.fay, &unreleased).0,
        200,
        "before any manifest, the folder rules alone decide"
    ); // WHAT: Fay reaches it. WHY: no manifest yet, as before 0.8.0
    let (status, said) = office.put(&office.sync, &job.project, &steel(&job)); // WHAT: the manifest. WHY: from now on it narrows
    assert_eq!(status, 200, "{said}");
    assert_eq!(office.get(&office.fay, &unreleased).0, 404); // WHAT: gone for Fay. WHY: released only, enforced on the server
    assert_eq!(
        office
            .get(&office.fay, &format!("/sets/{}/file", job.unreleased))
            .0,
        404
    ); // WHAT: its bytes too. WHY: every route through one gate
    assert_eq!(office.get(&office.eve, &unreleased).0, 200); // WHAT: Eve still reaches it. WHY: the office is never narrowed
    assert_eq!(
        office
            .get(&office.fay, &format!("/sets/{}", job.id("set_s101")))
            .0,
        200
    ); // WHAT: a released sheet. WHY: named in the manifest
    assert_eq!(
        office
            .get(&office.fay, &format!("/sets/{}", job.id("set_traveler")))
            .0,
        404
    ); // WHAT: the traveler. WHY: the field's sections don't include it, and its folder is the shop's
    assert_eq!(
        office
            .get(&office.sid, &format!("/sets/{}", job.id("set_traveler")))
            .0,
        200
    ); // WHAT: Sid reaches it. WHY: Traveler is on for the shop
    assert_eq!(
        office
            .get(&office.sid, &format!("/sets/{}", job.id("set_asi04")))
            .0,
        404
    ); // WHAT: the ASI. WHY: pinned for the field only
    assert_eq!(
        office
            .get(&office.fay, &format!("/sets/{}", job.id("set_asi04")))
            .0,
        200
    ); // WHAT: Fay reaches it. WHY: pinned for her
    assert_eq!(
        office
            .get(
                &office.fay,
                &format!("/files/{}/file", job.id("file_boltform"))
            )
            .0,
        404
    ); // WHAT: the shop's pinned form. WHY: not the field's
    assert_eq!(
        office
            .get(
                &office.sid,
                &format!("/files/{}/file", job.id("file_boltform"))
            )
            .0,
        200
    ); // WHAT: Sid gets it. WHY: pinned for the shop
    let (_, listed) = office.get(&office.fay, &format!("/projects/{}/sets", job.project)); // WHAT: Fay's list of sets. WHY: must agree with the gate
    let mut seen: Vec<String> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect(); // WHAT: what she is listed. WHY: compared
    seen.sort(); // WHAT: in order. WHY: compared
    let mut want: Vec<String> = [
        "set_a001",
        "set_asi04",
        "set_e101",
        "set_s101",
        "set_s102",
        "set_ticket2",
        "set_ticket3",
    ]
    .iter()
    .map(|n| job.id(n).to_string())
    .collect(); // WHAT: the field's reach in the shared example. WHY: the same rule
    want.sort(); // WHAT: in order. WHY: compared
    assert_eq!(seen, want); // WHAT: exactly those. WHY: nothing else is listed
    let (_, folders) = office.get(&office.fay, &format!("/projects/{}/folders", job.project)); // WHAT: her folder list. WHY: the tablet has no folder tree
    assert_eq!(folders, json!([])); // WHAT: none. WHY: narrowed accounts are never shown the tree
    assert_eq!(
        office
            .get(&office.nan, &format!("/sets/{}", job.id("set_s101")))
            .0,
        404
    ); // WHAT: Nan, in neither group. WHY: reaches nothing of a job with a manifest
    let (_, jobs) = office.get(&office.nan, "/tablet"); // WHAT: her tablet's jobs. WHY: none
    assert_eq!(jobs, json!([]));
}

#[test]
fn a_manifest_only_narrows_the_folder_rules_never_widens_them() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    let mut m = steel(&job); // WHAT: the example. WHY: changed to name the contract
    m["sheets"][0]["set"] = json!(job.contract); // WHAT: the office's contract named as a released sheet. WHY: a manifest trying to widen
    m["documents"][1]["id"] = json!(job.contract); // WHAT: and pinned for the field. WHY: likewise
    let (status, said) = office.put(&office.sync, &job.project, &m); // WHAT: kept: it is this job's and current. WHY: the server can't know it is wrong, so the folder rule must hold
    assert_eq!(status, 200, "{said}");
    assert_eq!(
        office
            .get(&office.fay, &format!("/sets/{}", job.contract))
            .0,
        404
    ); // WHAT: still not Fay's. WHY: the folder rule holds underneath
    assert_eq!(
        office
            .get(&office.fay, &format!("/sets/{}/file", job.contract))
            .0,
        404
    ); // WHAT: nor its bytes. WHY: likewise
    let (_, mine) = office.get(&office.fay, &format!("/projects/{}/tablet", job.project)); // WHAT: her tablet's manifest. WHY: must not name it either
    assert!(!mine.to_string().contains(&job.contract), "{mine}"); // WHAT: never named. WHY: the tablet lists only what it can open
                                                                  // A sheet released, then moved out of use in View before Fabwire's next pass: refused at once.
    office.post(
        &office.admin,
        &format!("/sets/{}/folder", job.id("set_s101")),
        json!({ "folder": "10 SHOP DRAWINGS/`OUT OF USE (OFFICE ONLY)" }),
    ); // WHAT: moved out of use. WHY: a stale manifest still names it
    assert_eq!(
        office
            .get(&office.sid, &format!("/sets/{}", job.id("set_s101")))
            .0,
        404
    ); // WHAT: refused. WHY: an out-of-use sheet is never a tablet's
}

#[test]
fn the_receiver_signs_the_tickets_own_line_and_the_account_that_collected_it_is_kept() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    let ticket = job.id("set_ticket2").to_string(); // WHAT: load 2's ticket. WHY: signed on site
    let manifest = steel(&job); // WHAT: the example. WHY: load 2's received_by is open
    let reference = manifest["loads"][0]["boxes"][2]["ref"]
        .as_str()
        .unwrap()
        .to_string(); // WHAT: RECEIVED BY's reference. WHY: Fabwire asks a line with it
    office.post(
        &office.sync,
        &format!("/sets/{ticket}/signoffs"),
        json!([{ "label": "Load #2 · Received by", "group": "Load #2", "ref": reference }]),
    ); // WHAT: Fabwire asks for the line, as today (Decision 0404). WHY: the existing machinery
    assert_eq!(office.put(&office.sync, &job.project, &manifest).0, 200); // WHAT: the manifest. WHY: names the line's reference
    let (status, mine) = office.get(&office.fay, &format!("/projects/{}/tablet", job.project)); // WHAT: Fay's tablet. WHY: the field signs on site
    assert_eq!(status, 200, "{mine}");
    assert_eq!(mine["field"], true); // WHAT: shown to her as the field. WHY: her group
    assert_eq!(mine["manifest"]["sections"]["shop"], json!([])); // WHAT: the shop's sections cut. WHY: not hers
    let line = mine["manifest"]["loads"][0]["boxes"][2]["line"].clone(); // WHAT: the live line on her ticket. WHY: what she signs
    assert_eq!(line["status"], "open", "{mine}");
    let signed = office.post(&office.fay, &format!("/signoffs/{}/sign", line["id"].as_str().unwrap()), json!({ "name": "Dana Whitfield", "strokes": [[[20.0, 100.0], [80.0, 30.0], [140.0, 100.0]]] })); // WHAT: the GC's receiver signs on Fay's tablet, typing their own name. WHY: the name is free text
    assert_eq!(signed["signer"], "Dana Whitfield"); // WHAT: the name written. WHY: printed in RECEIVED BY
    assert_eq!(signed["signed_by"], FAY); // WHAT: the account that collected it. WHY: kept as today
    let (_, after) = office.get(&office.fay, &format!("/projects/{}/tablet", job.project)); // WHAT: her tablet again. WHY: shows signed before Fabwire's next pass
    assert_eq!(
        after["manifest"]["loads"][0]["boxes"][2]["line"]["status"],
        "signed"
    );
    let (_, changes) = office.get(&office.sync, "/signoffs?since=0"); // WHAT: Fabwire reads it back. WHY: writes RECEIVED BY and Delivered (Decision 0410)
    assert!(
        changes["signoffs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["ref"] == reference.as_str() && l["status"] == "signed"),
        "{changes}"
    ); // WHAT: there. WHY: the write-back is unchanged
    let (_, jobs) = office.get(&office.fay, "/tablet"); // WHAT: her job list. WHY: the card
    assert_eq!(jobs[0]["number"], "2601"); // WHAT: the job. WHY: assigned to her
    assert_eq!(jobs[0]["todo"][0], "Load 2 is on site: sign the ticket"); // WHAT: what needs doing. WHY: the card's words
    let mut off = manifest.clone(); // WHAT: the field's Shipping turned off. WHY: then the ticket isn't hers
    off["sections"]["field"] = json!(["drawings", "documents"]);
    assert_eq!(office.put(&office.sync, &job.project, &off).0, 200);
    assert_eq!(
        office
            .get(&office.fay, &format!("/sets/{ticket}/signoffs"))
            .0,
        404
    ); // WHAT: the ticket gone for her. WHY: a section turned off is enforced too
}

#[test]
fn a_sync_key_made_for_another_program_never_sets_a_manifest() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: filed under Fabwire's reference, which any sync key reaches.
    let other = office.post(
        &office.admin,
        "/me/keys",
        json!({ "purpose": "sync", "name": "LatCon" }),
    )["key"]
        .as_str()
        .unwrap()
        .to_string(); // WHAT: an administrator's sync key for another program. WHY: any administrator can make one.
    let (status, said) = office.put(&other, &job.project, &steel(&job)); // WHAT: it tries. WHY: only Fabwire's own key may.
    assert_eq!(
        (status, said["error"].as_str()),
        (403, Some("program_only")),
        "{said}"
    ); // WHAT: refused with the program-only word. WHY: the review's finding, closed.
    assert_eq!(office.audited("changed what a job's tablets show"), 0); // WHAT: nothing written. WHY: nothing changed.
    assert_eq!(office.put(&office.sync, &job.project, &steel(&job)).0, 200); // WHAT: Fabwire's key still sets it. WHY: only the other program was shut out.
}

#[test]
fn a_manifest_listing_the_same_item_again_keeps_it_once() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    let mut flood = steel(&job); // WHAT: the example. WHY: its first sheet repeated below.
    let one = flood["sheets"][0].clone(); // WHAT: the first sheet. WHY: repeated.
    flood["sheets"] = json!(vec![one; 5000]); // WHAT: the same sheet five thousand times. WHY: the review's probe.
    let doc = steel(&job)["documents"][0].clone(); // WHAT: a pinned document. WHY: listed twice.
    flood["documents"] = json!([doc.clone(), doc]);
    let (status, said) = office.put(&office.sync, &job.project, &flood); // WHAT: sent. WHY: kept once, not refused.
    assert_eq!(status, 200, "{said}");
    let (_, kept) = office.get(&office.sync, &format!("/projects/{}/tablet", job.project)); // WHAT: read back. WHY: what was stored.
    assert_eq!(kept["manifest"]["sheets"].as_array().unwrap().len(), 1); // WHAT: one sheet. WHY: repeats dropped on the way in.
    assert_eq!(kept["manifest"]["documents"].as_array().unwrap().len(), 1); // WHAT: one document. WHY: likewise.
}

#[test]
fn a_ticket_box_is_signed_only_by_the_accounts_its_kind_allows() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: its tickets are in 08 SHIPPING DOCS, which Shop and Field see.
    let ask = |ticket: &str, with_role: bool| {
        let line = |words: &str, role: &str| {
            let mut one = json!({ "label": format!("Load #2 \u{b7} {words}"), "group": "Load #2", "ref": format!("ship:{role}") });
            if with_role {
                one["role"] = json!(role); // WHAT: the box's kind, as Fabwire sends it. WHY: what the server holds signing to.
            }
            one
        };
        office.post(
            &office.sync,
            &format!("/sets/{ticket}/signoffs"),
            json!([line("Loaded by", "loaded_by"), line("Driver", "driver"), line("Received by", "received_by")]),
        ) // WHAT: the three lines asked for. WHY: as Fabwire asks a ticket's.
    };
    let ids = |lines: &Value| -> Vec<String> {
        ["loaded_by", "driver", "received_by"]
            .iter()
            .map(|r| {
                lines.as_array().unwrap().iter().find(|l| l["ref"] == format!("ship:{r}").as_str()).unwrap()["id"].as_str().unwrap().to_string()
            })
            .collect() // WHAT: each line's id, in the box order. WHY: signed by id
    };
    let pad = json!({ "name": "Someone", "strokes": [[[20.0, 100.0], [80.0, 30.0], [140.0, 100.0]]] }); // WHAT: a signature. WHY: what each tries to sign with.
    let sign = |who: &str, id: &str| office.ask(who, "POST", &format!("/signoffs/{id}/sign"), Some(pad.clone())).0;
    let bad = office.ask(&office.sync, "POST", &format!("/sets/{}/signoffs", job.id("set_ticket2")), Some(json!([{ "label": "x", "ref": "x", "role": "boss" }]))); // WHAT: a role that is no box. WHY: refused whole.
    assert_eq!(bad.0, 400, "{}", bad.1);
    let two = ids(&ask(job.id("set_ticket2"), true)); // WHAT: load 2's lines, with kinds. WHY: the shop and the field try them.
    assert_eq!(sign(&office.sid, &two[1]), 403); // WHAT: the shop signs Driver. WHY: field or office only.
    assert_eq!(sign(&office.sid, &two[2]), 403); // WHAT: the shop signs Received by. WHY: field or office only.
    assert_eq!(sign(&office.sid, &two[0]), 200); // WHAT: the shop signs Loaded by. WHY: the shop may.
    assert_eq!(sign(&office.fay, &two[1]), 200); // WHAT: the field signs Driver. WHY: the field may.
    assert_eq!(sign(&office.fay, &two[2]), 200); // WHAT: the field signs Received by. WHY: the field may.
    let three = ids(&ask(job.id("set_ticket3"), true)); // WHAT: load 3's lines. WHY: the office tries them.
    for id in &three {
        assert_eq!(sign(&office.eve, id), 200); // WHAT: the office signs every box. WHY: it may.
    }
    let four = ids(&ask(job.id("set_loadlog"), false)); // WHAT: lines asked with no kind. WHY: asked before kinds existed.
    assert_eq!(office.get(&office.sync, &format!("/sets/{}/signoffs", job.id("set_loadlog"))).1[0].get("role"), None); // WHAT: none says a kind. WHY: nothing was sent.
    let again = ids(&ask(job.id("set_loadlog"), true)); // WHAT: the same lines asked again with kinds. WHY: a kind is given to a line that had none.
    assert_eq!(four, again); // WHAT: the same lines. WHY: asking again makes no doubles.
    assert_eq!(sign(&office.sid, &again[2]), 403); // WHAT: now held to its kind. WHY: the rule reaches lines asked before it.
}

#[test]
fn a_job_pushed_empty_drops_off_every_tablet() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    assert_eq!(office.put(&office.sync, &job.project, &steel(&job)).0, 200); // WHAT: the live manifest. WHY: Fay has the job on her tablet.
    assert_eq!(office.get(&office.fay, "/tablet").1.as_array().unwrap().len(), 1); // WHAT: one job on her list. WHY: before it leaves active.
    let mut empty = steel(&job); // WHAT: the example emptied. WHY: what Fabwire pushes when a job completes, goes on hold or is cancelled.
    empty["sections"] = json!({ "shop": [], "field": [] });
    for part in ["sheets", "documents", "loads", "todo"] {
        empty[part] = json!([]);
    }
    empty["traveler"] = Value::Null;
    empty["loading"] = Value::Null;
    assert_eq!(office.put(&office.sync, &job.project, &empty).0, 200); // WHAT: pushed. WHY: the same route, nothing in it.
    assert_eq!(office.get(&office.fay, "/tablet").1.as_array().unwrap().len(), 0); // WHAT: her list is empty. WHY: the job dropped off.
    assert_eq!(office.get(&office.eve, "/tablet").1.as_array().unwrap().len(), 0); // WHAT: the office's too. WHY: nothing for any tablet.
    assert_eq!(office.get(&office.fay, &format!("/sets/{}", job.id("set_s101"))).0, 404); // WHAT: its sheet gone for her. WHY: the empty manifest reaches nothing.
}
