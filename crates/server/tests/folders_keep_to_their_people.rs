//! Who sees which folder, run for real over sockets.
//!
//! One job, three folders: `01 CONTRACT` for the office only, `10 SHOP
//! DRAWINGS` for the Shop and Field groups, and `07 CONTRACT DRAWINGS` for
//! everybody. Fay is a viewer in Field; Nan is a viewer in no group. For
//! every way the server can hand out what is in a folder, Fay must get the
//! field's folder and must not get the office's, and what she is told about
//! the office's must be word for word what she would be told about a folder
//! that isn't there.

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
        let path = std::env::temp_dir().join(format!("hyperview-folders-{n:016x}")); // WHAT: the folder. WHY: under the system's temporary folder.
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

#[test]
fn the_folder_list_shows_a_field_hand_the_fields_folders_and_never_names_the_offices() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    let path = format!("/projects/{}/folders", job.project); // WHAT: the folder list. WHY: the surface.
    let (status, fay) = office.get(&office.fay, &path); // WHAT: as Fay. WHY: Field.
    assert_eq!(status, 200); // WHAT: answered. WHY: she is on the job.
    let seen = names(&fay, "folder"); // WHAT: the folders she sees. WHY: checked.
    assert!(
        seen.contains(&"10 SHOP DRAWINGS".to_string())
            && seen.contains(&"10 SHOP DRAWINGS/10.4 FOR FABRICATION".to_string()),
        "{seen:?}"
    ); // WHAT: the field's, and the folder inside it. WHY: given to Field.
    assert!(
        seen.contains(&"07 CONTRACT DRAWINGS".to_string()),
        "{seen:?}"
    ); // WHAT: the folder with no rule. WHY: everybody's, as before.
    assert!(!fay.to_string().contains("01 CONTRACT\""), "{fay}"); // WHAT: the office's folder is never named. WHY: not even its name leaks.
    let (_, nan) = office.get(&office.nan, &path); // WHAT: as Nan, in no group. WHY: sees only what everyone sees.
    assert_eq!(names(&nan, "folder"), vec!["07 CONTRACT DRAWINGS"]); // WHAT: only the open folder. WHY: neither restricted one is hers.
    let (_, admin) = office.get(&office.admin, &path); // WHAT: as Sam. WHY: the office sees everything.
    let contract = admin
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["folder"] == "01 CONTRACT")
        .expect("the office sees its own folder"); // WHAT: the office's folder. WHY: listed for the office.
    assert_eq!(contract["see"], "office"); // WHAT: its rule. WHY: shown in the Studio.
    assert_eq!(contract["set_in"], "view"); // WHAT: set in the Studio. WHY: an administrator set it, not the program.
    assert_eq!(contract["set_by"], "Sam"); // WHAT: who. WHY: recorded.
    let inside = admin
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["folder"] == "10 SHOP DRAWINGS/10.4 FOR FABRICATION")
        .unwrap(); // WHAT: the folder inside the shop's. WHY: it takes its parent's rule.
    assert_eq!(inside["see"], json!(["Field", "Shop"])); // WHAT: its parent's groups. WHY: a subfolder inherits.
    assert!(inside.get("own").is_none(), "{inside}"); // WHAT: no rule of its own. WHY: shown as inherited.
    let (_, eve) = office.get(&office.eve, &path); // WHAT: as Eve, an estimator. WHY: the office.
    assert!(names(&eve, "folder").contains(&"01 CONTRACT".to_string())); // WHAT: sees it. WHY: estimators are the office.
}

#[test]
fn the_set_list_the_file_list_and_the_counts_hold_only_what_a_field_hand_may_see() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    let (_, sets) = office.get(&office.fay, &format!("/projects/{}/sets", job.project)); // WHAT: the sets, as Fay. WHY: the surface.
    let mut seen = names(&sets, "name"); // WHAT: their names. WHY: checked.
    seen.sort(); // WHAT: in order. WHY: compared.
    assert_eq!(seen, vec!["A-101", "S-101"], "{sets}"); // WHAT: the field's and the open one. WHY: never the contract.
    let (_, files) = office.get(&office.fay, &format!("/projects/{}/files", job.project)); // WHAT: the files, as Fay. WHY: the surface.
    assert_eq!(names(&files, "name"), vec!["Cut list.xlsx"], "{files}"); // WHAT: the cut list only. WHY: never the estimate.
    let (_, all_files) = office.get(
        &office.fay,
        &format!("/projects/{}/files?all=1", job.project),
    ); // WHAT: with history. WHY: history is no way round.
    assert_eq!(names(&all_files, "name"), vec!["Cut list.xlsx"]); // WHAT: the same. WHY: likewise.
    let (_, one) = office.get(&office.fay, &format!("/projects/{}", job.project)); // WHAT: the project, as Fay. WHY: its count.
    assert_eq!(one["sets"], 2, "{one}"); // WHAT: two sets. WHY: a hidden folder's set isn't even counted.
    let (_, listed) = office.get(&office.fay, "/projects"); // WHAT: the list, as Fay. WHY: its count too.
    assert_eq!(listed[0]["sets"], 2, "{listed}"); // WHAT: two. WHY: likewise.
    let (_, admin) = office.get(&office.admin, &format!("/projects/{}", job.project)); // WHAT: as Sam. WHY: the office counts all.
    assert_eq!(admin["sets"], 3); // WHAT: three. WHY: unchanged for the office.
    let (_, sid) = office.get(&office.sid, &format!("/projects/{}/sets", job.project)); // WHAT: as Sid, Shop. WHY: the other group named.
    assert!(names(&sid, "name").contains(&"S-101".to_string())); // WHAT: sees the shop drawing. WHY: given to Shop.
    let (_, nan) = office.get(&office.nan, &format!("/projects/{}/sets", job.project)); // WHAT: as Nan. WHY: no group.
    assert_eq!(names(&nan, "name"), vec!["A-101"]); // WHAT: only the open one. WHY: neither group's.
}

#[test]
fn a_drawing_or_file_in_the_offices_folder_is_answered_as_one_that_isnt_there() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    let hidden = office.bytes(&office.fay, &format!("/sets/{}/file", job.contract)); // WHAT: the contract's PDF, as Fay. WHY: the download.
    let missing = office.bytes(&office.fay, "/sets/set_0000000000000000000000/file"); // WHAT: a set that doesn't exist. WHY: the answer to compare with.
    assert_eq!(hidden, missing); // WHAT: word for word the same. WHY: nobody learns a hidden drawing exists.
    assert_eq!(hidden.0, 404); // WHAT: not found. WHY: as documented.
    for route in [
        "",
        "/sheets",
        "/markups",
        "/takeoff",
        "/takeoff.csv",
        "/markuplist",
    ] {
        let (status, said) = office.get(&office.fay, &format!("/sets/{}{route}", job.contract)); // WHAT: every read of the set: its record, sheets (what a page is drawn from), markups, takeoff, export and markup list. WHY: each could leak it.
        let (_, nothing) = office.get(
            &office.fay,
            &format!("/sets/set_0000000000000000000000{route}"),
        ); // WHAT: the same of a set that isn't there. WHY: compared.
        assert_eq!((status, &said), (404, &nothing), "/sets/{{id}}{route}"); // WHAT: the same refusal. WHY: indistinguishable.
    }
    let (status, _) = office.ask(
        &office.fay,
        "POST",
        &format!("/sets/{}/copy", job.contract),
        Some(json!({})),
    ); // WHAT: a copy of it. WHY: a copy would be a way round.
    assert!(status == 403 || status == 404, "{status}"); // WHAT: refused. WHY: a viewer may not write, and the set isn't hers anyway.
    let (status, bytes) = office.bytes(&office.fay, &format!("/sets/{}/file", job.shop)); // WHAT: the shop drawing, as Fay. WHY: hers.
    assert_eq!(status, 200); // WHAT: downloaded. WHY: given to Field.
    assert!(bytes.starts_with(b"%PDF")); // WHAT: the drawing. WHY: the real file.
    assert_eq!(
        office
            .get(&office.fay, &format!("/sets/{}/sheets", job.shop))
            .0,
        200
    ); // WHAT: its sheets. WHY: hers.
    let file_hidden = office.bytes(&office.fay, &format!("/files/{}/file", job.estimate)); // WHAT: the estimate, as Fay. WHY: the office's file.
    let file_missing = office.bytes(&office.fay, "/files/file_000000000000000000000000/file"); // WHAT: a file that isn't there. WHY: compared.
    assert_eq!(file_hidden, file_missing); // WHAT: the same. WHY: indistinguishable.
    assert_eq!(file_hidden.0, 404); // WHAT: not found. WHY: as documented.
    assert_eq!(
        office.bytes(&office.fay, &format!("/files/{}/file", job.cut_list)),
        (200, b"PK the cut list".to_vec())
    ); // WHAT: the cut list, whole. WHY: hers.
    assert_eq!(
        office
            .bytes(&office.admin, &format!("/files/{}/file", job.estimate))
            .0,
        200
    ); // WHAT: the estimate, as Sam. WHY: the office sees everything.
}

#[test]
fn the_projects_markup_list_and_its_stamp_leave_the_offices_folder_out_for_a_field_hand() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    let pull = |token: &str| {
        office
            .get(
                token,
                &format!("/projects/{}/markuplist?all=1", job.project),
            )
            .1
    }; // WHAT: the whole job's markups, superseded ones too. WHY: the export a program reads.
    let fay = pull(&office.fay); // WHAT: as Fay. WHY: under test.
    assert!(!fay.to_string().contains("SECRET CONTRACT BEAM"), "{fay}"); // WHAT: the contract's beam is absent. WHY: a summary must not carry a hidden folder's takeoff.
    assert!(!fay.to_string().contains(&job.contract), "{fay}"); // WHAT: nor its id. WHY: nothing about it.
    assert!(fay.to_string().contains("W12x26"), "{fay}"); // WHAT: the shop's beam is there. WHY: hers.
    assert!(pull(&office.admin)
        .to_string()
        .contains("SECRET CONTRACT BEAM")); // WHAT: the office sees it. WHY: unchanged for the office.
    let stamp = |token: &str| {
        office
            .get(token, &format!("/projects/{}/stamp", job.project))
            .1
    }; // WHAT: the change stamp. WHY: a change notice a seat or program polls.
    let (fay_before, admin_before) = (stamp(&office.fay), stamp(&office.admin)); // WHAT: both stamps now. WHY: compared after a change.
    assert_eq!(fay_before["markups"], 1); // WHAT: one markup counted for Fay. WHY: the contract's isn't hers.
    office.post(
        &office.admin,
        &format!("/sets/{}/markups", job.contract),
        a_beam("ANOTHER SECRET"),
    ); // WHAT: a change in the office's folder. WHY: must not show as a change to Fay.
    assert_eq!(stamp(&office.fay), fay_before); // WHAT: Fay's stamp unchanged. WHY: a hidden folder's changes aren't changes for her.
    assert_ne!(stamp(&office.admin)["stamp"], admin_before["stamp"]); // WHAT: Sam's moved. WHY: he sees it.
    office.post(
        &office.admin,
        &format!("/sets/{}/markups", job.shop),
        a_beam("W12x26"),
    ); // WHAT: a change in the field's folder. WHY: is a change for Fay.
    assert_ne!(stamp(&office.fay)["stamp"], fay_before["stamp"]); // WHAT: Fay's moved. WHY: hers.
}

#[test]
fn an_assistant_working_for_a_field_hand_sees_what_she_sees_and_no_more() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    let key = office.key(&office.fay, "assistant"); // WHAT: Fay's assistant key. WHY: the assistant acts as her.
    let sets = office.tool(&key, "list_sets", json!({ "project": "26-500" })); // WHAT: the job's sets, found by its number. WHY: the assistant's search.
    assert!(
        sets.contains("S-101") && !sets.contains("Contract"),
        "{sets}"
    ); // WHAT: the shop's, not the contract. WHY: as the API's list.
    let every = office.tool(&key, "list_sets", json!({})); // WHAT: every set on the server. WHY: the widest search.
    assert!(!every.contains("Contract"), "{every}"); // WHAT: still not the contract. WHY: likewise.
    let jobs = office.tool(&key, "list_projects", json!({})); // WHAT: the jobs. WHY: their counts.
    assert!(jobs.contains("2 current drawing set(s)"), "{jobs}"); // WHAT: two, not three. WHY: a hidden folder is not counted.
    for tool in ["takeoff", "sheets", "shop_list", "counted_twice"] {
        let hidden = office.tool(&key, tool, json!({ "set": job.contract })); // WHAT: a tool on the contract. WHY: each reads the set.
        assert!(
            hidden.contains("no drawing set called") && !hidden.contains("SECRET"),
            "{tool}: {hidden}"
        ); // WHAT: answered as not there. WHY: as a missing set is.
    }
    let compared = office.tool(
        &key,
        "revision_cost",
        json!({ "was_bid": job.shop, "arrived": job.contract }),
    ); // WHAT: a comparison against the contract. WHY: a way round through a second set.
    assert!(compared.contains("no drawing set called"), "{compared}"); // WHAT: refused. WHY: both sides are gated.
    let takeoff = office.tool(&key, "takeoff", json!({ "set": job.shop })); // WHAT: the shop drawing's takeoff. WHY: hers.
    assert!(takeoff.contains("W12x26"), "{takeoff}"); // WHAT: its beam. WHY: given to Field.
}

#[test]
fn taking_somebody_out_of_a_group_hides_its_folders_at_once_and_putting_them_back_shows_them() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    let (_, groups) = office.get(&office.admin, "/groups"); // WHAT: the groups. WHY: Field's id.
    let field = groups
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["name"] == "Field")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string(); // WHAT: Field. WHY: Fay is taken out of it.
    let shop_file = format!("/sets/{}/file", job.shop); // WHAT: the shop drawing. WHY: hers while she is in Field.
    assert_eq!(office.bytes(&office.fay, &shop_file).0, 200); // WHAT: hers now. WHY: in Field.
    office.post(
        &office.admin,
        &format!("/groups/{field}/people"),
        json!({ "person": FAY, "on": false }),
    ); // WHAT: Fay out of Field. WHY: she moved to the office's phones, say.
    assert_eq!(office.bytes(&office.fay, &shop_file).0, 404); // WHAT: gone on her very next request. WHY: read fresh every time.
    office.post(
        &office.admin,
        &format!("/groups/{field}/people"),
        json!({ "person": FAY, "on": true }),
    ); // WHAT: back in. WHY: and it comes back.
    assert_eq!(office.bytes(&office.fay, &shop_file).0, 200); // WHAT: hers again. WHY: likewise.
}

#[test]
fn a_folder_with_no_rule_is_everybodys_as_before_and_a_program_s_new_folders_are_the_offices() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let admin = &office.admin; // WHAT: Sam. WHY: sets up.
    let project = office.post(
        admin,
        "/projects",
        json!({ "number": "26-600", "name": "Mirror", "reference": "program:job:600" }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string(); // WHAT: a job a program keeps in step. WHY: its sync key may reach it.
    let old = office.drawing(admin, &project, "Old", "02 OLD", "old"); // WHAT: a set in a folder nobody gave a rule. WHY: how every office is today.
    assert_eq!(
        office.bytes(&office.nan, &format!("/sets/{old}/file")).0,
        200
    ); // WHAT: Nan sees it. WHY: existing offices keep working.
    office.post(
        admin,
        &format!("/projects/{project}/folders"),
        json!({ "folders": ["03 BY HAND"] }),
    ); // WHAT: an administrator's folder, no rule. WHY: takes the default.
    let sync = office.key(admin, "sync"); // WHAT: the program's sync key. WHY: the mirror.
    let made = office.post(&sync, &format!("/projects/{project}/folders"), json!({ "folders": ["04 MIRRORED", "04 MIRRORED/04.1 INSIDE", { "folder": "05 FOR ALL", "see": "everyone" }] })); // WHAT: the program's folders, two without a rule. WHY: safe by default.
    assert_eq!(made["made"], 3, "{made}"); // WHAT: three new folders. WHY: the mirrored one, the one inside it and the open one.
    let (_, nan) = office.get(&office.nan, &format!("/projects/{project}/folders")); // WHAT: as Nan. WHY: what a viewer sees.
    let seen = names(&nan, "folder"); // WHAT: her folders. WHY: checked.
    assert!(
        seen.contains(&"03 BY HAND".to_string())
            && seen.contains(&"05 FOR ALL".to_string())
            && seen.contains(&"02 OLD".to_string()),
        "{seen:?}"
    ); // WHAT: the hand-made, the open and the old. WHY: everybody's.
    assert!(!seen.iter().any(|f| f.starts_with("04")), "{seen:?}"); // WHAT: never the mirrored ones. WHY: a program's new folder is the office's until it says.
    let (_, admin_view) = office.get(admin, &format!("/projects/{project}/folders")); // WHAT: as Sam. WHY: the rules as set.
    let mirrored = admin_view
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["folder"] == "04 MIRRORED")
        .unwrap(); // WHAT: the program's folder. WHY: its rule.
    assert_eq!(
        (mirrored["see"].clone(), mirrored["set_in"].clone()),
        (json!("office"), json!("program"))
    ); // WHAT: the office's, set by the program. WHY: safe by default, and said where.
    let inside = admin_view
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["folder"] == "04 MIRRORED/04.1 INSIDE")
        .unwrap(); // WHAT: inside it. WHY: takes its parent's.
    assert_eq!(
        (inside["see"].clone(), inside.get("own").cloned()),
        (json!("office"), None)
    ); // WHAT: the office's, inherited. WHY: never its own default.
}

#[test]
fn a_drawing_a_program_sends_into_a_new_folder_is_the_offices_before_any_folder_is_made() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let admin = &office.admin; // WHAT: Sam. WHY: sets up.
    let project = office.post(
        admin,
        "/projects",
        json!({ "number": "26-610", "name": "Mirror first", "reference": "program:job:610" }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string(); // WHAT: a job a program keeps in step. WHY: its sync key may reach it.
    let sync = office.key(admin, "sync"); // WHAT: the program's sync key. WHY: the mirror.
    let sent = office.drawing(&sync, &project, "Contract", "01 CONTRACT", "sent first"); // WHAT: a drawing sent before its folder was ever made. WHY: the order a mirror may send things in.
    assert_eq!(
        office.bytes(&office.nan, &format!("/sets/{sent}/file")).0,
        404
    ); // WHAT: Nan can't open it. WHY: a program's new folder is the office's until it says, whatever came first.
    let (_, nan) = office.get(&office.nan, &format!("/projects/{project}/sets")); // WHAT: her set list. WHY: it mustn't be listed either.
    assert!(!names(&nan, "name").contains(&"Contract".to_string()), "{nan}"); // WHAT: not there. WHY: hidden looks like missing.
    assert_eq!(
        office.bytes(admin, &format!("/sets/{sent}/file")).0,
        200
    ); // WHAT: Sam still opens it. WHY: the office's, not nobody's.
}

#[test]
fn a_rule_changed_in_the_studio_stays_until_the_program_sets_that_folder_again() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    let path = format!("/projects/{}/folders", job.project); // WHAT: the folders route. WHY: both sides use it.
    let sync = office.key(&office.admin, "sync"); // WHAT: the program's key. WHY: the program's side.
    office.post(
        &sync,
        &path,
        json!({ "folders": ["01 CONTRACT", "10 SHOP DRAWINGS"] }),
    ); // WHAT: the program naming folders again, no rule. WHY: its ordinary pass.
    let rule = |name: &str| {
        let (_, list) = office.get(&office.admin, &path); // WHAT: the folders as Sam sees them. WHY: their rules.
        list.as_array()
            .unwrap()
            .iter()
            .find(|f| f["folder"] == name)
            .cloned()
            .unwrap() // WHAT: the one folder. WHY: checked.
    };
    assert_eq!(rule("10 SHOP DRAWINGS")["set_in"], "view"); // WHAT: Sam's rule kept. WHY: the program didn't set that folder.
    let answer = office.post(
        &sync,
        &path,
        json!({ "folders": [{ "folder": "10 SHOP DRAWINGS", "see": ["Field", "Erectors"] }] }),
    ); // WHAT: the program setting it. WHY: now it is the program's.
    assert_eq!(answer["groups_made"], json!(["Erectors"]), "{answer}"); // WHAT: a group nobody made, made empty. WHY: and said so.
    assert!(answer["notes"].to_string().contains("Erectors"), "{answer}"); // WHAT: the note. WHY: an administrator fills it later.
    let now = rule("10 SHOP DRAWINGS"); // WHAT: the rule now. WHY: checked.
    assert_eq!(
        (now["see"].clone(), now["set_in"].clone()),
        (json!(["Erectors", "Field"]), json!("program"))
    ); // WHAT: the program's rule. WHY: it set the folder.
    assert_eq!(
        office.get(&office.sid, &format!("/sets/{}", job.shop)).0,
        404
    ); // WHAT: Sid, Shop, no longer sees it. WHY: Shop isn't named any more.
    let (_, groups) = office.get(&office.admin, "/groups"); // WHAT: the groups. WHY: Erectors is there, empty.
    let erectors = groups
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["name"] == "Erectors")
        .expect("made"); // WHAT: the new group. WHY: made by the rule.
    assert_eq!(erectors["people"], json!([])); // WHAT: nobody in it. WHY: an administrator puts people in.
}

#[test]
fn a_folder_asked_to_be_wider_than_its_parent_is_not_and_the_answer_says_so() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job. WHY: under test.
    let answer = office.post(
        &office.admin,
        &format!("/projects/{}/folders", job.project),
        json!({ "folders": [{ "folder": "01 CONTRACT/PUBLIC", "see": "everyone" }] }),
    ); // WHAT: everyone, inside the office's folder. WHY: a child can't be wider.
    assert!(
        answer["notes"][0]
            .as_str()
            .unwrap_or_default()
            .contains("never seen by more people"),
        "{answer}"
    ); // WHAT: said. WHY: say so if asked.
    let (status, file) = office.file(
        &office.admin,
        &job.project,
        "01 CONTRACT/PUBLIC",
        "Notice.txt",
        b"public?",
    ); // WHAT: a file put in it. WHY: to see who can reach it.
    assert_eq!(status, 200); // WHAT: taken. WHY: Sam may.
    assert_eq!(
        office
            .bytes(
                &office.fay,
                &format!("/files/{}/file", file["id"].as_str().unwrap())
            )
            .0,
        404
    ); // WHAT: Fay still can't. WHY: its parent is the office's.
}

#[test]
fn groups_are_for_an_administrator_signed_in_and_nobody_else() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let made = office.post(&office.admin, "/groups", json!({ "name": " Field " })); // WHAT: a group. WHY: to ask about.
    let again = office.post(&office.admin, "/groups", json!({ "name": "field" })); // WHAT: the same name, another case. WHY: one group.
    assert_eq!(
        (made["id"].clone(), made["name"].clone()),
        (again["id"].clone(), json!("Field"))
    ); // WHAT: the same group, as first spelled. WHY: Field and field are one.
    let id = made["id"].as_str().unwrap(); // WHAT: its id. WHY: for the member route.
    let assistant = office.key(&office.fay, "assistant"); // WHAT: a key. WHY: no key administers.
    let refused = |status: u16| status == 401 || status == 403; // WHAT: refused as not allowed (a person) or as nobody (an assistant key, which reaches only /mcp). WHY: either way, nothing done.
    for token in [&office.eve, &office.fay, &assistant] {
        assert!(refused(office.get(token, "/groups").0)); // WHAT: refused the list. WHY: only an administrator decides who sees what.
        assert!(refused(
            office
                .ask(token, "POST", "/groups", Some(json!({ "name": "Mine" })))
                .0
        )); // WHAT: refused making one. WHY: likewise.
        assert!(refused(
            office
                .ask(
                    token,
                    "POST",
                    &format!("/groups/{id}/people"),
                    Some(json!({ "person": FAY }))
                )
                .0
        )); // WHAT: refused joining one. WHY: nobody puts themselves in a group.
        assert!(refused(
            office
                .ask(
                    token,
                    "POST",
                    "/projects/x/folders",
                    Some(json!({ "folders": ["A"] }))
                )
                .0
        )); // WHAT: refused setting folders. WHY: likewise.
    }
    assert_eq!(office.get(&office.eve, "/groups").0, 403); // WHAT: an estimator in particular. WHY: the office, but not an administrator.
    assert_eq!(
        office
            .ask(
                &office.admin,
                "POST",
                &format!("/groups/{id}/people"),
                Some(json!({ "person": "usr_nobody" }))
            )
            .0,
        404
    ); // WHAT: a person who isn't. WHY: not put in.
    assert_eq!(
        office
            .ask(
                &office.admin,
                "POST",
                "/groups",
                Some(json!({ "name": "" }))
            )
            .0,
        400
    ); // WHAT: no name. WHY: refused.
}

#[test]
fn a_file_of_any_kind_is_kept_once_and_saved_again_replaces_the_one_before() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let admin = &office.admin; // WHAT: Sam. WHY: puts files up.
    let project = office.post(
        admin,
        "/projects",
        json!({ "number": "26-700", "name": "Files" }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string(); // WHAT: a job. WHY: somewhere to put them.
    let (status, first) = office.file(
        admin,
        &project,
        "01 CONTRACT",
        "Estimate.xlsx",
        b"version one",
    ); // WHAT: a spreadsheet. WHY: not a PDF.
    assert_eq!(status, 200, "{first}"); // WHAT: taken. WHY: any kind of file.
    assert!(first["id"].as_str().unwrap().starts_with("file_")); // WHAT: its id. WHY: as documented.
    assert_eq!(
        (first["bytes"].clone(), first["modified"].clone()),
        (json!(11), json!("2026-10-27T15:04:05.1234567Z"))
    ); // WHAT: its size and time saved. WHY: as sent.
    let (_, twice) = office.file(
        admin,
        &project,
        "Somewhere Else",
        "Copy.xlsx",
        b"version one",
    ); // WHAT: the same bytes again. WHY: one file.
    assert_eq!(
        (twice["id"].clone(), twice["already_here"].clone()),
        (first["id"].clone(), json!(true))
    ); // WHAT: the one there. WHY: never stored twice.
    let (_, second) = office.file(
        admin,
        &project,
        "01 contract",
        " estimate.XLSX",
        b"version two",
    ); // WHAT: saved again, case and spacing aside. WHY: the same file.
    assert_eq!(second["replaces"], first["id"], "{second}"); // WHAT: it replaces the first. WHY: kept as its history.
    assert_eq!(second["folder"], "01 CONTRACT"); // WHAT: the folder as first spelled. WHY: never shown twice.
    let (_, now) = office.get(admin, &format!("/projects/{project}/files")); // WHAT: the current files. WHY: one.
    assert_eq!(now.as_array().unwrap().len(), 1); // WHAT: only the newest. WHY: the old is history.
    let (_, all) = office.get(admin, &format!("/projects/{project}/files?all=1")); // WHAT: with history. WHY: the old kept.
    assert_eq!(all.as_array().unwrap().len(), 2); // WHAT: both. WHY: nothing is thrown away.
    assert_eq!(
        office.bytes(
            admin,
            &format!("/files/{}/file", first["id"].as_str().unwrap())
        ),
        (200, b"version one".to_vec())
    ); // WHAT: the old one's bytes. WHY: history can be read.
    let (status, _) = office.form(
        admin,
        "/files",
        &[("project", &project), ("modified", "last Tuesday")],
        "x.txt",
        b"x",
    ); // WHAT: a time that isn't one. WHY: refused.
    assert_eq!(status, 400); // WHAT: refused. WHY: said rather than guessed.
    let (status, _) = office.form(admin, "/files", &[("project", &project)], "empty.txt", b""); // WHAT: an empty file. WHY: nothing to keep.
    assert_eq!(status, 400); // WHAT: refused. WHY: as /sets.
    let (status, named) = office.form(
        admin,
        "/files",
        &[("project", &project)],
        "..\\..\\evil/../notes.txt",
        b"notes",
    ); // WHAT: a name with folders in it. WHY: a name never says where to write.
    assert_eq!((status, named["name"].clone()), (200, json!("notes.txt"))); // WHAT: the last part only. WHY: as documented.
    let (status, _) = office.form(
        &office.fay,
        "/files",
        &[("project", &project)],
        "mine.txt",
        b"mine",
    ); // WHAT: a viewer putting a file up. WHY: viewers don't write.
    assert_eq!(status, 403); // WHAT: refused. WHY: as for drawings.
}

#[test]
fn a_drawing_moved_takes_its_earlier_issues_and_its_markups_with_it() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let admin = &office.admin; // WHAT: Sam. WHY: sets up.
    let project = office.post(
        admin,
        "/projects",
        json!({ "number": "26-800", "name": "Moves" }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string(); // WHAT: a job. WHY: somewhere to move in.
    let first = office.drawing(admin, &project, "S-200", "10.1 FOR APPROVAL", "first"); // WHAT: the first issue. WHY: history.
    office.post(admin, &format!("/sets/{first}/markups"), a_beam("W12x26")); // WHAT: a markup on it. WHY: must stay.
    let second = office.drawing(admin, &project, "S-200", "10.1 FOR APPROVAL", "second"); // WHAT: the second issue. WHY: supersedes the first.
    let moved = office.post(
        admin,
        &format!("/sets/{second}/folder"),
        json!({ "folder": "10.4 FOR FABRICATION" }),
    ); // WHAT: moved. WHY: the folder renamed on the share.
    assert_eq!(moved["folder"], "10.4 FOR FABRICATION"); // WHAT: there now. WHY: moved.
    assert_eq!(
        office.get(admin, &format!("/sets/{first}")).1["folder"],
        "10.4 FOR FABRICATION"
    ); // WHAT: the first issue too. WHY: history goes with it.
    assert_eq!(
        office.get(admin, &format!("/sets/{first}/markups")).1["markups"]
            .as_array()
            .unwrap()
            .len(),
        1
    ); // WHAT: its markup still there. WHY: untouched.
    let (_, file) = office.file(admin, &project, "A", "Note.txt", b"note"); // WHAT: a file. WHY: files move too.
    let moved = office.post(
        admin,
        &format!("/files/{}/folder", file["id"].as_str().unwrap()),
        json!({ "folder": "B/C" }),
    ); // WHAT: moved. WHY: as a set.
    assert_eq!(moved["folder"], "B/C"); // WHAT: there now. WHY: moved.
    let (status, _) = office.ask(
        admin,
        "POST",
        &format!("/sets/{second}/folder"),
        Some(json!({ "folder": "../up" })),
    ); // WHAT: a folder that climbs. WHY: refused, as everywhere.
    assert_eq!(status, 400); // WHAT: refused. WHY: folders are names, not paths.
}

#[test]
fn a_sync_key_reaches_each_new_route_on_its_own_projects_and_reads_every_folder_of_them() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: a job filed under a program's reference. WHY: the program's own.
    let hand = office.post(
        &office.admin,
        "/projects",
        json!({ "number": "26-900", "name": "By hand" }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string(); // WHAT: a job made by hand. WHY: never the program's.
    let sync = office.key(&office.admin, "sync"); // WHAT: the program's key. WHY: under test.
    assert_eq!(
        office
            .ask(
                &sync,
                "POST",
                &format!("/projects/{}/folders", job.project),
                Some(json!({ "folders": ["11 NEW"] }))
            )
            .0,
        200
    ); // WHAT: POST /projects/:id/folders. WHY: on its list.
    let (status, file) = office.file(&sync, &job.project, "11 NEW", "Model.ifc", b"ISO-10303-21;"); // WHAT: POST /files. WHY: on its list.
    assert_eq!(status, 200, "{file}"); // WHAT: taken. WHY: likewise.
    assert_eq!(
        office
            .ask(
                &sync,
                "POST",
                &format!("/files/{}/folder", file["id"].as_str().unwrap()),
                Some(json!({ "folder": "12 MOVED" }))
            )
            .0,
        200
    ); // WHAT: POST /files/:id/folder. WHY: on its list.
    assert_eq!(
        office
            .ask(
                &sync,
                "POST",
                &format!("/sets/{}/folder", job.open),
                Some(json!({ "folder": "07 CONTRACT DRAWINGS/07.1" }))
            )
            .0,
        200
    ); // WHAT: POST /sets/:id/folder. WHY: on its list.
    let pull = office
        .get(&sync, &format!("/projects/{}/markuplist", job.project))
        .1; // WHAT: the takeoff read back. WHY: the program reads every folder of its own job.
    assert!(pull.to_string().contains("SECRET CONTRACT BEAM"), "{pull}"); // WHAT: the office's folder included. WHY: a sync key acts as the office for its own projects.
    assert_eq!(
        office
            .ask(
                &sync,
                "POST",
                &format!("/projects/{hand}/folders"),
                Some(json!({ "folders": ["A"] }))
            )
            .0,
        404
    ); // WHAT: a hand-made job. WHY: never the program's.
    assert_eq!(office.file(&sync, &hand, "A", "x.txt", b"x").0, 404); // WHAT: a file into it. WHY: likewise.
    assert_eq!(
        office
            .get(&sync, &format!("/projects/{}/folders", job.project))
            .0,
        403
    ); // WHAT: a route not on its list. WHY: refused before it runs.
    assert_eq!(office.get(&sync, "/groups").0, 403); // WHAT: groups. WHY: not a program's business.
}

#[test]
fn every_change_to_who_sees_what_is_written_in_the_audit_log() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let job = a_job(&office); // WHAT: the job, set up through every new route. WHY: each writes a line.
    office.bytes(&office.fay, &format!("/files/{}/file", job.cut_list)); // WHAT: a file downloaded. WHY: a line.
    office.post(
        &office.admin,
        &format!("/files/{}/folder", job.estimate),
        json!({ "folder": "01 CONTRACT/OLD" }),
    ); // WHAT: a file moved. WHY: a line.
    let (_, log) = office.get(&office.admin, "/audit"); // WHAT: the log. WHY: checked.
    let actions: Vec<String> = names(&log, "action"); // WHAT: what happened. WHY: checked.
    for action in [
        "made a group of people",
        "changed who is in a group",
        "changed who sees a folder",
        "put a file in a folder",
        "downloaded a file",
        "moved to another folder",
    ] {
        assert!(
            actions.iter().any(|a| a == action),
            "no {action:?} in {actions:?}"
        ); // WHAT: each written. WHY: an auditor asks who opened what to whom.
    }
}

#[test]
fn a_long_request_writes_every_rule_change_and_every_group_it_made_in_the_audit_log() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let admin = &office.admin; // WHAT: Sam. WHY: sets up.
    let project = office.post(
        admin,
        "/projects",
        json!({ "number": "26-620", "name": "Long list", "reference": "program:job:620" }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string(); // WHAT: a job a program keeps in step. WHY: its sync key may reach it.
    let sync = office.key(admin, "sync"); // WHAT: the program's sync key. WHY: it sends the long request.
    let mut folders: Vec<Value> = (1..=20)
        .map(|n| json!({ "folder": format!("{n:02} HARMLESS"), "see": "everyone" }))
        .collect(); // WHAT: twenty harmless changes. WHY: they used to fill the one audit line.
    folders.push(json!({ "folder": "21 CONTRACT", "see": ["Crew"] })); // WHAT: the twenty-first, given to a group nobody made. WHY: the one that must not be hidden.
    office.post(&sync, &format!("/projects/{project}/folders"), json!({ "folders": folders })); // WHAT: sent at once. WHY: one request.
    let (_, log) = office.get(admin, "/audit"); // WHAT: the log. WHY: checked.
    let text = log.to_string(); // WHAT: all of it as text. WHY: searched.
    assert!(text.contains("21 CONTRACT"), "{text}"); // WHAT: the twenty-first change is there. WHY: nothing past twenty is dropped.
    assert!(text.contains("of 21"), "{text}"); // WHAT: the total said. WHY: anybody reading knows how many there were.
    let made: Vec<&Value> = log
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["action"] == "made a group of people")
        .collect(); // WHAT: the group lines. WHY: checked.
    assert!(made.iter().any(|e| e.to_string().contains("Crew")), "{text}"); // WHAT: the group the request made. WHY: groups decide who sees folders.
}
