//! The MCP endpoint, talked to the way an assistant talks to it.
//!
//! These matter more than most wire tests, because the reader on the other end
//! is a language model writing sentences for somebody who will price a job off
//! them. Two things have to survive the trip:
//!
//! * a takeoff that is short says so, in words, in the answer; and
//! * a shape with no unit weight comes back with no weight, never a zero.
//!
//! And the endpoint has to stay read-only. There is no tool here that changes
//! a drawing, and `tools/list` is checked for that on every run.

use std::sync::Arc;

use hyperview_server::api::{self, Server};
use hyperview_server::{auth, Config, Store};
use rusqlite::params;
use serde_json::{json, Value};

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let n: u64 = rand::random();
        let path = std::env::temp_dir().join(format!("hyperview-mcp-{n:016x}"));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Running {
    base: String,
    scratch: Scratch,
    _runtime: tokio::runtime::Runtime,
}

fn start() -> Running {
    let scratch = Scratch::new();
    let config = Config {
        name: "Example Steel".into(),
        data: scratch.0.clone(),
        listen: "127.0.0.1:0".parse().unwrap(),
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
                params![
                    api::fresh_id("usr"),
                    "Sam",
                    "sam@example.com",
                    hashed,
                    "admin",
                    now
                ],
            )?;
            Ok(())
        })
        .expect("an administrator");

    let runtime = tokio::runtime::Builder::new_multi_thread()
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
        scratch,
        _runtime: runtime,
    }
}

/// One JSON-RPC call, with a token or without one.
fn rpc(server: &Running, token: Option<&str>, method: &str, params: Value) -> Value {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let mut request = ureq::post(&format!("{}/mcp", server.base))
        .set("Content-Type", "application/json");
    if let Some(token) = token {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    request
        .send_json(body)
        .expect("the endpoint answered")
        .into_json()
        .expect("json came back")
}

/// The text of a tool's answer, and whether it was an error.
fn said(answer: &Value) -> (String, bool) {
    let result = &answer["result"];
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    (text.to_string(), result["isError"].as_bool().unwrap_or(false))
}

fn token(server: &Running) -> String {
    let client = hub::Client::new(&server.base);
    let mut client = client;
    client
        .sign_in("sam@example.com", "brace-gusset-purlin-shim-42")
        .expect("sign in");
    client.token().expect("a token").to_string()
}

fn a_beam(length_points: f64, subject: &str, weight: &str) -> String {
    use base64::Engine;
    let mut m = annot::Markup::new(annot::Subtype::Line);
    m.set("IT", pdf::Object::name("LineDimension"));
    m.set_subject(subject);
    m.set(
        "BSIColumnData",
        pdf::Object::Array(vec![
            pdf::Object::text(weight),
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

fn a_drawing() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets = Vec::new();
    let mut add = |out: &mut Vec<u8>, body: &str| {
        offsets.push(out.len());
        out.extend_from_slice(body.as_bytes());
    };
    add(&mut out, "1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n");
    add(&mut out, "2 0 obj\n<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>\nendobj\n");
    add(&mut out, "3 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]>>\nendobj\n");
    add(&mut out, "4 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]>>\nendobj\n");
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
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

fn upload(server: &Running, token: &str, project: &str) -> hub::DrawingSet {
    let bytes = a_drawing();
    let boundary = "----hyperviewmcp";
    let mut body: Vec<u8> = Vec::new();
    for (name, value) in [("project", project), ("name", "Structural")] {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"structural.pdf\"\r\nContent-Type: application/pdf\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let answer = ureq::post(&format!("{}/api/v1/sets", server.base))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body)
        .expect("upload");
    answer.into_json().expect("a drawing set came back")
}

fn set_scale(server: &Running, set: &str, page: usize, scale: Option<&str>) {
    let path = std::path::Path::new(&server.scratch.0).join("hyperview.sqlite");
    let db = rusqlite::Connection::open(path).unwrap();
    let changed = db
        .execute(
            "UPDATE sheets SET scale = ?1 WHERE set_id = ?2 AND page = ?3",
            params![scale, set, i64::try_from(page).unwrap()], // WHAT: the scale, set and page. WHY: the page as SQLite keeps it
        )
        .unwrap();
    // SQLite will happily read a mistyped column name as a string literal and
    // update nothing, which is how this test silently tested the wrong thing
    // the first time it was written.
    assert_eq!(changed, 1, "no sheet was updated — the scale did not take");
}

/// A server with a job, a set, a scale on sheet one and two beams on it.
fn a_job(server: &Running) -> (String, String) {
    let token = token(server);
    let mut client = hub::Client::new(&server.base);
    client
        .sign_in("sam@example.com", "brace-gusset-purlin-shim-42")
        .expect("sign in");
    let project = client.create_project("6742", "Elm West").expect("project");
    let set = upload(server, &token, &project.id);
    set_scale(server, &set.id, 0, Some("1/4\" = 1'-0\""));
    client
        .push_markups(
            &set.id,
            &[
                hub::NewMarkup { page: 0, dictionary: a_beam(720.0, "W12x26", "26"), replaces: None },
                hub::NewMarkup { page: 0, dictionary: a_beam(720.0, "W12x26", "26"), replaces: None },
            ],
        )
        .expect("push");
    (token, set.id)
}

// ---- the protocol ----------------------------------------------------------

#[test]
fn it_introduces_itself_the_way_an_mcp_client_expects() {
    let server = start();
    let answer = rpc(&server, None, "initialize", json!({}));
    assert_eq!(answer["jsonrpc"], "2.0");
    let result = &answer["result"];
    assert!(result["protocolVersion"].is_string());
    assert_eq!(result["serverInfo"]["title"], "Excalibur View");
    assert_eq!(result["serverInfo"]["name"], "Example Steel", "the shop's own name");
    assert!(result["capabilities"]["tools"].is_object());
    // The instructions carry the rule everything here is built on, because the
    // thing reading them is going to write sentences somebody prices a job off.
    let instructions = result["instructions"].as_str().unwrap_or_default();
    assert!(instructions.contains("never counted as zero"), "{instructions}");
    assert!(instructions.contains("read-only"));
}

#[test]
fn the_tools_are_all_read_only() {
    // The contract. There is no door here a language model can click through.
    let server = start();
    let tools = rpc(&server, None, "tools/list", json!({}));
    let list = tools["result"]["tools"].as_array().expect("a list of tools");
    assert!(list.len() >= 6, "{list:?}");
    for tool in list {
        let name = tool["name"].as_str().unwrap_or_default();
        for verb in ["add", "create", "delete", "remove", "upload", "write", "edit", "place"] {
            assert!(!name.starts_with(verb), "{name} looks like it changes something");
        }
        assert!(tool["description"].is_string(), "{name} says nothing about itself");
    }
    let names: Vec<&str> = list.iter().filter_map(|t| t["name"].as_str()).collect();
    for wanted in ["takeoff", "shop_list", "counted_twice", "revision_cost", "sheets"] {
        assert!(names.contains(&wanted), "{wanted} is missing from {names:?}");
    }
}

#[test]
fn a_method_this_server_does_not_have_is_refused_politely() {
    let server = start();
    let answer = rpc(&server, None, "resources/list", json!({}));
    assert_eq!(answer["error"]["code"], -32601);
    assert!(answer["result"].is_null());
}

// ---- who may ask -----------------------------------------------------------

#[test]
fn nobody_gets_the_quantities_without_a_token() {
    // The drawings are somebody's project under somebody's contract. An
    // assistant gets exactly what the person whose token it is would get.
    let server = start();
    let (_, set) = a_job(&server);
    let answer = rpc(
        &server,
        None,
        "tools/call",
        json!({ "name": "takeoff", "arguments": { "set": set } }),
    );
    assert_eq!(answer["error"]["code"], -32001);
    let message = answer["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("Bearer"), "it should say how: {message}");
}

#[test]
fn a_bad_token_is_no_better_than_none() {
    let server = start();
    let (_, set) = a_job(&server);
    let answer = rpc(
        &server,
        Some("not-a-real-token"),
        "tools/call",
        json!({ "name": "takeoff", "arguments": { "set": set } }),
    );
    assert_eq!(answer["error"]["code"], -32001);
}

// ---- the answers -----------------------------------------------------------

#[test]
fn the_takeoff_comes_back_as_something_worth_reading_aloud() {
    let server = start();
    let (token, set) = a_job(&server);
    let answer = rpc(
        &server,
        Some(&token),
        "tools/call",
        json!({ "name": "takeoff", "arguments": { "set": set } }),
    );
    let (text, error) = said(&answer);
    assert!(!error, "{text}");
    assert!(text.contains("W12x26"), "{text}");
    // Eighty feet of a twenty-six pound section, off his own tool chest.
    assert!(text.contains("2080 lb"), "{text}");
    assert!(text.contains("1.040 tons"), "{text}");
}

#[test]
fn a_takeoff_that_is_short_says_so_in_words() {
    // The one that matters most. A model summarising this for somebody has to
    // be told what it does not know, or it reports a total missing a floor.
    let server = start();
    let (token, set) = a_job(&server);
    // Take the scale off, so both beams fall out of the totals.
    set_scale(&server, &set, 0, None);
    let answer = rpc(
        &server,
        Some(&token),
        "tools/call",
        json!({ "name": "takeoff", "arguments": { "set": set } }),
    );
    let (text, _) = said(&answer);
    assert!(text.contains("SHORT:"), "{text}");
    assert!(text.contains("no scale"), "{text}");
    assert!(text.contains("not counted as zero"), "{text}");
    assert!(text.contains("you must say so"), "{text}");
}

#[test]
fn a_shape_with_no_unit_weight_never_comes_back_as_zero() {
    let server = start();
    let token = token(&server);
    let mut client = hub::Client::new(&server.base);
    client
        .sign_in("sam@example.com", "brace-gusset-purlin-shim-42")
        .expect("sign in");
    let project = client.create_project("6742", "Elm West").expect("project");
    let set = upload(&server, &token, &project.id);
    set_scale(&server, &set.id, 0, Some("1/4\" = 1'-0\""));
    client
        .push_markups(
            &set.id,
            &[hub::NewMarkup {
                page: 0,
                dictionary: a_beam(720.0, "SOMETHING ODD", ""),
                replaces: None,
            }],
        )
        .expect("push");

    let answer = rpc(
        &server,
        Some(&token),
        "tools/call",
        json!({ "name": "takeoff", "arguments": { "set": set.id } }),
    );
    let (text, _) = said(&answer);
    assert!(text.contains("NO unit weight"), "{text}");
    assert!(text.contains("not a weight of zero"), "{text}");
    assert!(!text.contains("0 lb"), "a zero weight was reported: {text}");
}

#[test]
fn the_cut_list_refuses_to_pick_a_stock_length() {
    let server = start();
    let (token, set) = a_job(&server);
    let answer = rpc(
        &server,
        Some(&token),
        "tools/call",
        json!({ "name": "shop_list", "arguments": { "set": set } }),
    );
    let (text, error) = said(&answer);
    assert!(!error, "{text}");
    assert!(text.contains("W12x26"), "{text}");
    assert!(text.contains("40.000 ft"), "{text}");
    assert!(text.contains("× 2"), "two of them: {text}");
    // And it tells the assistant not to invent one either.
    assert!(text.contains("no stock length was given"), "{text}");
    assert!(text.contains("Do not pick"), "{text}");
}

#[test]
fn a_stock_length_that_was_given_gets_nested() {
    let server = start();
    let (token, set) = a_job(&server);
    let answer = rpc(
        &server,
        Some(&token),
        "tools/call",
        json!({ "name": "shop_list",
                "arguments": { "set": set, "stock_length_feet": 40.0 } }),
    );
    let (text, _) = said(&answer);
    assert!(text.contains("WHAT TO BUY"), "{text}");
    // Two forty-foot pieces out of forty-foot stock is two sticks.
    assert!(text.contains("2 stick(s)"), "{text}");
}

#[test]
fn the_double_check_says_plainly_that_it_changed_nothing() {
    let server = start();
    let (token, set) = a_job(&server);
    // The two beams in `a_job` are the same line drawn twice.
    let answer = rpc(
        &server,
        Some(&token),
        "tools/call",
        json!({ "name": "counted_twice", "arguments": { "set": set } }),
    );
    let (text, _) = said(&answer);
    assert!(text.contains("NOTHING HAS BEEN CHANGED"), "{text}");
    assert!(text.contains("Do not recommend deleting"), "{text}");
}

#[test]
fn the_sheets_answer_names_the_ones_with_no_scale() {
    let server = start();
    let (token, set) = a_job(&server);
    let answer = rpc(
        &server,
        Some(&token),
        "tools/call",
        json!({ "name": "sheets", "arguments": { "set": set } }),
    );
    let (text, _) = said(&answer);
    assert!(text.contains("NO SCALE SET"), "sheet two has none: {text}");
    assert!(text.contains("never counted as zero"), "{text}");
}

#[test]
fn asking_about_a_set_that_is_not_here_is_a_tool_error_not_a_crash() {
    let server = start();
    let (token, _) = a_job(&server);
    let answer = rpc(
        &server,
        Some(&token),
        "tools/call",
        json!({ "name": "takeoff", "arguments": { "set": "set_nonsense" } }),
    );
    let (text, error) = said(&answer);
    assert!(error, "{text}");
    assert!(text.contains("no drawing set"), "{text}");
}

#[test]
fn a_tool_that_does_not_exist_is_refused() {
    let server = start();
    let (token, _) = a_job(&server);
    let answer = rpc(
        &server,
        Some(&token),
        "tools/call",
        json!({ "name": "delete_everything", "arguments": {} }),
    );
    assert_eq!(answer["error"]["code"], -32602);
}
