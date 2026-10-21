//! The assistant, on this computer: reading the office's takeoffs, and working the
//! Hyperview window alongside the person at it.
//!
//! The assistant app runs its connectors as programs on the same computer and
//! talks to them over standard input and output — so this program is that
//! connector too. Started with `--mcp`, Hyperview has no window. It answers
//! the assistant with two kinds of tool:
//!
//! - **The office's**, passed through to the office server's `/mcp` with a key
//!   that can do nothing but ask: the jobs, the sets, the quantities, what a
//!   revision cost. Read-only, by design (see `hyperview_server::mcp`).
//! - **The window's**, answered through the desk ([`crate::desk`]): what is
//!   open, the words and the picture of a sheet, turning to a sheet, pointing
//!   at markups, running the office's checks — and *proposing* markups, which
//!   appear in the window with a Fix button and are drawn only when the person
//!   clicks it.
//!
//! "Connect Assistant" in the Help menu does the setting up: it asks the server
//! for the key, keeps it beside the program, and adds Hyperview to the assistant
//! app's list of connectors — both places the assistant app keeps that list
//! on Windows, because the Store version reads a different one from the one
//! its own Edit Config button opens. Nobody edits a file.
//!
//! # When it does not work
//!
//! Every start of the connector, and every question the assistant asks it, is
//! written to `assistant-bridge.log` beside the program. "Test Assistant Connection"
//! in the Help menu reads that log, checks both settings files, and runs the
//! connector itself the way the assistant would — so the answer to "it didn't seem to
//! work" is a report, not a guess. The three things that actually go wrong:
//! The assistant app was not quit from the tray and reopened (it reads its list
//! only when it starts), a settings file it could not read (a byte-order mark
//! left by Notepad or PowerShell), and a kept-alive connection to an office
//! server that has since restarted.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::desk;

/// What `--mcp` needs: where the server is, and the key.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Connection {
    pub base: String,
    pub key: String,
    /// What the server calls itself, for the error messages.
    #[serde(default)]
    pub name: String,
}

/// The name Hyperview goes by in the assistant app's list.
pub const SERVER_NAME: &str = "excalibur-hyperview";

// Where the assistant app this connects to keeps its list of connectors, and
// what its process is called. These are its names, not ours.
// WHAT: the folder name the desktop assistant app keeps its settings under. WHY: it is that app's own name on disk, so it must be written exactly as the app writes it or its list of connectors is never found.
const APP_FOLDER: &str = "Claude";
// WHAT: the desktop assistant app's settings file name. WHY: chosen by that app, not by us; any other name is a file it never reads.
const SETTINGS_FILE: &str = "claude_desktop_config.json";
// WHAT: how the Microsoft Store copies of the assistant app name their package folders. WHY: the Store version keeps its own settings file there, and these prefixes are the only way to find it.
const STORE_PACKAGES: [&str; 2] = ["Claude_", "AnthropicClaude"];
// WHAT: the running assistant app's process name. WHY: used only to tell the person it must be quit and reopened; the name is the app's own.
const PROCESS: &str = "claude";

fn home_folder() -> Option<PathBuf> {
    crate::install::home().or_else(|| {
        directories::ProjectDirs::from("com", "Excalibur", "Hyperview").map(|d| d.data_local_dir().to_path_buf())
    })
}

fn connection_file() -> Option<PathBuf> {
    home_folder().map(|home| home.join("assistant.json"))
}

pub fn log_file() -> Option<PathBuf> {
    home_folder().map(|home| home.join("assistant-bridge.log"))
}

pub fn keep(connection: &Connection) -> Result<(), String> {
    let path = connection_file().ok_or("there is no app data folder to keep the key in")?;
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(connection).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn kept() -> Option<Connection> {
    let text = std::fs::read_to_string(connection_file()?).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

// ---- the assistant app's settings -------------------------------------------------

/// Every assistant app settings file on this computer, whether it exists
/// yet or not, for every copy of the assistant app that is installed.
pub fn assistant_app_settings() -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Some(roaming) = std::env::var_os("APPDATA").map(PathBuf::from) {
        let folder = roaming.join(APP_FOLDER);
        if folder.is_dir() {
            found.push(folder.join(SETTINGS_FILE));
        }
    }
    // The Microsoft Store version keeps its own, in its package's folder.
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        if let Ok(entries) = std::fs::read_dir(local.join("Packages")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if STORE_PACKAGES.iter().any(|p| name.starts_with(p)) {
                    let folder = entry.path().join("LocalCache").join("Roaming").join(APP_FOLDER);
                    if folder.is_dir() {
                        found.push(folder.join(SETTINGS_FILE));
                    }
                }
            }
        }
    }
    found
}

/// A settings file as JSON. A byte-order mark — what Notepad and Windows
/// PowerShell 5 put at the front of a file they save as UTF-8 — is read past
/// rather than taken for the file being broken, which is what made Connect
/// Assistant leave the one file that mattered alone.
pub fn read_settings(settings: &Path) -> Result<Value, String> {
    let text = match std::fs::read(settings) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
        Err(_) => return Ok(json!({})),
    };
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(text).map_err(|e| {
        format!("{} is not readable as settings ({e}), so it has been left alone", settings.display())
    })
}

/// Puts Hyperview on one assistant app settings file's list of connectors,
/// leaving everything else in it exactly as it was, then reads it back to
/// make sure it says so.
pub fn add_to(settings: &Path, program: &Path) -> Result<(), String> {
    let mut config = read_settings(settings)?;
    if !config.is_object() {
        return Err(format!("{} is not a settings file, so it has been left alone", settings.display()));
    }
    let servers = config
        .as_object_mut()
        .expect("checked above")
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    servers[SERVER_NAME] = json!({
        "command": program.to_string_lossy(),
        "args": ["--mcp"],
    });
    // The one before is kept, once, in case anybody wants it back.
    let backup = settings.with_extension("json.before-hyperview");
    if settings.exists() && !backup.exists() {
        let _ = std::fs::copy(settings, &backup);
    }
    // Written without a byte-order mark: that is the form every reader takes.
    let text = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(settings, text).map_err(|e| format!("{}: {e}", settings.display()))?;
    match lists_us(settings) {
        Some(true) => Ok(()),
        _ => Err(format!("{} was written but does not read back with Excalibur View on it", settings.display())),
    }
}

/// Whether a settings file lists this connector, pointing at a program that
/// is there. `None` when the file cannot be read at all.
pub fn lists_us(settings: &Path) -> Option<bool> {
    let config = read_settings(settings).ok()?;
    let entry = &config["mcpServers"][SERVER_NAME];
    let command = entry["command"].as_str()?;
    Some(Path::new(command).exists() && entry["args"][0] == "--mcp")
}

/// Adds Hyperview to every assistant app on this computer. Says which.
pub fn add_to_assistant_app() -> Result<Vec<PathBuf>, String> {
    let program = crate::install::installed_program()
        .filter(|p| p.exists())
        .or_else(|| std::env::current_exe().ok())
        .ok_or("where Excalibur View is installed could not be found")?;
    let places = assistant_app_settings();
    if places.is_empty() {
        // WHAT: says no assistant app is installed and names the Help command to run again. WHY: the command is now called Connect Assistant, so the message names it the way the menu does.
        return Err("No assistant app that takes connectors is installed on this computer. \
                    Install one, open it once, then choose Connect Assistant again."
            .into());
    }
    let mut done = Vec::new();
    let mut trouble = Vec::new();
    for place in places {
        match add_to(&place, &program) {
            Ok(()) => done.push(place),
            Err(why) => trouble.push(why),
        }
    }
    if done.is_empty() {
        return Err(trouble.join("; "));
    }
    Ok(done)
}

// ---- the log ---------------------------------------------------------------------

/// `assistant-bridge.log`: when the assistant started the connector, what it asked,
/// what went wrong. Never a key, never what a drawing says.
pub struct BridgeLog {
    path: Option<PathBuf>,
}

impl BridgeLog {
    pub fn open() -> BridgeLog {
        let path = log_file();
        if let Some(p) = &path {
            if std::fs::metadata(p).map(|m| m.len() > 512 * 1024).unwrap_or(false) {
                let _ = std::fs::rename(p, p.with_extension("old.log"));
            }
        }
        BridgeLog { path }
    }

    pub fn line(&self, what: impl AsRef<str>) {
        let Some(path) = &self.path else { return };
        let stamp = hyperview_server::api::now();
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{stamp} [{}] {}", std::process::id(), what.as_ref());
        }
    }
}

// ---- --mcp: the connector itself ---------------------------------------------------

/// Said first when there is no office server: the window is everything.
const WINDOW_ALONE: &str = "\
You are connected to Excalibur View, a drawing viewer, markup and takeoff \
program, on this person's computer. It is not connected to an office server, \
so there are no shared jobs to read: work from the drawings open in the \
window. takeoff_totals gives the totals of the drawing open, the same numbers \
as the markups list the person sees. Numbers come only from what was measured; \
never estimate a quantity or a weight.";

const WINDOW_INSTRUCTIONS: &str = "\
You are also connected to the Excalibur View window on this person's computer. \
window_status says what they have open. read_sheet gives the words on a sheet \
and look_at_sheet a picture of it; ask for an area to read detail, because a \
whole large sheet in one picture is too small to read. Coordinates everywhere \
are fractions of the sheet from its top-left corner. show and go_to move what \
the person sees, so you can point at what you mean. run_check runs the \
office's plugin checks.

You cannot draw or change anything in a drawing. propose_markups puts \
proposals in front of the person, each with a Fix button, and nothing is drawn \
or changed until they click it. Propose; do not claim to have marked anything \
up. Use the subjects of the office's tool chest (W12x26, Moment Conn) so the \
weights come with them, and say in `why` what you saw that makes you propose it.

A sheet with no scale has no measurements in any total. When window_status \
says the sheet has none and the title block or view title gives one, propose \
the scale (kind scale) before proposing lengths or areas on that sheet. A wrong \
subject, quantity or slope on a markup already drawn is proposed the same way, \
by the markup's id from markups_on_sheet.";

/// Runs as the assistant app's connector until the assistant closes the pipe. Returns the
/// exit code. Nothing but answers ever goes to standard output.
pub fn serve() -> i32 {
    let log = BridgeLog::open();
    log.line(format!(
        "The assistant started the connector: Excalibur View {} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default()
    ));
    let connection = kept();
    log.line(match &connection {
        Some(c) => format!("office server: {} {}", c.name, c.base),
        None => "no office server kept on this computer (Help → Connect Assistant makes one)".into(), // WHAT: the log line when no office server is kept. WHY: names the Help command by its menu name, so the log points at something the person can find.
    });
    let agent = office_agent();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                log.line("The assistant sent something that was not JSON");
                reply(&stdout, &failure(Value::Null, -32700, "That was not JSON."));
                continue;
            }
        };
        if let Some(answer) = answer(&agent, connection.as_ref(), &request, &log) {
            reply(&stdout, &answer);
        }
    }
    log.line("The assistant closed the connection");
    0
}

/// Without kept-alive connections: a pooled socket to an office server that
/// has since restarted — it updates itself — fails as "connection forcibly
/// closed" (10054) on its next use, and a question is lost to it.
fn office_agent() -> hub::web::ureq::Agent {
    hub::web::agent()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(120))
        .max_idle_connections(0)
        .user_agent(concat!("excalibur-hyperview-mcp/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// One message from the assistant, and what to say back — nothing for a notification.
pub fn answer(
    agent: &hub::web::ureq::Agent,
    connection: Option<&Connection>,
    request: &Value,
    log: &BridgeLog,
) -> Option<Value> {
    let id = request.get("id").cloned();
    let method = request["method"].as_str().unwrap_or("").to_string();
    let Some(id) = id else {
        log.line(format!("notification {method}"));
        return None;
    };
    let result = match method.as_str() {
        // Answered here rather than by the office server, so the assistant connects
        // even when the office is out of reach — and says why in the tools.
        "initialize" => {
            let asked = request.pointer("/params/protocolVersion").and_then(|v| v.as_str());
            let version = hyperview_server::mcp::protocol_for(asked);
            log.line(format!(
                "initialize from {} (protocol asked {}, answered {version})",
                request.pointer("/params/clientInfo/name").and_then(|v| v.as_str()).unwrap_or("?"),
                asked.unwrap_or("?")
            ));
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false }, "prompts": { "listChanged": false } },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "title": "Excalibur View",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "instructions": match connection {
                    Some(_) => format!("{}\n\n{}", hyperview_server::mcp::INSTRUCTIONS, WINDOW_INSTRUCTIONS),
                    None => format!("{WINDOW_ALONE}\n\n{WINDOW_INSTRUCTIONS}"),
                },
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_list(agent, connection, log) })),
        "resources/list" => Ok(json!({ "resources": [] })),
        "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
        "prompts/list" => Ok(json!({ "prompts": desk::prompts_for(connection.is_some()) })),
        "prompts/get" => {
            let name = request.pointer("/params/name").and_then(|v| v.as_str()).unwrap_or("");
            let args = request.pointer("/params/arguments").cloned().unwrap_or(json!({}));
            log.line(format!("prompts/get {name}"));
            desk::prompt(name, &args).map_err(|why| (-32602, why))
        }
        "tools/call" => Ok(call(agent, connection, request, log)),
        other => {
            log.line(format!("asked for {other}, which this connector does not have"));
            Err((-32601, format!("this connector has no method called {other}")))
        }
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => failure(id, code, &message),
    })
}

/// The window's tools, and the office's — asked of the office when it can be
/// reached, and otherwise the office's tools as this build knows them, so
/// the assistant still sees them and each one says what is wrong when it is used.
///
/// With no office server at all — one person on the free app — only the
/// window's: offering a list of jobs that can never be read would only send
/// the assistant after them.
fn tool_list(agent: &hub::web::ureq::Agent, connection: Option<&Connection>, log: &BridgeLog) -> Vec<Value> {
    let mut tools = desk::tools();
    let Some(connection) = connection else {
        log.line(format!("tools/list: {} tools (this window; no office server)", tools.len()));
        return tools;
    };
    let office = Some(connection)
        .and_then(|c| {
            let asked = json!({ "jsonrpc": "2.0", "id": "list", "method": "tools/list" });
            match office(agent, c, &asked) {
                Ok(answer) => answer["result"]["tools"].as_array().cloned(),
                Err(why) => {
                    log.line(format!("tools/list: office not reached: {why}"));
                    None
                }
            }
        })
        .unwrap_or_else(hyperview_server::mcp::tools);
    tools.extend(office);
    log.line(format!("tools/list: {} tools", tools.len()));
    tools
}

fn text_result(text: impl Into<String>, error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }], "isError": error })
}

fn call(agent: &hub::web::ureq::Agent, connection: Option<&Connection>, request: &Value, log: &BridgeLog) -> Value {
    let name = request.pointer("/params/name").and_then(|n| n.as_str()).unwrap_or("").to_string();
    let args = request.pointer("/params/arguments").cloned().unwrap_or(json!({}));
    let started = Instant::now();
    let result = if desk::is_window_tool(&name) {
        window_call(&name, &args)
    } else {
        match connection {
            // WHAT: the answer when this copy has no office server. WHY: it tells the assistant which Help command adds the office's jobs, by the name the menu shows.
            None => text_result(
                "This copy of Excalibur View is not connected to an office server, so there are no shared \
                 jobs to read. Everything in the window works: window_status, read_sheet, look_at_sheet, \
                 takeoff_totals and propose_markups. An office with Excalibur View Office signs in to its \
                 server (Studio) and chooses Help → Connect Assistant again to add its jobs.",
                true,
            ),
            Some(c) => match office(agent, c, request) {
                Ok(answer) if answer.get("result").is_some() => answer["result"].clone(),
                Ok(answer) => text_result(
                    answer["error"]["message"].as_str().unwrap_or("The office server refused that.").to_string(),
                    true,
                ),
                Err(why) => text_result(why, true),
            },
        }
    };
    log.line(format!(
        "tools/call {name}: {} in {} ms",
        if result["isError"] == true { "error" } else { "ok" },
        started.elapsed().as_millis()
    ));
    result
}

/// One JSON-RPC message to the office server. A connection that drops is
/// tried once more before it is reported.
fn office(agent: &hub::web::ureq::Agent, c: &Connection, request: &Value) -> Result<Value, String> {
    let place = if c.name.is_empty() { c.base.clone() } else { format!("{} ({})", c.name, c.base) };
    let url = format!("{}/mcp", c.base.trim_end_matches('/'));
    let send = || agent.post(&url).set("Authorization", &format!("Bearer {}", c.key)).send_json(request.clone());
    let sent = match send() {
        Err(hub::web::ureq::Error::Transport(_)) => {
            std::thread::sleep(Duration::from_millis(400));
            send()
        }
        other => other,
    };
    match sent {
        Ok(response) => response
            .into_json::<Value>()
            .map_err(|_| "The office server's answer was not readable.".to_string()),
        // WHAT: the message when the office server refuses this computer's key. WHY: it names the Help command that makes a new key, by the name the menu shows.
        Err(hub::web::ureq::Error::Status(401, _)) | Err(hub::web::ureq::Error::Status(403, _)) => Err(format!(
            "{place} no longer accepts this computer's key. Open Excalibur View and choose Help → Connect \
             Assistant again."
        )),
        Err(hub::web::ureq::Error::Status(code, _)) => Err(format!("{place} answered with an error ({code}).")),
        Err(e) => Err(format!(
            "The office server {place} could not be reached from this computer: {e}. It has to be on, \
             and this computer on the office network."
        )),
    }
}

// ---- the window's tools, from the connector's side -----------------------------------

fn window_call(name: &str, args: &Value) -> Value {
    match name {
        "read_sheet" => read_sheet(args).unwrap_or_else(|why| text_result(why, true)),
        "look_at_sheet" => look_at_sheet(args).unwrap_or_else(|why| text_result(why, true)),
        "open_drawing" => open_drawing(args),
        other => {
            let wait = if other == "run_check" { Duration::from_secs(50) } else { Duration::from_secs(20) };
            match desk::ask(other, args.clone(), wait) {
                Ok(said) => text_result(said.text, !said.ok),
                Err(why) => text_result(why, true),
            }
        }
    }
}

/// Opens a drawing in the window — starting Hyperview with it when no window
/// is open, the same way a double-click or a link would.
fn open_drawing(args: &Value) -> Value {
    let root = crate::instance::folder();
    if root.as_deref().is_some_and(desk::window_is_open) {
        return match desk::ask("open_drawing", args.clone(), Duration::from_secs(50)) {
            Ok(said) => text_result(said.text, !said.ok),
            Err(why) => text_result(why, true),
        };
    }
    let target = if let Some(set) = args["set"].as_str() {
        let mut link = format!("hyperview://open?set={set}");
        if let Some(page) = args["page"].as_u64() {
            link.push_str(&format!("&page={page}"));
        }
        link
    } else if let Some(file) = args["file"].as_str() {
        file.to_string()
    } else {
        return text_result("Say which drawing: a set id from the office server, or a file path.", true);
    };
    let program = crate::install::installed_program()
        .filter(|p| p.exists())
        .or_else(|| std::env::current_exe().ok());
    let Some(program) = program else {
        return text_result("Excalibur View could not be found on this computer.", true);
    };
    match std::process::Command::new(program).arg(&target).spawn() {
        Ok(_) => text_result(
            "Excalibur View was not open; it is starting with that drawing now. Give it a few seconds, \
             then ask window_status.",
            false,
        ),
        Err(e) => text_result(format!("Excalibur View could not be started: {e}"), true),
    }
}

/// Which file and sheet the window means, and how big the sheet is.
struct Sheet {
    path: PathBuf,
    page: u32,
    size: (f64, f64),
    label: String,
    unsaved: bool,
}

fn sheet_asked(args: &Value) -> Result<Sheet, String> {
    let mut asked = json!({});
    if let Some(page) = args.get("page") {
        asked["page"] = page.clone();
    }
    let said = desk::ask("where", asked, Duration::from_secs(10))?;
    if !said.ok {
        return Err(said.text);
    }
    let d = said.data;
    let page = d["page"].as_u64().unwrap_or(0) as u32;
    let number = d["sheet_number"].as_str().unwrap_or("");
    let title = d["sheet_title"].as_str().unwrap_or("");
    let scale = d["scale"].as_str().unwrap_or("no scale set");
    Ok(Sheet {
        path: PathBuf::from(d["path"].as_str().unwrap_or("")),
        page,
        size: (d["width"].as_f64().unwrap_or(612.0), d["height"].as_f64().unwrap_or(792.0)),
        label: format!(
            "Sheet {} of {}{}{} · {scale}",
            page + 1,
            d["sheets"].as_u64().unwrap_or(0),
            if number.is_empty() { String::new() } else { format!(", {number}") },
            if title.is_empty() { String::new() } else { format!(" — {title}") },
        ),
        unsaved: d["unsaved"].as_bool().unwrap_or(false),
    })
}

fn area_asked(args: &Value, size: (f64, f64)) -> [f64; 4] {
    args.get("area")
        .and_then(desk::area_arg)
        .map(|a| desk::area_from_fraction(a, size))
        .unwrap_or([0.0, 0.0, size.0, size.1])
}

/// The words on a sheet, in reading order, a line at a time, each line with
/// where it starts.
fn read_sheet(args: &Value) -> Result<Value, String> {
    let sheet = sheet_asked(args)?;
    let area = area_asked(args, sheet.size);
    let reading = crate::render::read_for_plugin(None, &sheet.path, sheet.page, true, false)?;
    let mut words: Vec<&plugin_api::Word> = reading
        .words
        .iter()
        .filter(|w| {
            let cx = (w.area[0] + w.area[2]) * 0.5;
            let cy = (w.area[1] + w.area[3]) * 0.5;
            cx >= area[0] && cx <= area[2] && cy >= area[1] && cy <= area[3]
        })
        .collect();
    words.sort_by(|a, b| a.area[1].total_cmp(&b.area[1]).then(a.area[0].total_cmp(&b.area[0])));
    let text = lines_of(&words, sheet.size);
    let mut out = format!("{} · {} words", sheet.label, words.len());
    if args.get("area").is_some() {
        out.push_str(&format!(
            " in [{:.3}, {:.3}, {:.3}, {:.3}]",
            area[0] / sheet.size.0,
            area[1] / sheet.size.1,
            area[2] / sheet.size.0,
            area[3] / sheet.size.1
        ));
    }
    out.push_str(". Each line starts with where it is: [left, top] as fractions of the sheet.\n");
    const MOST: usize = 60_000;
    if text.len() > MOST {
        let cut = text.char_indices().take_while(|(i, _)| *i < MOST).last().map(|(i, _)| i).unwrap_or(0);
        out.push_str(&text[..cut]);
        out.push_str("\n… cut short. Ask for an area of the sheet to read the rest.");
    } else if text.is_empty() {
        out.push_str("No words here — it may be a scan. look_at_sheet shows it.");
    } else {
        out.push_str(&text);
    }
    Ok(text_result(out, false))
}

/// Words gathered into lines: the same line when they sit at the same height
/// and are close enough to be one phrase.
fn lines_of(words: &[&plugin_api::Word], size: (f64, f64)) -> String {
    let mut lines: Vec<(f64, f64, f64, f64, Vec<String>)> = Vec::new(); // left, top, bottom, right, words
    for w in words {
        let height = (w.area[3] - w.area[1]).abs().max(1.0);
        let joined = lines.iter_mut().rev().take(8).find(|l| {
            let overlap = l.2.min(w.area[3]) - l.1.max(w.area[1]);
            overlap > height * 0.5 && w.area[0] - l.3 < height * 3.0 && w.area[0] >= l.0 - height
        });
        match joined {
            Some(l) => {
                l.4.push(w.text.clone());
                l.3 = l.3.max(w.area[2]);
            }
            None => lines.push((w.area[0], w.area[1], w.area[3], w.area[2], vec![w.text.clone()])),
        }
    }
    lines
        .iter()
        .map(|l| {
            let at = desk::to_fraction([l.0, l.1], size);
            format!("[{:.3}, {:.3}] {}", at[0], at[1], l.4.join(" "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A picture of a sheet, or part of one, for the assistant to look at.
fn look_at_sheet(args: &Value) -> Result<Value, String> {
    use base64::Engine;
    let sheet = sheet_asked(args)?;
    let area = area_asked(args, sheet.size);
    let (w, h) = ((area[2] - area[0]).max(1.0), (area[3] - area[1]).max(1.0));
    // Big enough to read, small enough to send: the long side at most 1568
    // pixels, and never finer than 300 dpi.
    let dpi = ((1568.0 * 72.0 / w.max(h)).min(300.0).max(10.0)) as u32;
    let image = crate::render::region_of(None, &sheet.path, sheet.page, area, dpi)?;
    let png = encode_png(&image)?;
    let note = format!(
        "{} · showing [{:.3}, {:.3}, {:.3}, {:.3}] of the sheet at {dpi} dpi ({}×{} px){}. \
         A point in this picture at (x, y) pixels is at [{:.3} + x/{} × {:.3}, {:.3} + y/{} × {:.3}] of the sheet.",
        sheet.label,
        area[0] / sheet.size.0,
        area[1] / sheet.size.1,
        area[2] / sheet.size.0,
        area[3] / sheet.size.1,
        image.width(),
        image.height(),
        if sheet.unsaved { " — markups not yet saved in the window are not in it" } else { "" },
        area[0] / sheet.size.0,
        image.width(),
        w / sheet.size.0,
        area[1] / sheet.size.1,
        image.height(),
        h / sheet.size.1,
    );
    Ok(json!({
        "content": [
            { "type": "image", "data": base64::engine::general_purpose::STANDARD.encode(png), "mimeType": "image/png" },
            { "type": "text", "text": note },
        ],
        "isError": false,
    }))
}

fn encode_png(image: &egui::ColorImage) -> Result<Vec<u8>, String> {
    let (width, height) = (image.width() as u32, image.height() as u32);
    let mut flat: Vec<u8> = Vec::with_capacity((width * height * 4) as usize);
    for pixel in &image.pixels {
        flat.extend_from_slice(&pixel.to_array());
    }
    let buffer: image::RgbaImage =
        image::ImageBuffer::from_raw(width, height, flat).ok_or("the picture came out the wrong size")?;
    let mut out = std::io::Cursor::new(Vec::new());
    buffer
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("could not make the picture: {e}"))?;
    Ok(out.into_inner())
}

fn failure(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn reply(stdout: &std::io::Stdout, answer: &Value) {
    let mut out = stdout.lock();
    let _ = writeln!(out, "{answer}");
    let _ = out.flush();
}

// ---- Test Assistant Connection -------------------------------------------------------------

/// Help → Test Assistant Connection, and `Hyperview.exe --mcp-test`: everything
/// that decides whether the assistant app can use Hyperview, checked, in words.
pub fn diagnose() -> String {
    let mut out = String::new();
    let mut ok = true;
    let mut say = |good: bool, line: String| {
        out.push_str(if good { "✓ " } else { "✗ " });
        out.push_str(&line);
        out.push('\n');
        if !good {
            ok = false;
        }
    };

    let office = kept();
    match &office {
        Some(c) => say(true, format!("Office server for the assistant: {} ({})", c.name, c.base)),
        None => say(
            true,
            // WHAT: the test's line when no office server is kept. WHY: it names the Help command that adds the office's jobs, by the name the menu shows.
            "No office server: the assistant works with this window on its own. (With Excalibur View Office, sign in \
             and choose Connect Assistant again to add the office's jobs.)"
                .into(),
        ),
    }

    let places = assistant_app_settings();
    if places.is_empty() {
        say(false, "No assistant app's settings folder was found. Install the assistant app and open it once.".into());
    }
    for place in &places {
        match (read_settings(place), lists_us(place)) {
            (Err(why), _) => say(false, why),
            (Ok(_), Some(true)) => say(true, format!("{} lists Excalibur View.", place.display())),
            (Ok(_), _) => say(false, format!("{} does not list Excalibur View. Choose Help → Connect Assistant.", place.display())), // WHAT: a settings file that does not list us yet. WHY: names the Help command that adds us, as the menu shows it.
        }
    }

    // What the assistant does, done here: start the connector and ask it.
    match exercise(office.is_some()) {
        Ok(report) => {
            for (good, line) in report {
                say(good, line);
            }
        }
        Err(why) => say(false, format!("The connector could not be run: {why}")),
    }

    // Whether the assistant has ever started it.
    let log = log_file().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    match log.lines().rev().find(|l| l.contains("The assistant started the connector")) {
        Some(line) => say(true, format!("The assistant app last started the connector at {}.", line.split(' ').next().unwrap_or("?"))),
        None => say(false, "The assistant app has never started the connector on this computer. Quit the assistant app \
                            from the system tray (right-click its icon → Quit — closing the window is not enough) \
                            and open it again: it reads its list of connectors only when it starts.".into()),
    }
    if let Some(line) = log.lines().rev().find(|l| l.contains("tools/call") && l.contains("error")) {
        out.push_str(&format!("  Last error the assistant hit: {line}\n"));
    }

    if assistant_app_running() {
        out.push_str("\nThe assistant app is running. After Connect Assistant, it must be quit from the tray and opened again before it sees Excalibur View.\n"); // WHAT: reminds the person to restart the running assistant app. WHY: it reads its connectors only when it starts; the command is named as the menu shows it.
    }
    out.insert_str(
        0,
        if ok {
            "The assistant can use Excalibur View on this computer.\n\n"
        } else {
            "Something is stopping the assistant from using Excalibur View here. Each ✗ says what.\n\n"
        },
    );
    out
}

/// Keeps the last check as `assistant-check.txt` beside the program, so a
/// support call can ask for one file.
pub fn write_check(report: &str) -> Option<PathBuf> {
    let path = home_folder()?.join("assistant-check.txt");
    let stamped = format!("{}  Excalibur View {}\n\n{report}", hyperview_server::api::now(), env!("CARGO_PKG_VERSION"));
    std::fs::write(&path, stamped).ok().map(|_| path)
}

/// Runs `Hyperview.exe --mcp` the way the assistant app does and asks it the
/// first three things the assistant asks — the third only of an office server, when
/// there is one.
fn exercise(office: bool) -> Result<Vec<(bool, String)>, String> {
    let program = crate::install::installed_program()
        .filter(|p| p.exists())
        .or_else(|| std::env::current_exe().ok())
        .ok_or("Excalibur View's program was not found")?;
    let mut child = std::process::Command::new(&program)
        .arg("--mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", program.display()))?;
    let mut stdin = child.stdin.take().ok_or("no input")?;
    let stdout = child.stdout.take().ok_or("no output")?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut report = Vec::new();
    let mut send = |message: Value| -> Option<Value> {
        let _ = writeln!(stdin, "{message}");
        let _ = stdin.flush();
        message.get("id")?;
        rx.recv_timeout(Duration::from_secs(20)).ok().and_then(|l| serde_json::from_str(&l).ok())
    };
    let hello = send(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "hyperview-test", "version": "1" } } }));
    match hello.as_ref().and_then(|h| h["result"]["serverInfo"]["name"].as_str().map(str::to_string)) {
        Some(name) => report.push((true, format!("The connector starts and answers ({name}).", ))),
        None => report.push((false, "The connector did not answer the assistant's first question.".into())),
    }
    let _ = send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    let tools = send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let count = tools.as_ref().and_then(|t| t["result"]["tools"].as_array().map(|a| a.len())).unwrap_or(0);
    report.push((count > 0, format!("It offers the assistant {count} tools.")));
    let asked = if office {
        send(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "list_projects", "arguments": {} } }))
    } else {
        None
    };
    match asked {
        _ if !office => {}
        Some(a) if a["result"]["isError"] == false => report.push((true, "It reached the office server and read the list of jobs.".into())),
        Some(a) => report.push((
            false,
            format!(
                "Asking the office server failed: {}",
                a["result"]["content"][0]["text"].as_str().or(a["error"]["message"].as_str()).unwrap_or("no reason given")
            ),
        )),
        None => report.push((false, "The office question got no answer within 20 seconds.".into())),
    }
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    Ok(report)
}

/// Whether the assistant app is running on this computer.
fn assistant_app_running() -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x0800_0000;
        let listed = std::process::Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {PROCESS}.exe"), "/NH"])
            .creation_flags(NO_WINDOW)
            .output();
        if let Ok(o) = listed {
            return String::from_utf8_lossy(&o.stdout).to_lowercase().contains(&format!("{PROCESS}.exe"));
        }
        false
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("ps")
            .arg("-A")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase().contains(PROCESS))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hv-assistant-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn hyperview_is_added_and_nothing_else_is_touched() {
        let dir = scratch("add");
        let settings = dir.join(SETTINGS_FILE);
        std::fs::write(
            &settings,
            r#"{"mcpServers":{"excalibur-fleet":{"command":"C:\\Fleet\\FleetMcp.exe"}},"sidebarMode":"chat"}"#,
        )
        .unwrap();
        let program = dir.join("Hyperview.exe");
        std::fs::write(&program, b"exe").unwrap();
        add_to(&settings, &program).unwrap();
        let after = read_settings(&settings).unwrap();
        assert_eq!(after["sidebarMode"], "chat");
        assert_eq!(after["mcpServers"]["excalibur-fleet"]["command"], r"C:\Fleet\FleetMcp.exe");
        assert_eq!(after["mcpServers"][SERVER_NAME]["args"][0], "--mcp");
        assert_eq!(lists_us(&settings), Some(true));
        // And again changes nothing but the entry itself.
        add_to(&settings, &program).unwrap();
        let again = read_settings(&settings).unwrap();
        assert_eq!(again["mcpServers"].as_object().unwrap().len(), 2);
        assert!(dir.join(format!("{SETTINGS_FILE}.before-hyperview")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_settings_file_saved_by_notepad_is_still_read_and_written() {
        // A byte-order mark in front of the JSON: what Notepad and Windows
        // PowerShell 5 write. The old connector refused the file outright, and
        // it was the one the assistant app reads.
        let dir = scratch("bom");
        let settings = dir.join(SETTINGS_FILE);
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(br#"{"mcpServers":{"excalibur-fleet":{"command":"fleet.exe"}}}"#);
        std::fs::write(&settings, bytes).unwrap();
        let program = dir.join("Hyperview.exe");
        std::fs::write(&program, b"exe").unwrap();
        add_to(&settings, &program).unwrap();
        let written = std::fs::read(&settings).unwrap();
        assert!(!written.starts_with(&[0xEF, 0xBB, 0xBF]), "written back in the form every reader takes");
        let after = read_settings(&settings).unwrap();
        assert_eq!(after["mcpServers"]["excalibur-fleet"]["command"], "fleet.exe");
        assert_eq!(lists_us(&settings), Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_settings_file_that_is_not_json_is_left_alone() {
        let dir = scratch("bad");
        let settings = dir.join(SETTINGS_FILE);
        std::fs::write(&settings, "{ this is not json").unwrap();
        assert!(add_to(&settings, Path::new("Hyperview.exe")).is_err());
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), "{ this is not json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_assistant_is_answered_even_with_no_office_server() {
        // initialize and tools/list are answered here, so the assistant connects and
        // every tool can say what is wrong, instead of the whole connector
        // showing as failed because the office was out of reach.
        let agent = office_agent();
        let log = BridgeLog { path: None };
        let hello = answer(
            &agent,
            None,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2024-11-05" } }),
            &log,
        )
        .unwrap();
        assert_eq!(hello["result"]["protocolVersion"], "2024-11-05", "the version asked for, when spoken");
        assert_eq!(hello["result"]["serverInfo"]["name"], SERVER_NAME);
        assert!(hello["result"]["instructions"].as_str().unwrap().contains("cannot draw"));
        assert!(answer(&agent, None, &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }), &log).is_none());
        let tools = answer(&agent, None, &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }), &log).unwrap();
        let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"window_status") && names.contains(&"propose_markups"));
        assert!(names.contains(&"takeoff_totals"), "the drawing's own takeoff, which needs no server");
        assert!(
            !names.contains(&"list_projects") && !names.contains(&"takeoff"),
            "no office server, so no office tools to send the assistant after: {names:?}"
        );
        let prompts = answer(&agent, None, &json!({ "jsonrpc": "2.0", "id": 4, "method": "prompts/list" }), &log).unwrap();
        let offered: Vec<&str> =
            prompts["result"]["prompts"].as_array().unwrap().iter().filter_map(|p| p["name"].as_str()).collect();
        assert!(offered.contains(&"check_takeoff") && offered.contains(&"drawing_quantities"));
        assert!(!offered.contains(&"job_quantities"), "a job's quantities live on an office server: {offered:?}");
        let asked = answer(
            &agent,
            None,
            &json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "takeoff", "arguments": { "set": "x" } } }),
            &log,
        )
        .unwrap();
        assert_eq!(asked["result"]["isError"], true);
        assert!(asked["result"]["content"][0]["text"].as_str().unwrap().contains("Connect Assistant")); // WHAT: the answer names the Help command to run. WHY: proves the message points at the command by its current menu name.
    }

    #[test]
    fn words_on_a_sheet_come_back_as_lines_in_reading_order() {
        let w = |t: &str, x: f64, y: f64| plugin_api::Word {
            text: t.into(),
            area: [x, y, x + 10.0 * t.len() as f64, y + 12.0],
            size: 12.0,
        };
        let words = [w("GENERAL", 100.0, 100.0), w("NOTES", 180.0, 100.0), w("1.", 100.0, 130.0), w("ALL", 125.0, 130.0), w("W12X26", 900.0, 101.0)];
        let refs: Vec<&plugin_api::Word> = words.iter().collect();
        let text = lines_of(&refs, (1000.0, 1000.0));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "[0.100, 0.100] GENERAL NOTES");
        assert!(lines.contains(&"[0.900, 0.101] W12X26"), "far along the same height is not the same phrase");
        assert!(lines.contains(&"[0.100, 0.130] 1. ALL"));
    }
}
