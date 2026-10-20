//! Letting an assistant ask about the quantities.
//!
//! A takeoff is a pile of numbers that somebody has to turn into sentences —
//! how much steel is on this job, what did the last revision cost, which
//! shapes have no weight against them. Those are questions, and the shop
//! already has something that answers questions. This puts the takeoff where
//! it can be asked.
//!
//! It speaks MCP over HTTP: JSON-RPC 2.0 at `/mcp`, with the same bearer token
//! as the rest of the API. Point an AI assistant or anything else that speaks MCP at a
//! shop's own server and it can read that shop's quantities. Bluebeam has
//! nothing of the kind, and could not easily: the numbers only exist inside
//! its own window.
//!
//! # The two rules this endpoint is built on
//!
//! **Everything here is read-only.** There is no tool that draws a markup,
//! changes a scale, edits a takeoff or uploads anything. That is not an
//! oversight to be filled in later — it is the point. A program whose whole
//! argument is that every quantity traces back to something a person clicked
//! must not grow a door through which a language model can click. A test at
//! the bottom of this file fails if a write tool ever appears here.
//!
//! **It answers with the same warnings a person would see.** A takeoff that
//! is short three sheets for want of a scale says so, in the answer, in
//! words — not as a field somebody might not read. An assistant summarising
//! this for somebody has to be told what it does not know, or it will
//! confidently report a total that is missing a floor.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::api::Shared;

/// The versions of the protocol this speaks, newest first. A client asking
/// for one of these gets that one back; asking for anything else, it is told
/// the newest and decides for itself — which is what the protocol asks of a
/// server. Answering a client's request for an older version with a newer one
/// is how a connector ends up refused by the client it was written for.
pub const PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// The version to answer an `initialize` with.
pub fn protocol_for(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|r| PROTOCOLS.iter().find(|p| **p == r).copied())
        .unwrap_or(PROTOCOLS[0])
}

pub fn router(server: Shared) -> Router {
    Router::new()
        .route("/mcp", post(talk))
        .with_state(server)
}

/// One JSON-RPC exchange.
async fn talk(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let id = body.get("id").cloned();
    let method = body
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or_default()
        .to_string();

    // A notification has no id and takes no answer.
    if id.is_none() {
        return (StatusCode::ACCEPTED, Json(json!({}))).into_response();
    }

    let result = match method.as_str() {
        "initialize" => Ok(json!({
            "protocolVersion": protocol_for(
                body.pointer("/params/protocolVersion").and_then(|v| v.as_str())
            ),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": crate::api::installation_name(&server),
                "title": "Excalibur View",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": INSTRUCTIONS,
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call(&server, &headers, &body),
        other => Err(Trouble::no_such_method(other)),
    };

    let answer = match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(trouble) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": trouble.code, "message": trouble.message },
        }),
    };
    (StatusCode::OK, Json(answer)).into_response()
}

pub const INSTRUCTIONS: &str = "\
These are the drawings, markups and takeoffs held on one company's Excalibur View \
server. Everything here is read-only.

Nothing in this takeoff is estimated: every quantity traces back to a markup \
somebody drew against a scale somebody set. Two things follow that you must \
carry into anything you say about these numbers.

A measurement on a sheet with no scale is LEFT OUT of every total and \
reported separately. It is never counted as zero. When an answer says \
measurements were left out, the totals in it are short, and you must say so \
rather than presenting them as the quantity.

A shape with no unit weight in the tool chest has NO weight, which is not a \
weight of zero. Do not fill one in from a section table, your own knowledge, \
or anywhere else. The weights here are this company's own numbers.";

struct Trouble {
    code: i64,
    message: String,
}

impl Trouble {
    fn no_such_method(name: &str) -> Trouble {
        Trouble {
            code: -32601,
            message: format!("this server has no method called {name}"),
        }
    }
    fn bad(message: impl Into<String>) -> Trouble {
        Trouble {
            code: -32602,
            message: message.into(),
        }
    }
    fn refused(message: impl Into<String>) -> Trouble {
        Trouble {
            code: -32001,
            message: message.into(),
        }
    }
}

/// Every tool, and what it takes.
///
/// All of them read. None of them write. See the note at the top of the file.
pub fn tools() -> Vec<Value> {
    let set_argument = json!({
        "type": "object",
        "properties": {
            "set": { "type": "string", "description": "The drawing set's id, from list_sets." }
        },
        "required": ["set"],
    });
    vec![
        json!({
            "name": "list_projects",
            "title": "List the jobs",
            "description": "The jobs on this server.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "list_sets",
            "title": "List the drawing sets",
            "description": "The drawing sets on this server, newest first. A set replaced by a newer \
                            issue of the same drawing is listed apart and is not the job's quantities; \
                            takeoff carried forward onto a new issue is flagged until somebody checks it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {
                        "type": "string",
                        "description": "Only the sets on this job: its id, or its job number. Leave it out for all of them.",
                    }
                },
            },
        }),
        json!({
            "name": "takeoff",
            "title": "The quantities on a set",
            "description": "What has been taken off a drawing set, totalled by subject. \
                            Says plainly what was left out for want of a scale.",
            "inputSchema": set_argument.clone(),
        }),
        json!({
            "name": "shop_list",
            "title": "The cut list",
            "description": "The takeoff as a cut list: every length grouped by shape and \
                            rounded UP to a cutting increment, with what it weighs. Give a \
                            stock length and it also says how many sticks to buy and what \
                            the drop is. There is no default stock length.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "set": { "type": "string", "description": "The drawing set's id." },
                    "stock_length_feet": {
                        "type": "number",
                        "description": "The stock length the shop buys, in feet. Leave it \
                                        out and nothing is nested — do not invent one.",
                    },
                },
                "required": ["set"],
            }),
        }),
        json!({
            "name": "counted_twice",
            "title": "Find things counted twice",
            "description": "Markups sitting on top of each other measuring the same thing \
                            on one sheet. It reports; it never removes anything, and the \
                            totals are untouched.",
            "inputSchema": set_argument.clone(),
        }),
        json!({
            "name": "revision_cost",
            "title": "What a revision cost",
            "description": "Compares the takeoffs on two drawing sets and says what moved \
                            and what it is worth.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "was_bid": { "type": "string", "description": "The set that was bid." },
                    "arrived": { "type": "string", "description": "The set that arrived." },
                },
                "required": ["was_bid", "arrived"],
            }),
        }),
        json!({
            "name": "sheets",
            "title": "The sheets in a set",
            "description": "Every sheet, its number and title, and whether it has a scale \
                            set. A sheet with no scale is a sheet whose measurements are \
                            not in any total.",
            "inputSchema": set_argument,
        }),
    ]
}

fn call(server: &crate::api::Server, headers: &HeaderMap, body: &Value) -> Result<Value, Trouble> {
    let params = body.get("params").cloned().unwrap_or(json!({}));
    let name = params
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| Trouble::bad("that call did not say which tool"))?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    // Same token as the rest of the API. An assistant gets exactly what the
    // person whose token it is would get, and nothing else: the person is
    // kept and handed to every tool, which applies the API's own project gate.
    let who = crate::api::mcp_caller(server, headers) // WHAT: whoever the token belongs to. WHY: kept, not thrown away: every answer is filtered by it.
        .map_err(|_| Trouble::refused(
            "this server does not know who is asking. Put an Excalibur View access token \
             in an Authorization: Bearer header.",
        ))?;

    let said = match name {
        "list_projects" => crate::api::mcp_projects(server, &who), // WHAT: the jobs this person may see. WHY: never every job on the server.
        "list_sets" => crate::api::mcp_sets(server, &who, args.get("project").and_then(|p| p.as_str())), // WHAT: sets on jobs this person may see. WHY: the same rule as the list.
        "takeoff" => crate::api::mcp_takeoff(server, &who, need(&args, "set")?), // WHAT: a set's takeoff, gated. WHY: as GET /sets/{id}/takeoff is.
        "shop_list" => crate::api::mcp_shop_list(
            server,
            &who, // WHAT: the person asking. WHY: the cut list is gated like the takeoff it comes from.
            need(&args, "set")?,
            args.get("stock_length_feet").and_then(|v| v.as_f64()),
        ),
        "counted_twice" => crate::api::mcp_doubles(server, &who, need(&args, "set")?), // WHAT: doubles on a set, gated. WHY: reading markups is reading the set.
        "revision_cost" => crate::api::mcp_revision(
            server,
            &who, // WHAT: the person asking. WHY: both sets compared are gated.
            need(&args, "was_bid")?,
            need(&args, "arrived")?,
        ),
        "sheets" => crate::api::mcp_sheets(server, &who, need(&args, "set")?), // WHAT: a set's sheets, gated. WHY: as GET /sets/{id}/sheets is.
        other => return Err(Trouble::bad(format!("there is no tool called {other}"))),
    };

    match said {
        Ok(text) => Ok(json!({
            "content": [{ "type": "text", "text": text }],
            "isError": false,
        })),
        // A tool that could not answer says so as a tool result rather than a
        // protocol error, which is what lets the assistant tell the person
        // what went wrong instead of falling over.
        Err(why) => Ok(json!({
            "content": [{ "type": "text", "text": why }],
            "isError": true,
        })),
    }
}

fn need<'a>(args: &'a Value, key: &str) -> Result<&'a str, Trouble> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| Trouble::bad(format!("that call needs a {key}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_here_can_change_a_drawing() {
        // The rule this endpoint exists under. A program whose whole argument
        // is that every quantity traces back to something a person clicked
        // must not grow a door a language model can click through.
        for tool in tools() {
            let name = tool["name"].as_str().unwrap_or_default().to_string();
            let description = tool["description"].as_str().unwrap_or_default().to_lowercase();
            for verb in [
                "add", "create", "delete", "remove", "upload", "write", "set_",
                "edit", "change", "update", "place", "draw", "save", "import",
            ] {
                assert!(
                    !name.starts_with(verb),
                    "{name} reads like a tool that changes something"
                );
            }
            // And the schemas take ids and numbers, never a markup to store.
            let schema = tool["inputSchema"].to_string();
            assert!(
                !schema.contains("markup") && !schema.contains("dictionary"),
                "{name} takes something that looks like a markup to write"
            );
            assert!(!description.is_empty(), "{name} does not say what it does");
        }
    }

    #[test]
    fn every_tool_says_what_it_needs() {
        for tool in tools() {
            let schema = &tool["inputSchema"];
            assert_eq!(
                schema["type"], "object",
                "{} has no input schema",
                tool["name"]
            );
            // Anything with a required argument must also describe it, or the
            // assistant has to guess what to put in it.
            if let Some(required) = schema["required"].as_array() {
                for key in required {
                    let key = key.as_str().unwrap_or_default();
                    assert!(
                        schema["properties"][key]["description"].is_string(),
                        "{} requires {key} without saying what it is",
                        tool["name"]
                    );
                }
            }
        }
    }

    #[test]
    fn the_instructions_carry_the_rule_the_whole_program_is_built_on() {
        // An assistant reading these numbers has to be told what they do not
        // include, or it will confidently report a total missing a floor.
        assert!(INSTRUCTIONS.contains("never counted as zero"));
        assert!(INSTRUCTIONS.contains("NO weight"));
        assert!(INSTRUCTIONS.contains("read-only"));
    }

    #[test]
    fn a_call_with_no_tool_named_is_refused_rather_than_guessed_at() {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                           "params": { "arguments": {} } });
        let params = body.get("params").cloned().unwrap();
        assert!(params.get("name").is_none());
    }

    #[test]
    fn a_client_asking_for_a_version_this_speaks_gets_that_version() {
        assert_eq!(protocol_for(Some("2024-11-05")), "2024-11-05");
        assert_eq!(protocol_for(Some("2025-06-18")), "2025-06-18");
        assert_eq!(protocol_for(Some("2099-01-01")), PROTOCOLS[0]);
        assert_eq!(protocol_for(None), PROTOCOLS[0]);
    }

    #[test]
    fn an_argument_that_is_blank_is_missing_rather_than_empty() {
        let args = json!({ "set": "   ", "other": "ok" });
        assert!(need(&args, "set").is_err());
        assert!(need(&args, "missing").is_err());
        assert_eq!(need(&args, "other").ok(), Some("ok"));
    }
}
