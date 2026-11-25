//! A job's crew, kept by its program, run for real over a socket
//! (`GET` and `POST /projects/{id}/crew`, `crate::crew`).
//!
//! Fabwire sets who sees each of its jobs on the iPads by sign-in, through
//! its sync key. Only Fabwire's key, only on a project it filed; a sign-in
//! nobody has is `404 no_such_person`; the last person is never taken off,
//! since a project with nobody named is everybody's; every change is on the
//! audit log, naming the key; and who is on it decides who sees the job.

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
        let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("crew-{n:016x}")); // WHAT: the folder. WHY: under the build's own scratch folder, never a shared one
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

/// A running office, its people signed in, and its keys.
struct Office {
    base: String,                      // WHAT: http://127.0.0.1:port. WHY: every request goes there.
    scratch: Scratch,                  // WHAT: its data folder. WHY: kept alive, and the audit read from it.
    admin: String,                     // WHAT: Sam's session, the administrator. WHY: makes the keys and a project by hand.
    fay: String,                       // WHAT: Fay's session, a viewer. WHY: put on the job.
    nan: String,                       // WHAT: Nan's session, a viewer. WHY: never put on it.
    eve: String,                       // WHAT: Eve's session, an estimator. WHY: the office, who sees a Fabwire job with nobody named.
    fabwire: String,                   // WHAT: Fabwire's sync key. WHY: the only one that keeps a crew.
    other: String,                     // WHAT: a sync key made for another program. WHY: refused.
    integration: String,               // WHAT: an integration key. WHY: refused.
    _runtime: tokio::runtime::Runtime, // WHAT: the async runtime. WHY: dropping it stops the server.
}

fn an_office() -> Office {
    let scratch = Scratch::new(); // WHAT: a fresh folder. WHY: a fresh office.
    let config = Config { name: "Example Steel".into(), data: scratch.0.clone(), listen: "127.0.0.1:0".parse().unwrap(), ..Config::default() }; // WHAT: an office on any free port. WHY: tests run side by side.
    let store = Store::open(&config.database(), &config.blobs()).expect("a store"); // WHAT: the database. WHY: the people go in it.
    let hashed = auth::hash_password(PASSWORD).unwrap(); // WHAT: hashed once. WHY: Argon2 is slow on purpose.
    store
        .with(|db| {
            for (id, name, email, role) in [("usr_sam", "Sam", "sam@example.com", "admin"), ("usr_fay", "Fay", "fay@example.com", "viewer"), ("usr_sid", "Sid", "sid@example.com", "viewer"), ("usr_nan", "Nan", "nan@example.com", "viewer"), ("usr_eve", "Eve", "eve@example.com", "estimator")] {
                db.execute("INSERT INTO people (id, name, email, password, role, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![id, name, email, hashed, role, "2026-09-01T00:00:00Z"])?; // WHAT: one person, before licensing began. WHY: a founding install, so nothing is paused.
            }
            Ok(())
        })
        .expect("the people");
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("a runtime"); // WHAT: a small runtime. WHY: enough for one test.
    let listener = runtime.block_on(tokio::net::TcpListener::bind(config.listen)).expect("a socket"); // WHAT: the socket. WHY: its port is the address.
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
    let mut office = Office {
        admin: session("sam@example.com"),
        fay: session("fay@example.com"),
        nan: session("nan@example.com"),
        eve: session("eve@example.com"),
        fabwire: String::new(),
        other: String::new(),
        integration: String::new(),
        base,
        scratch,
        _runtime: runtime,
    }; // WHAT: the office. WHY: the keys are made through it
    let key = |office: &Office, purpose: &str, name: &str| office.ok(&office.admin, "POST", "/me/keys", json!({ "purpose": purpose, "name": name }))["key"].as_str().unwrap().to_string(); // WHAT: a key made by Sam. WHY: as an administrator makes one in Studio
    office.fabwire = key(&office, "sync", "Fabwire V2"); // WHAT: Fabwire's key, named as Fabwire names it. WHY: the one that keeps a crew
    office.other = key(&office, "sync", "Estimating Plus"); // WHAT: another program's sync key. WHY: refused
    office.integration = key(&office, "integration", "Fabwire V2"); // WHAT: an integration key with Fabwire's name. WHY: refused: the purpose decides, not the name
    office
}

impl Office {
    /// A request as `token`: the status and the answer's JSON.
    fn ask(&self, token: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let request = ureq::request(method, &format!("{}/api/v1{path}", self.base)).set("Authorization", &format!("Bearer {token}")); // WHAT: as whoever. WHY: the gate under test.
        let answer = match body {
            Some(body) => request.send_json(body), // WHAT: with a body. WHY: the writes.
            None => request.call(),                // WHAT: without. WHY: the reads.
        };
        let read = |r: ureq::Response| serde_json::from_str(&r.into_string().unwrap_or_default()).unwrap_or(Value::Null); // WHAT: the JSON answer. WHY: checked.
        match answer {
            Ok(r) => (r.status(), read(r)),                       // WHAT: done. WHY: checked by the caller.
            Err(ureq::Error::Status(code, r)) => (code, read(r)), // WHAT: refused. WHY: checked by the caller.
            Err(e) => panic!("{method} {path}: {e}"),             // WHAT: nothing answered. WHY: a broken test, not a refusal.
        }
    }

    /// A request expected to work: its answer.
    fn ok(&self, token: &str, method: &str, path: &str, body: Value) -> Value {
        let (status, said) = self.ask(token, method, path, Some(body)); // WHAT: asked. WHY: setting up.
        assert_eq!(status, 200, "{method} {path}: {said}"); // WHAT: it worked. WHY: a set-up that failed would make the test meaningless.
        said
    }

    /// The crew's sign-ins, as Fabwire reads them.
    fn crew(&self, project: &str) -> Vec<String> {
        let (status, said) = self.ask(&self.fabwire, "GET", &format!("/projects/{project}/crew"), None); // WHAT: read with Fabwire's key. WHY: how Fabwire reads it
        assert_eq!(status, 200, "{said}"); // WHAT: read. WHY: its own project
        said.as_array().unwrap().iter().map(|m| m["email"].as_str().unwrap().to_string()).collect() // WHAT: the sign-ins. WHY: compared
    }

    /// Fabwire puts a sign-in on, or takes it off: the status and the answer.
    fn set(&self, project: &str, email: &str, on: bool) -> (u16, Value) {
        self.ask(&self.fabwire, "POST", &format!("/projects/{project}/crew"), Some(json!({ "email": email, "on": on }))) // WHAT: asked with Fabwire's key. WHY: as Fabwire asks
    }

    /// How many audit lines say `action` and mention `detail`.
    fn audited(&self, action: &str, detail: &str) -> i64 {
        let db = rusqlite::Connection::open(self.scratch.0.join("hyperview.sqlite")).unwrap(); // WHAT: the database. WHY: the audit log is in it.
        db.query_row("SELECT count(*) FROM audit WHERE action = ?1 AND instr(detail, ?2) > 0", params![action, detail], |r| r.get(0)).unwrap() // WHAT: counted. WHY: one line a change
    }

    /// Fabwire's job 2601, filed under its reference.
    fn job(&self) -> String {
        self.job_numbered("2601") // WHAT: job 2601. WHY: most tests need one job
    }

    /// Fabwire's job `number`, filed under its reference.
    fn job_numbered(&self, number: &str) -> String {
        self.ok(&self.fabwire, "POST", "/projects", json!({ "number": number, "name": "Hotel Stair", "reference": format!("fabwire:job:{number}") }))["id"].as_str().unwrap().to_string() // WHAT: made by Fabwire's key. WHY: a project the program keeps
    }

    /// Whether `token`'s person finds `project` in the project list.
    fn listed(&self, token: &str, project: &str) -> bool {
        let (status, said) = self.get_status(token, "/projects"); // WHAT: their list. WHY: a listing and a direct fetch must agree
        assert_eq!(status, 200, "{said}");
        said.as_array().unwrap().iter().any(|p| p["id"] == project) // WHAT: whether it is in it. WHY: checked
    }

    fn get_status(&self, token: &str, path: &str) -> (u16, Value) {
        self.ask(token, "GET", path, None) // WHAT: a read. WHY: the visibility checks
    }
}

// WHAT: Fabwire's key reads the crew, puts people on by sign-in (capitals aside), says who is on it with their names, changes nothing
//       for a repeat, takes one off, refuses the last one off and a sign-in nobody has, and writes each change on the audit log.
// WHY:  the contract Fabwire's crews are built on (its Decision 0412, point 11).
#[test]
fn fabwires_key_keeps_a_jobs_crew_by_sign_in() {
    let office = an_office(); // WHAT: a running office. WHY: the real routes.
    let job = office.job(); // WHAT: Fabwire's job. WHY: its crew is kept.
    assert!(office.crew(&job).is_empty(), "nobody named yet: everybody's"); // WHAT: empty. WHY: a new project is everybody's
    let (status, said) = office.set(&job, " FAY@example.com ", true); // WHAT: Fay put on, as typed with capitals and spaces. WHY: sign-ins are compared that way
    assert_eq!(status, 200, "{said}");
    assert_eq!(said, json!([{ "email": "fay@example.com", "name": "Fay" }]), "the answer is the crew now, with names"); // WHAT: the list as it now is. WHY: Fabwire keeps what it was told
    assert_eq!(office.set(&job, "sid@example.com", true).0, 200); // WHAT: Sid put on. WHY: a second person
    assert_eq!(office.crew(&job), ["fay@example.com", "sid@example.com"]); // WHAT: both, in the order put on. WHY: a steady order
    assert_eq!(office.set(&job, "sid@example.com", true).0, 200, "a repeat is fine"); // WHAT: Sid again. WHY: Fabwire may ask again after a lost answer
    assert_eq!(office.audited("project_person_added", "sid@example.com"), 1, "a repeat writes nothing"); // WHAT: one line for Sid. WHY: nothing changed the second time
    assert_eq!(office.audited("project_person_added", "by the program's key \"Fabwire V2\""), 2, "each line names the key"); // WHAT: the key named. WHY: somebody will ask which program put them on
    let (status, said) = office.set(&job, "nobody@example.com", true); // WHAT: a sign-in nobody has. WHY: Fabwire says it on the job's page
    assert_eq!((status, said["error"].as_str()), (404, Some("no_such_person")), "{said}");
    assert_eq!(office.set(&job, "sid@example.com", false).0, 200); // WHAT: Sid taken off. WHY: off the job
    assert_eq!(office.audited("project_person_removed", "sid@example.com"), 1); // WHAT: written. WHY: when a job stopped showing for him
    let (status, said) = office.set(&job, "fay@example.com", false); // WHAT: the last person taken off. WHY: that would make the job everybody's
    assert_eq!((status, said["error"].as_str()), (409, Some("last_person")), "{said}");
    assert_eq!(office.crew(&job), ["fay@example.com"], "Fay is still on it"); // WHAT: unchanged. WHY: refused
    assert_eq!(office.ask(&office.fabwire, "POST", &format!("/projects/{job}/crew"), Some(json!({ "email": "fay@example.com", "on": false, "extra": 1 }))).0, 422, "an unknown field is refused"); // WHAT: a field not in the contract. WHY: never half-read
    assert_eq!(office.ask(&office.fabwire, "POST", &format!("/projects/{job}/crew"), Some(json!({ "email": "", "on": true }))).0, 400, "no sign-in is refused"); // WHAT: empty. WHY: said plainly
}

// WHAT: who is on the crew decides who sees the job: Fay on it sees it, Nan doesn't; the office's administrator always does.
// WHY:  the point of the route: the iPads show each person only the jobs they are on.
#[test]
fn the_crew_decides_who_sees_the_job() {
    let office = an_office(); // WHAT: a running office. WHY: the real routes.
    let job = office.job(); // WHAT: Fabwire's job. WHY: under test.
    assert_eq!(office.ask(&office.nan, "GET", &format!("/projects/{job}"), None).0, 404, "a Fabwire job with nobody named is the office's only"); // WHAT: Nan doesn't see it. WHY: Fabwire keeps who sees its jobs; with nobody named, never the shop or field
    assert_eq!(office.ask(&office.eve, "GET", &format!("/projects/{job}"), None).0, 200, "the office does"); // WHAT: Eve sees it. WHY: an estimator is the office
    assert_eq!(office.set(&job, "fay@example.com", true).0, 200); // WHAT: Fay put on. WHY: she alone of the viewers sees it now
    assert_eq!(office.ask(&office.fay, "GET", &format!("/projects/{job}"), None).0, 200, "Fay is on it"); // WHAT: seen. WHY: on the crew
    assert_eq!(office.ask(&office.nan, "GET", &format!("/projects/{job}"), None).0, 404, "Nan is not"); // WHAT: not found. WHY: off the crew, and refused as missing
    assert_eq!(office.ask(&office.admin, "GET", &format!("/projects/{job}"), None).0, 200, "the administrator always does"); // WHAT: seen. WHY: administrators see every job
}

// WHAT: nobody but Fabwire's own sync key keeps a crew: a person (even the administrator), another program's sync key and an
//       integration key are refused; Fabwire's key on a project somebody made by hand is "not found".
// WHY:  who sees a customer's job is the one program's to keep, and a sync key never touches a job it didn't file.
#[test]
fn only_fabwires_key_keeps_a_crew_and_only_on_its_own_projects() {
    let office = an_office(); // WHAT: a running office. WHY: the real routes.
    let job = office.job(); // WHAT: Fabwire's job. WHY: asked about.
    let path = format!("/projects/{job}/crew"); // WHAT: its crew. WHY: every refusal below asks it
    let body = json!({ "email": "fay@example.com", "on": true }); // WHAT: a valid change. WHY: so the refusal is the gate's
    for (who, token) in [("the administrator", &office.admin), ("a viewer", &office.fay), ("another program's sync key", &office.other)] {
        let (status, said) = office.ask(token, "POST", &path, Some(body.clone())); // WHAT: asked to change it. WHY: refused
        assert_eq!((status, said["error"].as_str()), (403, Some("program_only")), "{who}: {said}");
        let (status, said) = office.ask(token, "GET", &path, None); // WHAT: asked to read it. WHY: refused too
        assert_eq!((status, said["error"].as_str()), (403, Some("program_only")), "{who} reading: {said}");
    }
    assert_eq!(office.ask(&office.integration, "POST", &path, Some(body.clone())).0, 403, "an integration key, whatever its name"); // WHAT: refused. WHY: only a sync key
    assert!(office.crew(&job).is_empty(), "nothing was changed"); // WHAT: still nobody. WHY: every attempt was refused
    let by_hand = office.ok(&office.admin, "POST", "/projects", json!({ "number": "9001", "name": "Made by hand" }))["id"].as_str().unwrap().to_string(); // WHAT: a project with no reference. WHY: not the program's
    let (status, _) = office.ask(&office.fabwire, "POST", &format!("/projects/{by_hand}/crew"), Some(body)); // WHAT: Fabwire's key on it. WHY: refused
    assert_eq!(status, 404, "a project the key didn't file is not found"); // WHAT: not found. WHY: nobody learns a job exists by being refused it
    assert_eq!(office.ask(&office.fabwire, "GET", &format!("/projects/{by_hand}/crew"), None).0, 404); // WHAT: nor read. WHY: likewise
}

// WHAT: removals sent at the same moment on jobs with three people on them, three at once each: exactly one person stays on every
//       job and the other two are taken off; never a job left with nobody named.
// WHY:  the review of 0.8.0 (medium): the count and the delete were two steps, so two removals at once both passed the count and
//       emptied a two-person job, which a viewer could then see. They are one statement now.
#[test]
fn removals_at_the_same_time_never_leave_a_job_with_nobody_named() {
    let office = Arc::new(an_office()); // WHAT: a running office, shared by the threads. WHY: the requests race for real.
    let people = ["fay@example.com", "sid@example.com", "nan@example.com"]; // WHAT: three on each job. WHY: three removals race, two may win
    let jobs: Vec<String> = (0..60).map(|n| office.job_numbered(&format!("R{n}"))).collect(); // WHAT: sixty jobs. WHY: enough tries for the race to show
    for job in &jobs {
        for email in people {
            assert_eq!(office.set(job, email, true).0, 200); // WHAT: each put on. WHY: the start
        }
    }
    for job in &jobs {
        let start = Arc::new(std::sync::Barrier::new(people.len())); // WHAT: a starting line. WHY: the removals leave together
        let threads: Vec<_> = people
            .iter()
            .map(|email| {
                let (office, job, start, email) = (Arc::clone(&office), job.clone(), Arc::clone(&start), (*email).to_string()); // WHAT: each thread's own handles. WHY: moved in
                std::thread::spawn(move || {
                    start.wait(); // WHAT: waits for the others. WHY: all three at once
                    office.set(&job, &email, false).0 // WHAT: its removal's status. WHY: counted below
                })
            })
            .collect();
        let mut statuses: Vec<u16> = threads.into_iter().map(|t| t.join().unwrap()).collect(); // WHAT: every answer. WHY: checked
        statuses.sort_unstable();
        assert_eq!(statuses, [200, 200, 409], "two taken off, the last refused, on {job}"); // WHAT: exactly one refusal. WHY: the last person stays
        assert_eq!(office.crew(job).len(), 1, "one person stays on {job}"); // WHAT: never empty. WHY: an empty Fabwire job would fall to the office alone, an empty hand-made one to everybody
    }
}

// WHAT: a Fabwire job whose only person is deleted from the office: nobody named, and then the office's only; shop and field
//       accounts don't see it, listed or fetched; an estimator and the administrator do. A project made by hand with nobody named is
//       still everybody's, as it always was.
// WHY:  the review of 0.8.0 (low): deleting a person cascades off every project they were on, which used to open the job to everybody.
#[test]
fn a_fabwire_job_with_nobody_named_is_the_offices_only_never_the_shops_or_fields() {
    let office = an_office(); // WHAT: a running office. WHY: the real routes.
    let job = office.job(); // WHAT: Fabwire's job. WHY: under test.
    assert_eq!(office.set(&job, "fay@example.com", true).0, 200); // WHAT: Fay alone on it. WHY: her removal from the office empties it
    assert_eq!(office.ask(&office.admin, "POST", "/people/usr_fay/remove", Some(json!({}))).0, 200, "Fay removed from the office"); // WHAT: Fay deleted. WHY: her place on the job goes with her
    assert!(office.crew(&job).is_empty(), "nobody named now"); // WHAT: empty. WHY: the cascade
    assert_eq!(office.ask(&office.nan, "GET", &format!("/projects/{job}"), None).0, 404, "a viewer doesn't see it"); // WHAT: not found. WHY: never the shop's or field's
    assert!(!office.listed(&office.nan, &job), "nor finds it in the list"); // WHAT: not listed. WHY: a list and a fetch agree
    assert_eq!(office.ask(&office.eve, "GET", &format!("/projects/{job}"), None).0, 200, "an estimator does"); // WHAT: seen. WHY: the office
    assert!(office.listed(&office.eve, &job), "and finds it in the list"); // WHAT: listed. WHY: likewise
    assert!(office.listed(&office.admin, &job), "the administrator always does"); // WHAT: listed. WHY: administrators see every job
    let by_hand = office.ok(&office.admin, "POST", "/projects", json!({ "number": "9002", "name": "Made by hand" }))["id"].as_str().unwrap().to_string(); // WHAT: a project made by hand, nobody named. WHY: unchanged
    assert_eq!(office.ask(&office.nan, "GET", &format!("/projects/{by_hand}"), None).0, 200, "a hand-made project with nobody named is everybody's, as before"); // WHAT: seen. WHY: today's projects are left as they were
    assert!(office.listed(&office.nan, &by_hand)); // WHAT: listed. WHY: likewise
}
