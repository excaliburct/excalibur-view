//! The security baseline's fixes, held by tests that ran red before them.
//!
//! Each test here was written first and watched failing against 0.6.25,
//! then the fix made it pass, then the fix was broken on purpose and the test
//! watched failing again. The server is served exactly as an office serves it,
//! with each caller's socket address.

use std::io::{Read, Write}; // WHAT: raw socket reads and writes. WHY: some tests send a body by hand, part of it at a time.
use std::net::{SocketAddr, TcpStream}; // WHAT: addresses and a plain TCP connection. WHY: as above.
use std::sync::Arc; // WHAT: shared ownership. WHY: the router takes the server as an Arc.
use std::time::{Duration, Instant}; // WHAT: time. WHY: some tests measure how long an answer takes.

use hyperview_server::api::{self, Server}; // WHAT: the router and its state. WHY: the real server.
use hyperview_server::{auth, Config, Store}; // WHAT: hashing, configuration, storage. WHY: an office set up directly.
use rusqlite::params; // WHAT: bound SQL parameters. WHY: people and sessions written straight in.
use serde_json::{json, Value}; // WHAT: JSON. WHY: bodies and answers.

const PASSWORD: &str = "brace-gusset-purlin-shim-42"; // WHAT: everybody's password. WHY: long enough.

/// A folder that is gone when the test is.
struct Scratch(std::path::PathBuf); // WHAT: the office's data folder. WHY: each test its own.

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0); // WHAT: removed. WHY: tests leave nothing.
    }
}

/// A running office.
struct Office {
    base: String,                      // WHAT: http://127.0.0.1:port. WHY: requests go there.
    address: SocketAddr,               // WHAT: its socket address. WHY: raw connections go there.
    server: Arc<Server>,               // WHAT: its state. WHY: sessions are written into its store.
    _scratch: Scratch,                 // WHAT: its folder. WHY: kept while it runs.
    _runtime: tokio::runtime::Runtime, // WHAT: the runtime serving it. WHY: dropped, it stops.
}

fn an_office() -> Office {
    let n: u64 = rand::random(); // WHAT: a random number. WHY: a folder no other test uses.
    let scratch = Scratch(std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("hyperview-asvs-{n:016x}"))); // WHAT: under the build's own temporary folder. WHY: never one another account shares.
    std::fs::create_dir_all(&scratch.0).expect("a scratch directory"); // WHAT: made. WHY: the store writes into it.
    let config = Config {
        name: "Example Steel".into(), // WHAT: its name. WHY: any.
        data: scratch.0.clone(), // WHAT: its folder. WHY: the scratch one.
        listen: "127.0.0.1:0".parse().unwrap(), // WHAT: any free port. WHY: tests run side by side.
        ..Config::default() // WHAT: everything else as shipped. WHY: the server an office runs.
    };
    let store = Store::open(&config.database(), &config.blobs()).expect("a store"); // WHAT: the database. WHY: the people go in it.
    let hashed = auth::hash_password(PASSWORD).unwrap(); // WHAT: hashed once. WHY: Argon2 is slow on purpose.
    store
        .with(|db| {
            for (id, name, email, role) in [
                ("usr_admin", "Sam", "sam@example.com", "admin"),
                ("usr_estimator", "Eve", "eve@example.com", "estimator"),
            ] {
                db.execute(
                    "INSERT INTO people (id, name, email, password, role, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![id, name, email, hashed, role, "2026-09-01T00:00:00Z"],
                )?; // WHAT: one person, from before licensing. WHY: a founding office.
            }
            Ok(())
        })
        .expect("people");
    let server = Arc::new(Server::new(store, config.clone())); // WHAT: the server's state. WHY: kept for sessions.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .enable_all()
        .build()
        .expect("a runtime"); // WHAT: eight workers. WHY: enough that requests sent together really run together.
    let listener = runtime.block_on(tokio::net::TcpListener::bind(config.listen)).expect("a socket"); // WHAT: the socket. WHY: its port is the address.
    let address = listener.local_addr().expect("an address"); // WHAT: where it listens. WHY: the base address.
    let app = api::router(server.clone()); // WHAT: the real router. WHY: the fixes are in it.
    runtime.spawn(async move {
        let _ = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await; // WHAT: served with each caller's address. WHY: as `run::serve` does.
    });
    Office { base: format!("http://{address}"), address, server, _scratch: scratch, _runtime: runtime }
}

/// A session for `person`, written straight in.
fn session(office: &Office, person: &str) -> String {
    let (token, hashed) = auth::new_token(); // WHAT: a token and what is kept of it. WHY: as signing in makes.
    office
        .server
        .store
        .with(|db| {
            db.execute("INSERT INTO sessions (token, person, issued, expires) VALUES (?1, ?2, ?3, ?4)", params![hashed, person, "2026-01-01T00:00:00Z", "2999-01-01T00:00:00Z"])?;
            Ok(())
        })
        .expect("a session");
    token
}

/// A JSON request: its status and answer.
fn post(office: &Office, path: &str, token: Option<&str>, extra: &[(&str, &str)], body: Value) -> (u16, Value) {
    let mut request = ureq::post(&format!("{}{path}", office.base)); // WHAT: the request. WHY: every one here is a POST.
    if let Some(token) = token {
        request = request.set("Authorization", &format!("Bearer {token}")); // WHAT: the bearer. WHY: who asks.
    }
    for (name, value) in extra {
        request = request.set(name, value); // WHAT: a header the caller chose. WHY: some tests claim an address.
    }
    match request.send_json(body) {
        Ok(r) => (r.status(), r.into_json().unwrap_or(Value::Null)),
        Err(ureq::Error::Status(code, r)) => (code, r.into_json().unwrap_or(Value::Null)),
        Err(e) => panic!("{path}: nothing answered: {e}"),
    }
}

/// Sends a request by hand that says its body is `declared` bytes long, then
/// sends only `sent` of them and waits up to `wait` for an answer. The
/// status line's code, or None when nothing came back in time.
fn part_of_a_body(office: &Office, path: &str, declared: usize, sent: usize, wait: Duration) -> Option<u16> {
    let mut socket = TcpStream::connect(office.address).expect("a connection"); // WHAT: a plain connection. WHY: so the body can stop part way.
    socket.set_read_timeout(Some(wait)).unwrap(); // WHAT: how long to wait for the answer. WHY: a server still reading never answers.
    let head = format!("POST {path} HTTP/1.1\r\nHost: probe\r\nContent-Type: application/json\r\nContent-Length: {declared}\r\n\r\n"); // WHAT: the request's head. WHY: it promises a large body.
    let _ = socket.write_all(head.as_bytes()); // WHAT: sent. WHY: the server starts on it.
    let _ = socket.write_all(&vec![b'a'; sent]); // WHAT: part of the body. WHY: enough to pass any small limit, far short of what was promised.
    let mut answer = Vec::new(); // WHAT: what comes back. WHY: its first line is read.
    let mut buffer = [0u8; 1024];
    let until = Instant::now() + wait; // WHAT: when to give up. WHY: as above.
    while Instant::now() < until && !answer.windows(2).any(|w| w == b"\r\n") {
        match socket.read(&mut buffer) {
            Ok(0) => break, // WHAT: closed. WHY: whatever came is all there is.
            Ok(n) => answer.extend_from_slice(&buffer[..n]),
            Err(_) => break, // WHAT: timed out or reset. WHY: nothing more will come.
        }
    }
    let line = String::from_utf8_lossy(&answer); // WHAT: the status line. WHY: its code is the answer.
    line.split_whitespace().nth(1).and_then(|code| code.parse().ok())
}

#[test]
fn a_stranger_cannot_make_the_server_hold_a_big_body() {
    let office = an_office(); // WHAT: a running office. WHY: asked by nobody.
    let wait = Duration::from_secs(20); // WHAT: long enough for any real answer. WHY: a server still reading gives none.
    // WHAT: the doors open to anybody take a small body and say 413 past it. WHY: before, each read up to 512 MiB, so a few strangers at once could take all the server's memory.
    for path in ["/api/v1/auth/token", "/api/v1/join", "/api/v1/setup"] {
        let said = part_of_a_body(&office, path, 100 * 1024 * 1024, api::SMALL_BODY + 4096, wait);
        assert_eq!(said, Some(413), "{path} kept reading a stranger's 100 MiB body");
    }
    // WHAT: the doors that take a large body ask who is calling before reading it. WHY: before, the body was read first, up to 512 MiB, and only then was a stranger turned away.
    for path in ["/api/v1/plugins/nothing/run", "/api/v1/plugins", "/api/v1/sets/nothing/markups", "/api/v1/projects/nothing/folders"] {
        let said = part_of_a_body(&office, path, 100 * 1024 * 1024, 64 * 1024 + 4096, wait);
        assert_eq!(said, Some(401), "{path} read a stranger's body before turning them away");
    }
}

#[test]
fn guessing_the_join_code_is_limited_from_one_machine_whatever_it_claims_to_be() {
    let office = an_office(); // WHAT: a running office, taking a join code. WHY: set up below.
    office.server.store.set_setting("joining", "code").unwrap(); // WHAT: people join with a code. WHY: the door guessed at.
    office.server.store.set_setting("join_code", "beam-brace-camber-deck-flange-1a2b").unwrap(); // WHAT: the real code. WHY: never guessed here.
    let mut said = Vec::new(); // WHAT: each answer's status. WHY: checked below.
    for n in 0..6 {
        let claimed = format!("203.0.113.{n}"); // WHAT: a different address claimed each time. WHY: what a guesser sends to be counted as somebody new.
        let (status, _) = post(
            &office,
            "/api/v1/join",
            None,
            &[("X-Forwarded-For", &claimed), ("X-Real-IP", &claimed), ("CF-Connecting-IP", &claimed)],
            json!({"code": format!("wrong-guess-{n}"), "name": "Guesser", "email": format!("g{n}@example.com"), "password": PASSWORD}),
        );
        said.push(status);
    }
    // WHAT: three wrong guesses free, then a wait. WHY: before, a made-up address per guess (or no header at all) was never counted, so guessing had no limit.
    assert_eq!(&said[..3], &[403, 403, 403], "{said:?}");
    assert!(said[3..].iter().all(|s| *s == 429), "guessing went on: {said:?}");
}

#[test]
fn guessing_at_invitations_on_a_closed_server_is_limited_too() {
    let office = an_office(); // WHAT: a running office that takes nobody without an invitation. WHY: the door guessed at.
    office.server.store.set_setting("joining", "closed").unwrap(); // WHAT: closed. WHY: an invitation is then the only way in, and a code is a guess at one.
    let mut said = Vec::new(); // WHAT: each answer's status. WHY: checked below.
    for n in 0..6 {
        let (status, _) = post(&office, "/api/v1/join", None, &[], json!({"code": format!("beam-beam-beam-beam-beam-{n:04x}"), "name": "Guesser", "email": format!("g{n}@example.com"), "password": PASSWORD}));
        said.push(status); // WHAT: kept. WHY: checked below.
    }
    // WHAT: three guesses free, then a wait. WHY: before, a closed server cleared the count on every refusal, so invitations could be guessed at without limit.
    assert_eq!(&said[..3], &[403, 403, 403], "{said:?}");
    assert!(said[3..].iter().all(|s| *s == 429), "guessing at invitations went on: {said:?}");
}

#[test]
fn a_key_named_with_a_line_break_is_refused() {
    let office = an_office(); // WHAT: a running office. WHY: a key is asked for.
    let estimator = session(&office, "usr_estimator"); // WHAT: anybody signed in. WHY: everybody may make an assistant key.
    let (status, said) = post(&office, "/api/v1/me/keys", Some(&estimator), &[], json!({"purpose": "assistant", "name": "laptop\n2026-11-02T00:00:00Z INFO sam@example.com turned remote access off"}));
    assert_eq!(status, 400, "{said}"); // WHAT: refused. WHY: before, the name was written into the server's log, line break and all, as a line nobody wrote.
}

#[test]
fn an_address_or_a_name_with_a_line_break_in_it_is_refused() {
    let office = an_office(); // WHAT: a running office, taking anybody. WHY: the join door.
    office.server.store.set_setting("joining", "open").unwrap(); // WHAT: no code needed. WHY: only the name and address are judged.
    for (name, email) in [("Pat", "pat@example.com\nINFO forged line"), ("Pat\nINFO forged line", "pat2@example.com"), ("Pat", "pat @example.com")] {
        let (status, said) = post(&office, "/api/v1/join", None, &[], json!({"code": "", "name": name, "email": email, "password": PASSWORD}));
        assert_eq!(status, 400, "{name:?} {email:?}: {said}"); // WHAT: refused. WHY: before, a line break was written into the server's log as a line of its own.
    }
}

#[test]
fn a_burst_of_sign_ins_sent_together_is_held_to_the_limit() {
    let office = Arc::new(an_office()); // WHAT: a running office. WHY: shared by the threads below.
    let barrier = Arc::new(std::sync::Barrier::new(16)); // WHAT: a starting gate. WHY: all sixteen go together.
    let threads: Vec<_> = (0..16)
        .map(|n| {
            let (office, barrier) = (office.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait(); // WHAT: together. WHY: the burst a script sends.
                post(&office, "/api/v1/auth/token", None, &[], json!({"email": "eve@example.com", "password": format!("wrong-password-{n}")})).0
            })
        })
        .collect();
    let said: Vec<u16> = threads.into_iter().map(|t| t.join().unwrap()).collect(); // WHAT: every answer. WHY: counted.
    let checked = said.iter().filter(|s| **s == 401).count(); // WHAT: guesses that were really checked. WHY: the limit is five.
    // WHAT: at most the five free tries were checked. WHY: before, every try was asked about before any was counted, so a burst got as many guesses as it sent.
    assert!(checked <= 5, "{checked} of 16 guesses were checked: {said:?}");
    assert_eq!(said.len() - checked, said.iter().filter(|s| **s == 429).count(), "the rest were told to wait: {said:?}");
}

#[test]
fn a_flood_of_made_up_names_does_not_reset_the_wait_on_the_account_being_guessed_at() {
    let office = an_office(); // WHAT: a running office. WHY: guessed at.
    let mut said = Vec::new(); // WHAT: the guesser's answers on the real account. WHY: checked below.
    for n in 0..6 {
        said.push(post(&office, "/api/v1/auth/token", None, &[], json!({"email": "eve@example.com", "password": format!("guess-{n}-xxxxxxx")})).0);
    }
    assert_eq!(said[5], 429, "the account owes a wait: {said:?}");
    for n in 0..4200 {
        let _ = post(&office, "/api/v1/auth/token", None, &[], json!({"email": format!("made-up-{n}@example.com"), "password": "anything-at-all"})); // WHAT: names nobody has, each once. WHY: before, this pushed the real account out of the ledger and gave the guesser five more tries.
    }
    let (status, _) = post(&office, "/api/v1/auth/token", None, &[], json!({"email": "eve@example.com", "password": "guess-again-xxxxxx"}));
    assert_eq!(status, 429, "the account's wait was reset by the flood");
}

#[test]
fn signing_in_takes_as_long_for_an_address_with_no_account_as_for_a_wrong_password() {
    let office = an_office(); // WHAT: a running office. WHY: timed.
    let timed = |email: &str| {
        let started = Instant::now(); // WHAT: now. WHY: the answer is timed.
        let (status, _) = post(&office, "/api/v1/auth/token", None, &[], json!({"email": email, "password": "not-the-password-x"}));
        assert_eq!(status, 401); // WHAT: refused either way. WHY: the same answer.
        started.elapsed()
    };
    let known = timed("eve@example.com").min(timed("eve@example.com")); // WHAT: a real account, wrong password, the quicker of two. WHY: noise taken out.
    let unknown = timed("nobody@example.com").min(timed("nobody-else@example.com")); // WHAT: no such account, the quicker of two. WHY: the same.
    // WHAT: within a factor of two. WHY: before, no account answered at once and a real one after the password check, which told anybody which addresses have accounts.
    assert!(unknown * 2 >= known, "no account: {unknown:?}, wrong password: {known:?}");
}

#[test]
fn a_markup_nested_past_any_real_one_is_refused_and_the_server_lives() {
    let office = an_office(); // WHAT: a running office. WHY: sent the markup.
    let estimator = session(&office, "usr_estimator"); // WHAT: an estimator. WHY: who may add markups.
    let (_, project) = post(&office, "/api/v1/projects", Some(&estimator), &[], json!({"number": "26-1", "name": "Deep"}));
    let project = project["id"].as_str().unwrap().to_string(); // WHAT: its id. WHY: a set goes in it.
    let set = upload(&office, &estimator, &project, &a_drawing()); // WHAT: a drawing set. WHY: markups go on it.
    use base64::Engine as _;
    let deep = format!("<</Type/Annot/Subtype/Line/A {}1{}>>", "[".repeat(100_000), "]".repeat(100_000)); // WHAT: an annotation a hundred thousand arrays deep. WHY: before, reading it ran the server out of stack and closed it.
    let (status, _) = post(&office, &format!("/api/v1/sets/{set}/markups"), Some(&estimator), &[], json!([{"page": 0, "dictionary": base64::engine::general_purpose::STANDARD.encode(deep)}]));
    assert_eq!(status, 400, "refused as unreadable");
    let alive = ureq::get(&format!("{}/api/v1/health", office.base)).call(); // WHAT: the server asked again. WHY: it must still be there.
    assert!(alive.is_ok(), "the server did not survive: {alive:?}");
}

#[test]
fn a_drawing_nested_past_any_real_one_goes_up_without_closing_the_server() {
    let office = an_office(); // WHAT: a running office. WHY: sent the drawing.
    let estimator = session(&office, "usr_estimator"); // WHAT: an estimator. WHY: who may upload.
    let (_, project) = post(&office, "/api/v1/projects", Some(&estimator), &[], json!({"number": "26-2", "name": "Deep"}));
    let project = project["id"].as_str().unwrap().to_string(); // WHAT: its id. WHY: the set goes in it.
    let deep = format!("%PDF-1.7\n1 0 obj\n<</Type/Catalog/Pages {}1{}>>\nendobj\ntrailer\n<</Root 1 0 R>>\n%%EOF\n", "[".repeat(100_000), "]".repeat(100_000)); // WHAT: a catalog a hundred thousand arrays deep. WHY: before, reading it ran the reading thread out of stack, which closes the whole server.
    let _ = upload(&office, &estimator, &project, deep.as_bytes()); // WHAT: put up. WHY: read as any upload is.
    let alive = ureq::get(&format!("{}/api/v1/health", office.base)).call(); // WHAT: asked again. WHY: it must still be there.
    assert!(alive.is_ok(), "the server did not survive: {alive:?}");
}

#[test]
fn every_answer_says_not_to_keep_sniff_or_frame_it() {
    let office = an_office(); // WHAT: a running office. WHY: its answers are read.
    let answers = [
        ureq::get(&format!("{}/api/v1/health", office.base)).call().map_err(Box::new), // WHAT: open to anybody. WHY: answers carry the marks.
        ureq::get(&format!("{}/api/version", office.base)).call().map_err(Box::new), // WHAT: Fleet's public door. WHY: the same.
        ureq::get(&format!("{}/api/v1/projects", office.base)).call().map_err(Box::new), // WHAT: a refusal. WHY: refusals carry them too.
        ureq::post(&format!("{}/mcp", office.base)).send_json(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})).map_err(Box::new), // WHAT: the assistant's door. WHY: the same.
    ];
    for answer in answers {
        let response = match answer {
            Ok(r) => r,
            Err(e) => match *e {
                ureq::Error::Status(_, r) => r,
                other => panic!("nothing answered: {other}"),
            },
        }; // WHAT: the answer, whatever its status. WHY: every one is checked.
        assert_eq!(response.header("cache-control"), Some("no-store"), "{}", response.get_url());
        assert_eq!(response.header("x-content-type-options"), Some("nosniff"), "{}", response.get_url());
        assert!(response.header("content-security-policy").is_some_and(|c| c.contains("frame-ancestors 'none'")), "{}", response.get_url());
        assert_eq!(response.header("x-frame-options"), Some("DENY"), "{}", response.get_url());
    }
}

/// What the server writes in its log, kept for the test to read.
#[derive(Clone, Default)]
struct Kept(Arc<std::sync::Mutex<Vec<u8>>>); // WHAT: the log's bytes. WHY: read back by the test.

impl Write for Kept {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes); // WHAT: kept. WHY: read below.
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Kept {
    type Writer = Kept; // WHAT: itself. WHY: every line goes to the same buffer.
    fn make_writer(&'a self) -> Kept {
        self.clone()
    }
}

#[test]
fn a_refused_request_is_written_in_the_log_with_its_door_and_where_it_came_from() {
    let log = Kept::default(); // WHAT: the server's log, in memory. WHY: read below.
    let _ = tracing_subscriber::fmt().with_writer(log.clone()).with_ansi(false).try_init(); // WHAT: the log written there. WHY: as the service writes its file.
    let office = an_office(); // WHAT: a running office. WHY: asked something it refuses.
    let estimator = session(&office, "usr_estimator"); // WHAT: an estimator. WHY: may not read the audit log.
    let status = match ureq::get(&format!("{}/api/v1/audit?since=secret-looking-query", office.base)).set("Authorization", &format!("Bearer {estimator}")).call() {
        Ok(r) => r.status(),
        Err(ureq::Error::Status(code, _)) => code,
        Err(e) => panic!("{e}"),
    }; // WHAT: the audit log asked for by somebody who may not read it. WHY: a refused door.
    assert_eq!(status, 403);
    let written = String::from_utf8_lossy(&log.0.lock().unwrap()).to_string(); // WHAT: what the log holds. WHY: checked.
    assert!(written.contains("refused GET /api/v1/audit from 127.0.0.1"), "the refusal is not in the log: {written}");
    assert!(!written.contains("secret-looking-query"), "the query was written: {written}");
}

/// A small drawing.
fn a_drawing() -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec(); // WHAT: the header. WHY: a PDF.
    let mut offsets = Vec::new(); // WHAT: where each object starts. WHY: the table.
    for body in [
        "1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n",
        "2 0 obj\n<</Type/Pages/Kids[3 0 R]/Count 1>>\nendobj\n",
        "3 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]>>\nendobj\n",
    ] {
        offsets.push(out.len());
        out.extend_from_slice(body.as_bytes());
    }
    let xref = out.len(); // WHAT: where the table starts. WHY: written last.
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).as_bytes());
    out
}

/// Puts `bytes` up as a drawing set: its id, or empty when refused.
fn upload(office: &Office, token: &str, project: &str, bytes: &[u8]) -> String {
    let boundary = "----asvs"; // WHAT: the boundary. WHY: in no part.
    let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"project\"\r\n\r\n{project}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"S-101.pdf\"\r\n\r\n").into_bytes();
    body.extend_from_slice(bytes); // WHAT: the drawing. WHY: the upload.
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let answer = ureq::post(&format!("{}/api/v1/sets", office.base))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body); // WHAT: sent. WHY: as a seat sends one.
    match answer {
        Ok(r) => r.into_json::<Value>().unwrap()["id"].as_str().unwrap_or_default().to_string(),
        Err(_) => String::new(), // WHAT: refused or failed. WHY: the caller checks the server is alive.
    }
}
