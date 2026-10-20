//! Keys, and what each one may reach, run for real over sockets.
//!
//! Two promises are held here. An assistant key sees exactly what the person
//! who made it may see, through `/mcp` as through the API. And a key never
//! runs the server: no key administers, whoever made it, and a `sync` key
//! reaches its own short list of routes and nothing else.

use std::sync::Arc; // WHAT: shared ownership of the server. WHY: the router takes it as an Arc.

use hyperview_server::api::{self, Server}; // WHAT: the router and the server state. WHY: the tests run the real thing.
use hyperview_server::{auth, Config, Store}; // WHAT: password hashing, configuration and storage. WHY: to set up an office with people in it.
use rusqlite::params; // WHAT: bound SQL parameters. WHY: people are put in the database directly, as the other wire tests do.
use serde_json::{json, Value}; // WHAT: JSON values. WHY: requests and answers.

/// A folder that is gone when the test is.
struct Scratch(std::path::PathBuf); // WHAT: the server's data folder. WHY: each test gets its own, so none sees another's.

impl Scratch {
    fn new() -> Scratch {
        let n: u64 = rand::random(); // WHAT: a random number. WHY: a folder name no other test is using.
        let path = std::env::temp_dir().join(format!("hyperview-keys-{n:016x}")); // WHAT: the folder. WHY: under the system's temporary folder.
        std::fs::create_dir_all(&path).expect("a scratch directory"); // WHAT: made. WHY: the store writes into it.
        Scratch(path) // WHAT: handed back. WHY: dropped at the end of the test.
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0); // WHAT: the folder removed. WHY: tests leave nothing behind.
    }
}

/// A running office: its address, its folder, and the runtime serving it.
struct Office {
    base: String, // WHAT: http://127.0.0.1:port. WHY: every request goes there.
    scratch: Scratch, // WHAT: its data folder. WHY: kept alive while it runs, and read by some tests.
    _runtime: tokio::runtime::Runtime, // WHAT: the async runtime. WHY: dropping it stops the server.
}

const PASSWORD: &str = "brace-gusset-purlin-shim-42"; // WHAT: everybody's password here. WHY: long enough for the server's rules.
const ADMIN: &str = "usr_admin"; // WHAT: the administrator's id. WHY: named, so a project can be put in their name.
const VIEWER: &str = "usr_viewer"; // WHAT: a viewer's id. WHY: the person kept off a job.
const ESTIMATOR: &str = "usr_estimator"; // WHAT: an estimator's id. WHY: the person named on a job.

/// An office with an administrator, an estimator and a viewer.
fn an_office() -> Office {
    let scratch = Scratch::new(); // WHAT: a fresh folder. WHY: a fresh office.
    let config = Config {
        name: "Example Steel".into(), // WHAT: the office's name. WHY: any name will do.
        data: scratch.0.clone(), // WHAT: where it keeps everything. WHY: the scratch folder.
        listen: "127.0.0.1:0".parse().unwrap(), // WHAT: any free port on this computer. WHY: tests run side by side.
        ..Config::default() // WHAT: everything else as shipped. WHY: the server as an office runs it.
    };
    let store = Store::open(&config.database(), &config.blobs()).expect("a store"); // WHAT: the database. WHY: the people go in it.
    let hashed = auth::hash_password(PASSWORD).unwrap(); // WHAT: the password, hashed once. WHY: Argon2 is slow on purpose; three people share it.
    store
        .with(|db| {
            for (id, name, email, role) in [
                (ADMIN, "Sam", "sam@example.com", "admin"), // WHAT: the administrator. WHY: makes the keys.
                (ESTIMATOR, "Eve", "eve@example.com", "estimator"), // WHAT: an estimator. WHY: named on the private job.
                (VIEWER, "Val", "val@example.com", "viewer"), // WHAT: a viewer. WHY: not named on it.
            ] {
                db.execute(
                    "INSERT INTO people (id, name, email, password, role, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", // WHAT: one person. WHY: the office's accounts.
                    params![id, name, email, hashed, role, "2026-09-01T00:00:00Z"], // WHAT: added before licensing began. WHY: a founding install, so sharing is never paused by a trial.
                )?;
            }
            Ok(())
        })
        .expect("three people"); // WHAT: all three in. WHY: every test needs them.
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("a runtime"); // WHAT: a small runtime. WHY: enough to serve one test's requests.
    let listener = runtime.block_on(tokio::net::TcpListener::bind(config.listen)).expect("a socket"); // WHAT: the listening socket. WHY: its port is the office's address.
    let address = listener.local_addr().expect("an address"); // WHAT: the port it got. WHY: for the base address.
    let app = api::router(Arc::new(Server::new(store, config))); // WHAT: the real router. WHY: the gates under test are in it.
    runtime.spawn(async move {
        let _ = axum::serve(listener, app).await; // WHAT: serving until the runtime is dropped. WHY: the test talks to it over the socket.
    });
    Office { base: format!("http://{address}"), scratch, _runtime: runtime } // WHAT: the running office. WHY: handed to the test.
}

/// A session for one of the office's people.
fn session(office: &Office, email: &str) -> String {
    let mut client = hub::Client::new(&office.base); // WHAT: the program's own client. WHY: signs in the way the desktop does.
    client.sign_in(email, PASSWORD).expect("sign in"); // WHAT: signed in. WHY: keys are made by signed-in people.
    client.token().expect("a token").to_string() // WHAT: the session token. WHY: used as a bearer below.
}

/// A request with `token` as its bearer: the status and the answer's JSON.
fn ask(office: &Office, method: &str, path: &str, token: &str, body: Option<Value>) -> (u16, Value) {
    let url = format!("{}{path}", office.base); // WHAT: the full address. WHY: paths here include /api/v1 or /mcp.
    let request = ureq::request(method, &url).set("Authorization", &format!("Bearer {token}")); // WHAT: the request with its bearer. WHY: every route here is asked as somebody.
    let answer = match body {
        Some(body) => request.send_json(body), // WHAT: with a JSON body. WHY: the writes take one.
        None => request.call(), // WHAT: without one. WHY: the reads.
    };
    match answer {
        Ok(r) => (r.status(), r.into_json().unwrap_or(Value::Null)), // WHAT: a success and its JSON. WHY: some answers are checked.
        Err(ureq::Error::Status(code, r)) => (code, r.into_json().unwrap_or(Value::Null)), // WHAT: a refusal and its problem. WHY: the error name is checked.
        Err(e) => panic!("{method} {path}: {e}"), // WHAT: nothing answered. WHY: a broken test, not a refusal.
    }
}

/// A key made by whoever `token` is: the key itself.
fn a_key(office: &Office, token: &str, purpose: &str, name: &str) -> String {
    let (status, made) = ask(office, "POST", "/api/v1/me/keys", token, Some(json!({ "purpose": purpose, "name": name }))); // WHAT: ask for a key. WHY: the way every program's key is made.
    assert_eq!(status, 200, "{purpose} key: {made}"); // WHAT: it was made. WHY: the rest of the test depends on it.
    made["key"].as_str().expect("the key, shown once").to_string() // WHAT: the key. WHY: used as the bearer.
}

/// One MCP tool call: its text and whether it was an error.
fn tool(office: &Office, token: &str, name: &str, arguments: Value) -> (String, bool) {
    let (status, answer) = ask(
        office,
        "POST",
        "/mcp",
        token,
        Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": arguments } })), // WHAT: a tool call. WHY: how an assistant asks.
    );
    assert_eq!(status, 200, "{answer}"); // WHAT: the endpoint answered. WHY: refusals come back inside the answer.
    let text = answer.pointer("/result/content/0/text").and_then(Value::as_str).unwrap_or_default().to_string(); // WHAT: what the assistant is told. WHY: checked for what it names.
    (text, answer.pointer("/result/isError").and_then(Value::as_bool).unwrap_or(false)) // WHAT: and whether it was a refusal. WHY: refused tools say so.
}

/// A two-sheet PDF, `mark` making its bytes differ from another's.
fn a_drawing(mark: &str) -> Vec<u8> {
    let mut out = format!("%PDF-1.7\n%{mark}\n").into_bytes(); // WHAT: the header and a distinguishing comment. WHY: two uploads that differ are two drawings.
    let mut offsets = Vec::new(); // WHAT: where each object starts. WHY: the cross-reference table needs them.
    for body in [
        "1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n", // WHAT: the catalog. WHY: every PDF has one.
        "2 0 obj\n<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>\nendobj\n", // WHAT: the page tree. WHY: two sheets.
        "3 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]>>\nendobj\n", // WHAT: sheet one. WHY: a drawing-sized page.
        "4 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]>>\nendobj\n", // WHAT: sheet two. WHY: likewise.
    ] {
        offsets.push(out.len()); // WHAT: this object's offset. WHY: for the table.
        out.extend_from_slice(body.as_bytes()); // WHAT: the object. WHY: into the file.
    }
    let xref = out.len(); // WHAT: where the table starts. WHY: the trailer points at it.
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes()); // WHAT: the table's head. WHY: PDF's layout.
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes()); // WHAT: one entry. WHY: PDF's layout.
    }
    out.extend_from_slice(format!("trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).as_bytes()); // WHAT: the trailer. WHY: a reader starts there.
    out // WHAT: the file. WHY: uploaded by the tests.
}

/// An upload into `project` as `token`: the status and the answer.
fn upload(office: &Office, token: &str, project: &str, mark: &str) -> (u16, Value) {
    let boundary = "----hyperviewkeys"; // WHAT: the multipart boundary. WHY: anything not in the body.
    let mut body = Vec::new(); // WHAT: the form. WHY: built by hand, as the other wire tests do.
    for (name, value) in [("project", project), ("name", "Structural")] {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes()); // WHAT: one field. WHY: the project and the drawing's name.
    }
    body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"s.pdf\"\r\nContent-Type: application/pdf\r\n\r\n").as_bytes()); // WHAT: the file's head. WHY: the server reads the file field.
    body.extend_from_slice(&a_drawing(mark)); // WHAT: the drawing. WHY: a real PDF.
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes()); // WHAT: the end. WHY: closes the form.
    match ureq::post(&format!("{}/api/v1/sets", office.base))
        .set("Authorization", &format!("Bearer {token}")) // WHAT: as whoever. WHY: the gate under test.
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}")) // WHAT: a form. WHY: what /sets takes.
        .send_bytes(&body)
    {
        Ok(r) => (r.status(), r.into_json().unwrap_or(Value::Null)), // WHAT: taken. WHY: checked by the caller.
        Err(ureq::Error::Status(code, r)) => (code, r.into_json().unwrap_or(Value::Null)), // WHAT: refused. WHY: checked by the caller.
        Err(e) => panic!("upload: {e}"), // WHAT: nothing answered. WHY: a broken test.
    }
}

/// An office with two jobs: "Open Job", everybody's, and "Private Job", with
/// only the estimator named on it. Each has a drawing. The administrator's
/// session, and the sets' ids (open, private).
fn two_jobs(office: &Office) -> (String, String, String, String, String) {
    let admin = session(office, "sam@example.com"); // WHAT: the administrator's session. WHY: makes the jobs and names people on them.
    let (_, open) = ask(office, "POST", "/api/v1/projects", &admin, Some(json!({ "number": "26-101", "name": "Open Job" }))); // WHAT: a job nobody is named on. WHY: everybody's.
    let (_, private) = ask(office, "POST", "/api/v1/projects", &admin, Some(json!({ "number": "26-202", "name": "Private Job" }))); // WHAT: a second job. WHY: the one to keep people off.
    let (open, private) = (open["id"].as_str().unwrap().to_string(), private["id"].as_str().unwrap().to_string()); // WHAT: their ids. WHY: used below.
    let (status, _) = ask(office, "POST", &format!("/api/v1/projects/{private}/people"), &admin, Some(json!({ "person": ESTIMATOR }))); // WHAT: the estimator named on the private job. WHY: once one name is on, it is theirs alone.
    assert_eq!(status, 200); // WHAT: named. WHY: the rest depends on it.
    let (_, open_set) = upload(office, &admin, &open, "open"); // WHAT: a drawing on the open job. WHY: something to read.
    let (_, private_set) = upload(office, &admin, &private, "private"); // WHAT: a drawing on the private job. WHY: something to be refused.
    (admin, open, private, open_set["id"].as_str().unwrap().to_string(), private_set["id"].as_str().unwrap().to_string()) // WHAT: what the tests need. WHY: one setup for all of them.
}

// ---- F1: an assistant sees what its person may see -------------------------

#[test]
fn a_viewers_assistant_key_cannot_list_or_read_a_job_it_is_not_on() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let (_, _, private, open_set, private_set) = two_jobs(&office); // WHAT: an open job and a private one. WHY: the private one is the one to hide.
    let viewer = session(&office, "val@example.com"); // WHAT: the viewer signed in. WHY: makes their own assistant key.
    let key = a_key(&office, &viewer, "assistant", "Val's assistant"); // WHAT: the viewer's assistant key. WHY: the key under test.

    let (jobs, _) = tool(&office, &key, "list_projects", json!({})); // WHAT: the jobs, as the assistant is told them. WHY: the listing must leave the private job off.
    assert!(jobs.contains("26-101"), "the open job is everybody's: {jobs}"); // WHAT: the open job is listed. WHY: the filter must not hide everything.
    assert!(!jobs.contains("26-202") && !jobs.contains("Private Job"), "the private job is listed for a viewer not on it: {jobs}"); // WHAT: the private job is not. WHY: the bug this fixes.

    let (sets, _) = tool(&office, &key, "list_sets", json!({})); // WHAT: every set the assistant may see. WHY: must not include the private job's.
    assert!(sets.contains(&open_set) && !sets.contains(&private_set), "{sets}"); // WHAT: the open set only. WHY: the same rule as the list.
    let (sets, _) = tool(&office, &key, "list_sets", json!({ "project": private })); // WHAT: asked for the private job by id. WHY: naming it must not open it.
    assert!(!sets.contains(&private_set), "{sets}"); // WHAT: nothing from it. WHY: refused as "no sets", as for a job that does not exist.
    let (sets, _) = tool(&office, &key, "list_sets", json!({ "project": "26-202" })); // WHAT: asked by its job number. WHY: another way in.
    assert!(!sets.contains(&private_set), "{sets}"); // WHAT: nothing from it. WHY: the same.

    for (name, arguments) in [
        ("takeoff", json!({ "set": private_set })), // WHAT: the private set's takeoff. WHY: each tool that reads a set.
        ("shop_list", json!({ "set": private_set })), // WHAT: its cut list. WHY: likewise.
        ("counted_twice", json!({ "set": private_set })), // WHAT: its doubles. WHY: likewise.
        ("sheets", json!({ "set": private_set })), // WHAT: its sheets. WHY: likewise.
        ("revision_cost", json!({ "was_bid": open_set, "arrived": private_set })), // WHAT: compared against it. WHY: either side is gated.
        ("revision_cost", json!({ "was_bid": private_set, "arrived": open_set })), // WHAT: the other way round. WHY: both sides.
    ] {
        let (text, refused) = tool(&office, &key, name, arguments); // WHAT: the call. WHY: each must be refused.
        assert!(refused, "{name} answered for a viewer not on the job: {text}"); // WHAT: refused. WHY: the bug this fixes.
        assert!(text.contains("There is no drawing set called"), "{name} must refuse as 'not here': {text}"); // WHAT: as "not found". WHY: nobody learns the job exists.
    }
    let (text, refused) = tool(&office, &key, "sheets", json!({ "set": open_set })); // WHAT: the open job's sheets. WHY: what the viewer may see still answers.
    assert!(!refused && text.contains("sheet(s) in"), "{text}"); // WHAT: answered. WHY: the gate is not a wall.
}

#[test]
fn an_administrators_assistant_key_still_sees_every_job() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let (admin, _, _, _, private_set) = two_jobs(&office); // WHAT: the two jobs. WHY: the administrator sees both.
    let key = a_key(&office, &admin, "assistant", "Sam's assistant"); // WHAT: the administrator's assistant key. WHY: reads what an administrator may.
    let (jobs, _) = tool(&office, &key, "list_projects", json!({})); // WHAT: the jobs. WHY: both should be there.
    assert!(jobs.contains("26-101") && jobs.contains("26-202"), "{jobs}"); // WHAT: both listed. WHY: administrators see every job.
    for (name, arguments) in [
        ("takeoff", json!({ "set": private_set })), // WHAT: each set tool on the private set. WHY: an administrator may read it.
        ("shop_list", json!({ "set": private_set })),
        ("counted_twice", json!({ "set": private_set })),
        ("sheets", json!({ "set": private_set })),
        ("revision_cost", json!({ "was_bid": private_set, "arrived": private_set })),
        ("list_sets", json!({ "project": "26-202" })),
    ] {
        let (text, refused) = tool(&office, &key, name, arguments.clone()); // WHAT: the call. WHY: each must answer.
        assert!(!refused, "{name} refused an administrator: {text}"); // WHAT: answered. WHY: the gate keeps what an administrator could always do.
        assert!(!text.contains("There is no drawing set called"), "{name}: {text}"); // WHAT: not refused in words either. WHY: the same.
    }
}

#[test]
fn an_estimator_named_on_a_job_reads_it_through_their_assistant() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let (_, _, _, _, private_set) = two_jobs(&office); // WHAT: the two jobs. WHY: the estimator is named on the private one.
    let estimator = session(&office, "eve@example.com"); // WHAT: the estimator signed in. WHY: makes their own assistant key.
    let key = a_key(&office, &estimator, "assistant", "Eve's assistant"); // WHAT: their key. WHY: membership, not role, opens the job.
    let (jobs, _) = tool(&office, &key, "list_projects", json!({})); // WHAT: the jobs. WHY: the private one is theirs.
    assert!(jobs.contains("26-202"), "{jobs}"); // WHAT: listed. WHY: named on it.
    let (text, refused) = tool(&office, &key, "takeoff", json!({ "set": private_set })); // WHAT: its takeoff. WHY: readable by someone on it.
    assert!(!refused, "{text}"); // WHAT: answered. WHY: the same as the API would.
}

// ---- F2: no key administers; a sync key keeps to its list -----------------

/// Every route the server serves, as "METHOD /api/v1/path" with ids filled in.
fn every_route() -> Vec<(String, String, String)> {
    api::ROUTES
        .iter()
        .map(|(method, path)| {
            let named = path.replace("{id}", ":id").replace("{code}", ":code").replace("{version}", ":version"); // WHAT: the router's own spelling. WHY: compared with SYNC_ROUTES.
            let real = path.replace("{id}", "x").replace("{code}", "x").replace("{version}", "9.9.9"); // WHAT: an address that reaches the route. WHY: the gate runs before the route looks anything up.
            (method.to_string(), named, format!("/api/v1{real}")) // WHAT: method, route name, address. WHY: what each test needs.
        })
        .collect()
}

#[test]
fn a_sync_key_is_refused_by_every_route_not_on_its_list_even_when_an_administrator_made_it() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let admin = session(&office, "sam@example.com"); // WHAT: the administrator. WHY: the most powerful maker there is.
    let key = a_key(&office, &admin, "sync", "Program"); // WHAT: a sync key made by them. WHY: must carry none of their power.
    let mut tried = 0; // WHAT: how many routes were asked. WHY: a test that asks nothing proves nothing.
    for (method, named, address) in every_route() {
        if api::SYNC_ROUTES.iter().any(|(m, p)| *m == method && *p == named) {
            continue; // WHAT: on the list. WHY: tested in the next test.
        }
        let body = if method == "GET" { None } else { Some(json!({})) }; // WHAT: a body for the writes. WHY: never reaches them anyway.
        let (status, said) = ask(&office, &method, &address, &key, body); // WHAT: the route asked with the sync key. WHY: each must refuse.
        assert_eq!(status, 403, "{method} {named} let a sync key through: {said}"); // WHAT: refused. WHY: the list is the whole of what a sync key may do.
        assert_eq!(said["error"], "key_not_allowed", "{method} {named}: {said}"); // WHAT: refused as a key, before the route ran. WHY: not by luck of the route's own checks.
        tried += 1; // WHAT: counted. WHY: for the check below.
    }
    assert!(tried > 40, "only {tried} routes were tried"); // WHAT: every admin route and every other write was in there. WHY: the list of routes is long; a short count means the test broke.
    let (text, _) = tool(&office, &key, "list_projects", json!({})); // WHAT: the assistant endpoint with a sync key. WHY: not on the list either.
    assert!(!text.contains("job(s)"), "a sync key read the jobs over /mcp: {text}"); // WHAT: no jobs. WHY: /mcp refuses it as nobody.
}

#[test]
fn a_sync_key_does_its_list_and_only_on_projects_filed_under_a_reference() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let admin = session(&office, "sam@example.com"); // WHAT: the administrator. WHY: makes the key and a hand-made job.
    let key = a_key(&office, &admin, "sync", "Program"); // WHAT: the sync key. WHY: under test.

    let (status, me) = ask(&office, "GET", "/api/v1/me", &key, None); // WHAT: who the key is. WHY: a program checks this when it connects.
    assert_eq!(status, 200); // WHAT: answered. WHY: /me is on the list.
    assert_eq!(me["key"]["purpose"], "sync", "{me}"); // WHAT: says it is a sync key. WHY: so the program can tell.
    assert_eq!(me["may_administer"], false, "{me}"); // WHAT: no administrator's power. WHY: whoever made it.
    assert_eq!(me["role"], "estimator", "{me}"); // WHAT: acts as an estimator. WHY: may write drawings, not run the server.

    let (status, made) = ask(&office, "POST", "/api/v1/projects", &key, Some(json!({ "number": "26-303", "name": "Synced", "reference": "program:bid:1", "place": "Bids/2026" }))); // WHAT: a project filed under a reference. WHY: the first thing a program does.
    assert_eq!(status, 200, "{made}"); // WHAT: made. WHY: on the list.
    let project = made["id"].as_str().unwrap().to_string(); // WHAT: its id. WHY: for the rest.
    let (status, again) = ask(&office, "POST", "/api/v1/projects", &key, Some(json!({ "number": "26-303", "name": "Synced, renamed", "reference": "program:bid:1" }))); // WHAT: the same reference again. WHY: found, not made twice.
    assert_eq!((status, again["id"].as_str()), (200, Some(project.as_str()))); // WHAT: the same project. WHY: a retried push never makes a second job.
    let (status, said) = ask(&office, "POST", "/api/v1/projects", &key, Some(json!({ "number": "26-304", "name": "No reference" }))); // WHAT: a project with no reference. WHY: a sync key may not make a hand-made job.
    assert_eq!(status, 400, "{said}"); // WHAT: refused. WHY: every sync project is filed under a reference.
    let (status, set) = upload(&office, &key, &project, "synced"); // WHAT: a drawing put up. WHY: on the list.
    assert_eq!(status, 200, "{set}"); // WHAT: taken. WHY: the program's drawings go up.
    let (status, _) = ask(&office, "GET", &format!("/api/v1/projects/{project}/stamp"), &key, None); // WHAT: the stamp. WHY: on the list.
    assert_eq!(status, 200); // WHAT: answered. WHY: the cheap check before a pull.
    let (status, list) = ask(&office, "GET", &format!("/api/v1/projects/{project}/markuplist"), &key, None); // WHAT: the markups. WHY: on the list.
    assert_eq!(status, 200, "{list}"); // WHAT: answered. WHY: the takeoff read back.
    let (status, _) = ask(&office, "POST", &format!("/api/v1/projects/{project}/archive"), &key, Some(json!({ "archived": true }))); // WHAT: put away. WHY: on the list.
    assert_eq!(status, 200); // WHAT: done. WHY: a deleted record follows here.

    let (_, by_hand) = ask(&office, "POST", "/api/v1/projects", &admin, Some(json!({ "number": "26-505", "name": "Made by hand" }))); // WHAT: a job an administrator made by hand. WHY: a sync key must not reach it.
    let by_hand = by_hand["id"].as_str().unwrap().to_string(); // WHAT: its id. WHY: asked for directly below.
    for (method, path) in [("GET", format!("/api/v1/projects/{by_hand}/stamp")), ("GET", format!("/api/v1/projects/{by_hand}/markuplist"))] {
        let (status, said) = ask(&office, method, &path, &key, None); // WHAT: asked for by id. WHY: guessing an id must not open it.
        assert_eq!(status, 404, "{path}: {said}"); // WHAT: "not found". WHY: nobody learns the job exists.
    }
    let (status, _) = ask(&office, "POST", &format!("/api/v1/projects/{by_hand}/archive"), &key, Some(json!({ "archived": true }))); // WHAT: put away by a sync key. WHY: must not reach it.
    assert_eq!(status, 404); // WHAT: refused as "not found". WHY: the same gate.
    let (status, _) = upload(&office, &key, &by_hand, "into-hand-made"); // WHAT: a drawing pushed into it. WHY: the upload is gated too.
    assert_eq!(status, 404); // WHAT: refused as "not found". WHY: the same gate.
}

#[test]
fn no_key_administers_whoever_made_it_and_an_integration_key_otherwise_works_as_before() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let admin = session(&office, "sam@example.com"); // WHAT: the administrator. WHY: makes the key.
    let key = a_key(&office, &admin, "integration", "Other program"); // WHAT: an integration key made by an administrator. WHY: before this, it could run the server.
    // Each admin route with a body its handler can read, so the refusal is the key's and not a malformed request's.
    let admin_routes: Vec<(&str, &str, Option<Value>)> = vec![
        ("GET", "/api/v1/admin/joining", None), // WHAT: how people get accounts. WHY: an admin read.
        ("POST", "/api/v1/admin/joining", Some(json!({ "how": "open" }))), // WHAT: opening the server to anyone. WHY: an admin write.
        ("GET", "/api/v1/admin/invites", None), // WHAT: the invitations. WHY: an admin read.
        ("POST", "/api/v1/admin/invites", Some(json!({ "email": "new@example.com", "name": "New", "role": "admin" }))), // WHAT: inviting an administrator. WHY: the most dangerous write.
        ("DELETE", "/api/v1/admin/invites/x", Some(json!({}))), // WHAT: dropping an invitation. WHY: an admin write.
        ("GET", "/api/v1/admin/updates", None), // WHAT: update settings. WHY: an admin read.
        ("POST", "/api/v1/admin/updates", Some(json!({ "channel": "early" }))), // WHAT: changing the update channel. WHY: an admin write.
        ("GET", "/api/v1/admin/fleet", None), // WHAT: fleet access. WHY: an admin read.
        ("POST", "/api/v1/admin/fleet", Some(json!({ "on": true }))), // WHAT: letting a fleet in. WHY: an admin write.
        ("GET", "/api/v1/admin/remote", None), // WHAT: remote access. WHY: an admin read.
        ("POST", "/api/v1/admin/remote", Some(json!({ "token": "t", "hostname": "h.example.com" }))), // WHAT: opening the server to the internet. WHY: the write F2 was about.
        ("DELETE", "/api/v1/admin/remote", Some(json!({}))), // WHAT: closing it. WHY: an admin write.
        ("POST", "/api/v1/projects/x/people", Some(json!({ "person": VIEWER }))), // WHAT: who is on a job. WHY: an admin write.
        ("GET", "/api/v1/people", None), // WHAT: everybody's accounts. WHY: an admin read.
        ("POST", "/api/v1/people", Some(json!({ "name": "New", "email": "n@example.com", "password": PASSWORD, "role": "admin" }))), // WHAT: adding an administrator. WHY: an admin write.
        ("POST", "/api/v1/people/x/role", Some(json!({ "role": "admin" }))), // WHAT: changing a role. WHY: an admin write.
        ("POST", "/api/v1/people/x/remove", Some(json!({}))), // WHAT: removing a person. WHY: an admin write.
        ("POST", "/api/v1/plugins", Some(json!({}))), // WHAT: adding a plugin. WHY: an admin write.
        ("POST", "/api/v1/plugins/x/remove", Some(json!({}))), // WHAT: removing one. WHY: an admin write.
        ("GET", "/api/v1/notices", None), // WHAT: change notices. WHY: an admin read.
        ("POST", "/api/v1/notices", Some(json!({ "url": "https://example.com/hook" }))), // WHAT: adding one. WHY: an admin write.
        ("POST", "/api/v1/notices/x/test", Some(json!({}))), // WHAT: testing one. WHY: an admin write.
        ("POST", "/api/v1/notices/x/remove", Some(json!({}))), // WHAT: removing one. WHY: an admin write.
        ("POST", "/api/v1/license", Some(json!({ "license": "x" }))), // WHAT: adding a license. WHY: an admin write.
        ("GET", "/api/v1/audit", None), // WHAT: the audit log. WHY: an admin read.
        ("GET", "/api/v1/audit.csv", None), // WHAT: the same as a file. WHY: an admin read.
        ("POST", "/api/v1/admin/pin", Some(json!({ "version": "9.9.9" }))), // WHAT: pinning every seat. WHY: an admin write.
        ("POST", "/api/v1/admin/releases/9.9.9/ready", Some(json!({ "platform": "windows-x64", "ready": true }))), // WHAT: offering a release. WHY: an admin write.
    ];
    for (method, path, body) in &admin_routes {
        let (status, said) = ask(&office, method, path, &key, body.clone()); // WHAT: the admin route with the key. WHY: must refuse.
        assert_eq!(status, 403, "{method} {path} let an administrator's integration key administer: {said}"); // WHAT: refused. WHY: keys never carry admin power.
        assert_eq!(said["error"], "key_not_allowed", "{method} {path}: {said}"); // WHAT: as a key. WHY: not as a missing account power.
    }
    for path in ["/api/v1/chests", "/api/v1/admin/releases"] {
        let answer = ureq::post(&format!("{}{path}", office.base))
            .set("Authorization", &format!("Bearer {key}")) // WHAT: the integration key. WHY: must refuse.
            .set("Content-Type", "multipart/form-data; boundary=x") // WHAT: an empty form. WHY: these take uploads, so the form must be readable for the refusal to be the key's.
            .send_bytes(b"--x--\r\n"); // WHAT: a form with nothing in it. WHY: the handler runs and asks who is asking.
        match answer {
            Err(ureq::Error::Status(403, r)) => assert_eq!(r.into_json::<Value>().unwrap()["error"], "key_not_allowed"), // WHAT: refused as a key. WHY: tool chests and releases are administrators' alone.
            other => panic!("POST {path} with an integration key: {other:?}"), // WHAT: anything else. WHY: a key uploaded a chest or a release.
        }
    }
    let (status, _) = ask(&office, "GET", "/api/v1/people", &admin, None); // WHAT: the same route with the administrator's own session. WHY: signed-in administrators still run the server.
    assert_eq!(status, 200); // WHAT: answered. WHY: only keys lost the power.
    let (status, me) = ask(&office, "GET", "/api/v1/me", &key, None); // WHAT: who the integration key is. WHY: says it cannot administer.
    assert_eq!((status, me["may_administer"].as_bool(), me["key"]["purpose"].as_str()), (200, Some(false), Some("integration")), "{me}"); // WHAT: an integration key, no admin power. WHY: what a program checks.
    assert_eq!(me["role"], "admin", "an integration key keeps its maker's role for everything else: {me}"); // WHAT: role as before. WHY: other customers' integrations keep working.

    let (status, project) = ask(&office, "POST", "/api/v1/projects", &key, Some(json!({ "number": "26-606", "name": "No reference needed" }))); // WHAT: a hand-made project by an integration key. WHY: allowed as before.
    assert_eq!(status, 200, "{project}"); // WHAT: made. WHY: integration keys are unchanged apart from admin power.
    let (status, _) = ask(&office, "GET", "/api/v1/projects", &key, None); // WHAT: the project list. WHY: allowed as before.
    assert_eq!(status, 200); // WHAT: answered. WHY: unchanged.
    let (status, _) = upload(&office, &key, project["id"].as_str().unwrap(), "integration"); // WHAT: a drawing. WHY: allowed as before.
    assert_eq!(status, 200); // WHAT: taken. WHY: unchanged.
}

#[test]
fn only_an_administrator_makes_a_sync_key_and_a_key_cannot_make_one() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let estimator = session(&office, "eve@example.com"); // WHAT: an estimator. WHY: may not make one.
    let (status, _) = ask(&office, "POST", "/api/v1/me/keys", &estimator, Some(json!({ "purpose": "sync", "name": "Program" }))); // WHAT: asking. WHY: refused.
    assert_eq!(status, 403); // WHAT: refused. WHY: a sync key reaches every program-filed job, which an estimator could not.
    let admin = session(&office, "sam@example.com"); // WHAT: the administrator. WHY: may.
    let key = a_key(&office, &admin, "sync", "Program"); // WHAT: a sync key. WHY: then used to try to make another.
    let (status, _) = ask(&office, "POST", "/api/v1/me/keys", &key, Some(json!({ "purpose": "sync", "name": "Another" }))); // WHAT: a key making a key. WHY: never.
    assert_eq!(status, 403); // WHAT: refused. WHY: keys are made by people.
    let (status, listed) = ask(&office, "GET", "/api/v1/me/keys", &admin, None); // WHAT: the administrator's key list. WHY: the sync key is there to take back.
    assert_eq!(status, 200); // WHAT: answered. WHY: a session may list keys.
    assert!(listed.as_array().unwrap().iter().any(|k| k["purpose"] == "sync" && k["key"].is_null()), "{listed}"); // WHAT: listed, without the key itself. WHY: shown once, never again.
}

#[test]
fn a_key_given_an_end_stops_at_its_end_and_one_without_never_does() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let admin = session(&office, "sam@example.com"); // WHAT: the administrator. WHY: makes the keys.
    let (status, made) = ask(&office, "POST", "/api/v1/me/keys", &admin, Some(json!({ "purpose": "sync", "name": "Ends", "expires_in_days": 30 }))); // WHAT: a key with an end. WHY: optional expiry.
    assert_eq!(status, 200, "{made}"); // WHAT: made. WHY: 30 days is allowed.
    assert!(made["expires"].as_str().is_some_and(|e| e > "2026"), "{made}"); // WHAT: its end is said. WHY: so whoever made it can see it.
    let key = made["key"].as_str().unwrap().to_string(); // WHAT: the key. WHY: used below.
    let (status, me) = ask(&office, "GET", "/api/v1/me", &key, None); // WHAT: before its end. WHY: it works.
    assert_eq!(status, 200); // WHAT: works. WHY: inside its time.
    assert_eq!(me["key"]["expires"], made["expires"], "{me}"); // WHAT: /me says when it ends. WHY: a program can warn before it does.

    let db = rusqlite::Connection::open(office.scratch.0.join("hyperview.sqlite")).unwrap(); // WHAT: the office's database. WHY: to move the end into the past without waiting a month.
    db.execute("UPDATE keys SET expires = '2020-01-01T00:00:00Z' WHERE name = 'Ends'", []).unwrap(); // WHAT: its end gone by. WHY: the moment it should stop.
    let (status, _) = ask(&office, "GET", "/api/v1/me", &key, None); // WHAT: after its end. WHY: must not work.
    assert_eq!(status, 401); // WHAT: refused as nobody. WHY: a key past its end is no key.
    db.execute("UPDATE keys SET expires = 'soon' WHERE name = 'Ends'", []).unwrap(); // WHAT: an end nobody can read. WHY: the safe way round.
    let (status, _) = ask(&office, "GET", "/api/v1/me", &key, None); // WHAT: asked with it. WHY: an unreadable end is not "forever".
    assert_eq!(status, 401); // WHAT: refused. WHY: as if it had run out.

    for days in [0, 3651] {
        let (status, said) = ask(&office, "POST", "/api/v1/me/keys", &admin, Some(json!({ "purpose": "sync", "name": "Bad", "expires_in_days": days }))); // WHAT: an end out of range. WHY: refused, not guessed at.
        assert_eq!(status, 400, "{days} days: {said}"); // WHAT: refused. WHY: zero is dead on arrival, more than ten years is "never" said badly.
    }
    let plain = a_key(&office, &admin, "integration", "Never ends"); // WHAT: a key with no end asked for. WHY: the default.
    let (status, me) = ask(&office, "GET", "/api/v1/me", &plain, None); // WHAT: asked with it. WHY: works.
    assert_eq!(status, 200); // WHAT: works. WHY: no end means never.
    assert!(me["key"].get("expires").is_none(), "{me}"); // WHAT: no end said. WHY: it has none.
}

#[test]
fn a_person_cannot_reach_a_job_they_are_not_on_by_uploading_into_it_or_filing_under_its_reference() {
    let office = an_office(); // WHAT: a fresh office. WHY: nothing from another test.
    let admin = session(&office, "sam@example.com"); // WHAT: the administrator. WHY: makes the private job.
    let (_, private) = ask(&office, "POST", "/api/v1/projects", &admin, Some(json!({ "number": "26-707", "name": "Kept", "reference": "program:bid:7" }))); // WHAT: a job filed under a reference. WHY: two doors into it to try.
    let private = private["id"].as_str().unwrap().to_string(); // WHAT: its id. WHY: used below.
    ask(&office, "POST", &format!("/api/v1/projects/{private}/people"), &admin, Some(json!({ "person": ADMIN }))); // WHAT: only the administrator named on it. WHY: the estimator is now kept off.
    let estimator = session(&office, "eve@example.com"); // WHAT: the estimator, not on it. WHY: may write, but not here.
    let (status, _) = upload(&office, &estimator, &private, "sneak"); // WHAT: a drawing pushed into it. WHY: the upload had no project gate.
    assert_eq!(status, 404); // WHAT: refused as "not found". WHY: the same gate as every other route.
    let (status, _) = ask(&office, "POST", "/api/v1/projects", &estimator, Some(json!({ "number": "26-707", "name": "Renamed", "reference": "program:bid:7" }))); // WHAT: renaming it through its reference. WHY: the other door.
    assert_eq!(status, 404); // WHAT: refused as "not found". WHY: a reference must not reach what an id would not.
    let (status, _) = ask(&office, "GET", &format!("/api/v1/projects/{private}"), &estimator, None); // WHAT: and by id. WHY: to show the two agree.
    assert_eq!(status, 404); // WHAT: the same answer. WHY: one rule.
}
