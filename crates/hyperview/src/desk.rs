//! The desk: the assistant asking the Hyperview window on this computer to do
//! something, and the window answering.
//!
//! The assistant app runs `Hyperview.exe --mcp` as its connector (see
//! [`crate::assistant`]). That copy has no window. When the assistant wants to look at
//! the drawing somebody has open, turn to a sheet, point at a markup or run the
//! office's checks, the connector leaves a note on this desk — a file in the
//! person's own app data, beside the inbox a double-clicked drawing already
//! goes through — and the window picks it up on its next frame and leaves an
//! answer beside it.
//!
//! # What the assistant can and cannot do here
//!
//! It can look, read, go, point and ask the plugins. It can **propose**. It
//! cannot draw. A proposal arrives in the window exactly where a plugin's
//! findings do, each with a Fix button, and nothing is written into a drawing
//! until a person clicks one — one step on the Undo list per click. That is
//! the same rule the office server's MCP endpoint is built on, carried over to
//! the desktop: every quantity in Hyperview traces back to something a person
//! clicked, and a language model does not get a door round that.
//!
//! Coordinates in both directions are **fractions of the sheet**, measured
//! from its top-left corner as it is shown: `[0.5, 0.5]` is the middle,
//! `[0, 0, 0.25, 0.25]` the top-left quarter. The same numbers work on the
//! words `read_sheet` returns, on the pictures `look_at_sheet` returns, and in
//! `propose_markups`, whatever size the paper is.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::Receiver;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// One thing asked of the window.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Ask {
    pub id: String,
    pub tool: String,
    #[serde(default)]
    pub args: Value,
}

/// The window's answer.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Said {
    pub id: String,
    pub ok: bool,
    /// What to tell the assistant, in words.
    pub text: String,
    /// The same, as data, for the connector's own use.
    #[serde(default)]
    pub data: Value,
}

impl Said {
    pub fn ok(id: &str, text: impl Into<String>, data: Value) -> Said {
        Said { id: id.into(), ok: true, text: text.into(), data }
    }
    pub fn no(id: &str, text: impl Into<String>) -> Said {
        Said { id: id.into(), ok: false, text: text.into(), data: Value::Null }
    }
}

pub const NOT_OPEN: &str = "Excalibur View is not open on this computer. Open it (or ask me to open a \
                            drawing, which starts it) and ask again.";

fn inbox(root: &Path) -> PathBuf {
    root.join("inbox")
}

fn alive_file(root: &Path) -> PathBuf {
    root.join("inbox").join("desk.alive")
}

fn millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// Whether a window is listening. The window touches a file every second
/// while it is; nothing else ever touches it.
///
/// A heartbeat rather than a test of the window's lock: taking the lock even
/// for an instant, just as somebody double-clicks Hyperview, would make their
/// start think a window was already open, hand its drawing to nobody and quit.
pub fn window_is_open(root: &Path) -> bool {
    std::fs::metadata(alive_file(root))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .map(|age| age < Duration::from_secs(5))
        .unwrap_or(false)
}

// ---- the connector's side ---------------------------------------------------

/// Leaves `tool` on the desk and waits up to `wait` for the answer.
pub fn ask(tool: &str, args: Value, wait: Duration) -> Result<Said, String> {
    let root = crate::instance::folder().ok_or("there is no app data folder on this computer")?;
    ask_in(&root, tool, args, wait)
}

pub fn ask_in(root: &Path, tool: &str, args: Value, wait: Duration) -> Result<Said, String> {
    if !window_is_open(root) {
        return Err(NOT_OPEN.into());
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = format!(
        "{}-{}-{}",
        millis(),
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let ask = Ask { id: id.clone(), tool: tool.into(), args };
    let folder = inbox(root);
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let part = folder.join(format!("{id}.ask.part"));
    std::fs::write(&part, serde_json::to_vec(&ask).map_err(|e| e.to_string())?)
        .map_err(|e| format!("could not leave a note for the window: {e}"))?;
    let asked = folder.join(format!("{id}.ask"));
    std::fs::rename(&part, &asked).map_err(|e| e.to_string())?;
    let said = folder.join(format!("{id}.said"));
    let until = Instant::now() + wait;
    while Instant::now() < until {
        if let Ok(bytes) = std::fs::read(&said) {
            let _ = std::fs::remove_file(&said);
            return serde_json::from_slice(&bytes)
                .map_err(|e| format!("the window's answer was not readable: {e}"));
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    // Never picked up: take it back, so it is not done later, unasked.
    let _ = std::fs::remove_file(&asked);
    Err(format!(
        "Excalibur View did not answer within {} seconds. It may be busy opening a large drawing; \
         ask again in a moment.",
        wait.as_secs()
    ))
}

// ---- the window's side ------------------------------------------------------

/// What the open window hears from the desk. Only the window holding the
/// lock listens, for the same reason only it listens to the inbox.
pub fn listen(ctx: egui::Context) -> Receiver<Ask> {
    let (tx, rx) = crossbeam_channel::unbounded();
    if !crate::instance::is_the_window() {
        return rx;
    }
    let Some(root) = crate::instance::folder() else { return rx };
    let _ = std::thread::Builder::new().name("hyperview-desk".into()).spawn(move || {
        let folder = inbox(&root);
        let _ = std::fs::create_dir_all(&folder);
        // Notes left for a window that has since closed are not this window's
        // business: an answer now would be to a question nobody is waiting on.
        for stale in list(&folder, "ask").into_iter().chain(list(&folder, "said")) {
            let _ = std::fs::remove_file(stale);
        }
        let _ = std::fs::write(alive_file(&root), millis().to_string());
        let mut beat = Instant::now();
        loop {
            if beat.elapsed() > Duration::from_secs(1) {
                let _ = std::fs::write(alive_file(&root), millis().to_string());
                beat = Instant::now();
            }
            for path in list(&folder, "ask") {
                let Ok(bytes) = std::fs::read(&path) else { continue };
                let _ = std::fs::remove_file(&path);
                if let Ok(ask) = serde_json::from_slice::<Ask>(&bytes) {
                    if tx.send(ask).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            }
            std::thread::sleep(Duration::from_millis(120));
        }
    });
    rx
}

/// Leaves the window's answer where the connector is looking for it.
pub fn answer(said: &Said) {
    let Some(root) = crate::instance::folder() else { return };
    answer_in(&root, said);
}

pub fn answer_in(root: &Path, said: &Said) {
    let folder = inbox(root);
    let part = folder.join(format!("{}.said.part", said.id));
    if let Ok(mut f) = std::fs::File::create(&part) {
        let _ = f.write_all(&serde_json::to_vec(said).unwrap_or_default());
        drop(f);
        let _ = std::fs::rename(&part, folder.join(format!("{}.said", said.id)));
    }
}

fn list(folder: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == extension))
        .collect();
    found.sort();
    found
}

// ---- sheet fractions ----------------------------------------------------------

/// A point given as fractions of the sheet, in the sheet's own units.
pub fn from_fraction(p: [f64; 2], size: (f64, f64)) -> [f64; 2] {
    [p[0].clamp(0.0, 1.0) * size.0, p[1].clamp(0.0, 1.0) * size.1]
}

/// An area given as fractions of the sheet, `[left, top, right, bottom]`, in
/// the sheet's own units, corners put the right way round.
pub fn area_from_fraction(a: [f64; 4], size: (f64, f64)) -> [f64; 4] {
    let p = from_fraction([a[0].min(a[2]), a[1].min(a[3])], size);
    let q = from_fraction([a[0].max(a[2]), a[1].max(a[3])], size);
    [p[0], p[1], q[0], q[1]]
}

pub fn to_fraction(p: [f64; 2], size: (f64, f64)) -> [f64; 2] {
    let w = size.0.max(1.0);
    let h = size.1.max(1.0);
    [round3(p[0] / w), round3(p[1] / h)]
}

pub fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn fractions(v: &Value) -> Option<Vec<f64>> {
    v.as_array()?.iter().map(|x| x.as_f64()).collect()
}

/// A point out of JSON: `[x, y]`.
pub fn point_arg(v: &Value) -> Option<[f64; 2]> {
    let f = fractions(v)?;
    (f.len() == 2 && f.iter().all(|x| x.is_finite())).then(|| [f[0], f[1]])
}

/// An area out of JSON: `[left, top, right, bottom]`.
pub fn area_arg(v: &Value) -> Option<[f64; 4]> {
    let f = fractions(v)?;
    (f.len() == 4 && f.iter().all(|x| x.is_finite())).then(|| [f[0], f[1], f[2], f[3]])
}

// ---- proposals ------------------------------------------------------------------

/// Turns what the assistant proposed into findings, each with the fix that would draw
/// it — findings being the thing the window already shows with a Fix button
/// and carries out only on a click. Nothing here touches a drawing.
///
/// `page` is the sheet on screen, used for any item that does not say; `size`
/// gives the size of each sheet, for turning fractions into the sheet's units.
pub fn proposals(
    args: &Value,
    page: u32,
    size: impl Fn(u32) -> Option<(f64, f64)>,
    markup: impl Fn(&str) -> Option<(u32, [f64; 4])>,
) -> Result<plugin_api::Output, String> {
    use plugin_api::{Action, Finding, Fix, Level, Output};
    let title = args["title"].as_str().unwrap_or("Proposed by the assistant").trim().to_string();
    let items = args["markups"]
        .as_array()
        .ok_or("propose_markups needs `markups`, a list of what to propose.")?;
    if items.is_empty() {
        return Err("There was nothing in the list to propose.".into());
    }
    if items.len() > 200 {
        return Err("That is more than 200 proposals at once. Propose them a sheet at a time.".into());
    }
    let mut findings = Vec::with_capacity(items.len());
    for (n, item) in items.iter().enumerate() {
        let at = n + 1;
        let on = item["page"].as_u64().map(|p| p as u32).unwrap_or(page);
        let Some(sheet) = size(on) else {
            return Err(format!("Proposal {at} is on page {}, which the drawing does not have.", on + 1));
        };
        let kind = item["kind"].as_str().unwrap_or("").trim().to_lowercase();
        let why = item["why"].as_str().unwrap_or("").trim().to_string();
        let subject = item["subject"].as_str().unwrap_or("").trim().to_string();
        let words = item["text"].as_str().unwrap_or("").trim().to_string();
        let points: Vec<[f64; 2]> = item["points"]
            .as_array()
            .map(|a| a.iter().filter_map(point_arg).map(|p| from_fraction(p, sheet)).collect())
            .unwrap_or_default();
        let area = item["area"].as_array().and_then(|_| area_arg(&item["area"])).map(|a| area_from_fraction(a, sheet));
        let bounds = |pts: &[[f64; 2]]| -> Option<[f64; 4]> {
            let first = pts.first()?;
            Some(pts.iter().fold([first[0], first[1], first[0], first[1]], |b, p| {
                [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
            }))
        };
        // The proposals about a markup already drawn say which one.
        let named = item["markup"].as_str().unwrap_or("").trim().to_string();
        let drawn = |what: &str| -> Result<(u32, [f64; 4]), String> {
            if named.is_empty() {
                return Err(format!("Proposal {at}: {what} needs `markup`, an id from markups_on_sheet."));
            }
            markup(&named).ok_or_else(|| format!("Proposal {at}: there is no markup {named} on the drawing."))
        };
        let mut on = on;
        let (action, label, where_) = match kind.as_str() {
            "length" => {
                if points.len() < 2 || subject.is_empty() {
                    return Err(format!("Proposal {at}: a length needs a subject and at least two points."));
                }
                (Action::AddLength { page: on, points: points.clone(), subject: subject.clone() },
                 format!("Take off {subject}"), bounds(&points))
            }
            "count" => {
                if points.is_empty() || subject.is_empty() {
                    return Err(format!("Proposal {at}: a count needs a subject and a point for each item."));
                }
                (Action::AddCount { page: on, points: points.clone(), subject: subject.clone() },
                 format!("Count {} {subject}", points.len()), bounds(&points).map(|b| [b[0] - 12.0, b[1] - 12.0, b[2] + 12.0, b[3] + 12.0]))
            }
            "area" => {
                if points.len() < 3 || subject.is_empty() {
                    return Err(format!("Proposal {at}: an area needs a subject and at least three corners."));
                }
                (Action::AddArea { page: on, points: points.clone(), subject: subject.clone() },
                 format!("Take off {subject}"), bounds(&points))
            }
            "cloud" => {
                let a = area.ok_or_else(|| format!("Proposal {at}: a cloud needs an `area`."))?;
                let note = if words.is_empty() { why.clone() } else { words.clone() };
                (Action::AddCloud { page: on, area: a, note }, "Cloud it".to_string(), Some(a))
            }
            "text" => {
                let a = area.ok_or_else(|| format!("Proposal {at}: text needs an `area` to sit in."))?;
                if words.is_empty() {
                    return Err(format!("Proposal {at}: text needs `text`."));
                }
                (Action::AddText { page: on, area: a, text: words.clone() }, "Add the note".to_string(), Some(a))
            }
            "scale" => {
                let said = if words.is_empty() { item["scale"].as_str().unwrap_or("").trim().to_string() } else { words.clone() };
                let ratio = scale_ratio(&said).ok_or_else(|| {
                    format!("Proposal {at}: \"{said}\" is not a scale that can be read. Write it as the sheet does: 1/8\" = 1'-0\", 1\" = 20', 1:50.")
                })?;
                let text = tidy_scale(&said);
                (Action::SetSheetScale { page: on, ratio, text: text.clone() }, format!("Set the scale to {text}"), area)
            }
            "subject" | "relabel" => {
                let (p, b) = drawn("a new subject")?;
                if subject.is_empty() {
                    return Err(format!("Proposal {at}: say the `subject` it should have."));
                }
                on = p;
                (Action::SetSubject { markup: named.clone(), subject: subject.clone() }, format!("Call it {subject}"), Some(b))
            }
            "quantity" => {
                let (p, b) = drawn("a quantity")?;
                let quantity = item["quantity"].as_f64().filter(|q| q.is_finite() && *q > 0.0 && *q <= 100_000.0)
                    .ok_or_else(|| format!("Proposal {at}: `quantity` must be a number above nothing."))?;
                on = p;
                (Action::SetQuantity { markup: named.clone(), quantity }, format!("Make it ×{}", round3(quantity)), Some(b))
            }
            "slope" => {
                let (p, b) = drawn("a slope")?;
                let rise = item["rise"].as_f64().filter(|r| r.is_finite() && *r >= 0.0);
                let run = item["run"].as_f64().filter(|r| r.is_finite() && *r > 0.0).unwrap_or(12.0);
                let rise = rise.ok_or_else(|| format!("Proposal {at}: a slope needs `rise` (and `run`, 12 if left out)."))?;
                on = p;
                (Action::SetSlope { markup: named.clone(), rise, run }, format!("Pitch it {}:{}", round3(rise), round3(run)), Some(b))
            }
            other => {
                return Err(format!(
                    "Proposal {at}: \"{other}\" is not something that can be proposed. Use length, count, \
                     area, cloud, text, scale, subject, quantity or slope."
                ))
            }
        };
        let message = if why.is_empty() { label.clone() } else { format!("{label} — {why}") };
        let mut finding = Finding::new(Level::Check, message).on(on);
        finding.area = where_;
        finding.fixes.push(Fix { label, actions: vec![action] });
        findings.push(finding);
    }
    Ok(Output {
        title: if title.is_empty() { "Proposed by the assistant".into() } else { title },
        summary: format!(
            "{} proposal(s) from the assistant. Nothing has been drawn: check each one and click Fix to \
             draw it. Each click is one step on the Undo list.",
            findings.len()
        ),
        findings,
        tables: Vec::new(),
        files: Vec::new(), // WHAT: no files. WHY: the assistant only proposes markups; it has nothing to save.
    })
}

// ---- scales, as a sheet writes them ------------------------------------------------

/// Real length per length of paper, from a scale as a title block writes it:
/// `1/8" = 1'-0"` is 96, `1" = 20'` is 240, `1:50` is 50, `3/4"=1'` is 16.
pub fn scale_ratio(said: &str) -> Option<f64> {
    let t: String = said
        .chars()
        .map(|c| match c {
            '″' | '“' | '”' => '"',
            '′' | '‘' | '’' => '\'',
            _ => c,
        })
        .collect();
    let t = t.trim().trim_start_matches("SCALE").trim_start_matches("Scale").trim_start_matches("scale");
    let t = t.trim().trim_start_matches(':').trim();
    if let Some((a, b)) = t.split_once(':') {
        let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
        return (a > 0.0 && b > 0.0).then(|| b / a).filter(|r| *r >= 1.0 && *r <= 10_000.0);
    }
    let (paper, real) = t.split_once('=')?;
    let paper = inches(paper.trim().trim_end_matches('"').trim())?;
    let real = feet_and_inches(real.trim())?;
    (paper > 0.0 && real > 0.0).then(|| real / paper).filter(|r| *r >= 1.0 && *r <= 10_000.0)
}

/// `3/32`, `1 1/2`, `1-1/2`, `0.25`: inches.
fn inches(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (whole, part) = match text.rsplit_once([' ', '-']) {
        Some((w, p)) if p.contains('/') => (w.trim(), p.trim()),
        _ => ("", text),
    };
    let mut value = if whole.is_empty() { 0.0 } else { whole.parse::<f64>().ok()? };
    value += match part.split_once('/') {
        Some((n, d)) => {
            let d = d.trim().parse::<f64>().ok()?;
            if d == 0.0 {
                return None;
            }
            n.trim().parse::<f64>().ok()? / d
        }
        None => part.parse::<f64>().ok()?,
    };
    value.is_finite().then_some(value)
}

/// `1'-0"`, `1'`, `10'`, `6"`, `1' 6"`: inches.
fn feet_and_inches(text: &str) -> Option<f64> {
    let text = text.trim();
    match text.split_once('\'') {
        Some((feet, rest)) => {
            let feet = feet.trim().parse::<f64>().ok()?;
            let rest = rest.trim().trim_start_matches('-').trim().trim_end_matches('"').trim();
            let more = if rest.is_empty() { 0.0 } else { inches(rest)? };
            Some(feet * 12.0 + more)
        }
        None => inches(text.trim_end_matches('"')),
    }
}

/// The scale as it will be shown: the way a drafter writes it.
fn tidy_scale(said: &str) -> String {
    let t = said.trim();
    let t = t.strip_prefix("SCALE:").or_else(|| t.strip_prefix("Scale:")).unwrap_or(t).trim();
    let Some(ratio) = scale_ratio(t) else { return t.to_string() };
    let table = if t.contains(':') { crate::scale::METRIC } else { crate::scale::IMPERIAL };
    table
        .iter()
        .find(|(_, v)| (v - ratio).abs() < 1e-6)
        .map(|(label, _)| label.to_string())
        .unwrap_or_else(|| t.to_string())
}

// ---- the tools the assistant is offered --------------------------------------------------

/// The window's tools, as MCP describes a tool. None of them draws.
pub fn tools() -> Vec<Value> {
    let page = json!({ "type": "integer", "description": "Sheet by position, 1 for the first. Leave it out for the sheet on screen." });
    let area = json!({
        "type": "array", "items": { "type": "number" }, "minItems": 4, "maxItems": 4,
        "description": "[left, top, right, bottom] as fractions of the sheet from its top-left: [0,0,0.5,0.5] is the top-left quarter."
    });
    vec![
        json!({
            "name": "window_status",
            "title": "What Excalibur View has open",
            "description": "What the Excalibur View window on this computer is showing: the drawings open, the sheet on screen with its number, title and scale, what is selected, and the plugin checks available. Start here.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "open_drawing",
            "title": "Open a drawing",
            "description": "Opens a drawing set in the Excalibur View window: one from the office server by its set id (from list_sets), or a PDF on this computer by its path. Starts Excalibur View if it is not open.",
            "inputSchema": { "type": "object", "properties": {
                "set": { "type": "string", "description": "A drawing set's id on the office server." },
                "file": { "type": "string", "description": "A PDF on this computer, by its full path." },
                "page": page.clone(),
            } },
        }),
        json!({
            "name": "go_to",
            "title": "Turn to a sheet or a place",
            "description": "Turns the window to a sheet — by position or by its sheet number (S-201) — and optionally zooms to an area of it or to a markup.",
            "inputSchema": { "type": "object", "properties": {
                "page": page.clone(),
                "sheet": { "type": "string", "description": "The sheet number in the title block, e.g. S-201." },
                "area": area.clone(),
                "markup": { "type": "string", "description": "A markup's id, from markups_on_sheet." },
            } },
        }),
        json!({
            "name": "read_sheet",
            "title": "Read the words on a sheet",
            "description": "Every word on a sheet, grouped into lines, each line with where it is (fractions of the sheet from the top-left). Notes, schedules, callouts, the title block. The text of the drawing, not its lines.",
            "inputSchema": { "type": "object", "properties": { "page": page.clone(), "area": area.clone() } },
        }),
        json!({
            "name": "look_at_sheet",
            "title": "Look at a sheet",
            "description": "A picture of a sheet, or of part of one, exactly as Excalibur View draws it, markups included (as last saved). Ask for an area to see detail: a whole E-size sheet in one picture is too small to read.",
            "inputSchema": { "type": "object", "properties": { "page": page.clone(), "area": area.clone() } },
        }),
        json!({
            "name": "markups_on_sheet",
            "title": "The markups on a sheet",
            "description": "Every markup on a sheet: its id, subject, kind, measurement, quantity, status, author, and where it is (fractions of the sheet).",
            "inputSchema": { "type": "object", "properties": { "page": page.clone() } },
        }),
        json!({
            "name": "takeoff_totals",
            "title": "The takeoff of the drawing open",
            "description": "The totals the markups list shows for the drawing open in Excalibur View: each subject's count, length or area, and weight from the tool chest's unit weights, with the sheets it is on. Measurements on a sheet with no scale are listed as left out and are in no total. Works with or without an office server. `every_open_drawing` totals each drawing open in the window; `page` only that sheet.",
            "inputSchema": { "type": "object", "properties": {
                "page": page.clone(),
                "every_open_drawing": { "type": "boolean", "description": "Each drawing open in the window, one after another." },
            } },
        }),
        json!({
            "name": "show",
            "title": "Point at something",
            "description": "Selects markups and zooms to them, or zooms to an area, so the person at the screen can see what you mean. Optionally shows them a short note. Changes nothing in the drawing.",
            "inputSchema": { "type": "object", "properties": {
                "markups": { "type": "array", "items": { "type": "string" }, "description": "Markup ids from markups_on_sheet." },
                "page": page.clone(),
                "area": area.clone(),
                "note": { "type": "string", "description": "A sentence shown in the status bar." },
            } },
        }),
        json!({
            "name": "run_check",
            "title": "Run one of the office's checks",
            "description": "Runs a plugin command (for example an office's estimating checks: missing scales, members nobody took off) on the drawing open, and returns what it found. The findings also appear in the window. Leave `command` out to list the commands.",
            "inputSchema": { "type": "object", "properties": {
                "command": { "type": "string", "description": "The command's name or id, as window_status lists it." },
            } },
        }),
        json!({
            "name": "propose_markups",
            "title": "Propose markups for someone to accept",
            "description": "Proposes takeoff or review changes. They appear in the Excalibur View window as a list, each outlined on the sheet with a Fix button. NOTHING IS DRAWN OR CHANGED until the person clicks Fix on it. New markups: length (subject + 2 or more points), count (subject + a point per item), area (subject + 3 or more corners), cloud (area + text), text (area + text). A sheet: scale (text as the title block writes it, e.g. 1/8\" = 1'-0\" or 1:50). A markup already drawn, by its id from markups_on_sheet: subject (the subject it should have), quantity (a multiplier, e.g. 6 for six identical), slope (rise per run, run 12 if left out). Points and areas are fractions of the sheet from the top-left. Use the subjects of the office's tool chest (W12x26, Moment Conn) so weights come with them.",
            "inputSchema": { "type": "object", "required": ["markups"], "properties": {
                "title": { "type": "string", "description": "What this batch is, e.g. \"Beams not taken off on S-201\"." },
                "markups": { "type": "array", "description": "What to propose.", "items": {
                    "type": "object", "required": ["kind"], "properties": {
                        "kind": { "type": "string", "enum": ["length", "count", "area", "cloud", "text", "scale", "subject", "quantity", "slope"] },
                        "markup": { "type": "string", "description": "For subject, quantity and slope: the markup's id from markups_on_sheet." },
                        "quantity": { "type": "number" },
                        "rise": { "type": "number" },
                        "run": { "type": "number" },
                        "page": page.clone(),
                        "subject": { "type": "string" },
                        "points": { "type": "array", "items": { "type": "array", "items": { "type": "number" } } },
                        "area": area.clone(),
                        "text": { "type": "string" },
                        "why": { "type": "string", "description": "One line saying why, shown beside the Fix button." },
                    } } },
            } },
        }),
    ]
}

// ---- ready-made requests ------------------------------------------------------------

/// Requests a person picks from the assistant app's menu rather than typing:
/// the jobs an estimator asks for every week, written out once, properly.
/// MCP calls these prompts; the assistant app shows them under the connector.
pub fn prompts() -> Vec<Value> {
    let job = json!({ "name": "job", "description": "The job number or name, e.g. 26-114.", "required": true });
    vec![
        json!({ "name": "check_takeoff", "title": "Check the takeoff on this sheet",
                "description": "Reads and looks at the sheet on screen in Excalibur View, compares it with the markups, and proposes what is missing.",
                "arguments": [{ "name": "focus", "description": "Only this, e.g. \"beams\" or \"moment connections\".", "required": false }] }),
        json!({ "name": "set_scales", "title": "Find each sheet's scale",
                "description": "Reads every sheet's title block and proposes the scale where none is set.", "arguments": [] }),
        json!({ "name": "draft_rfis", "title": "Draft RFIs for this sheet",
                "description": "Finds what can't be fabricated or priced from the sheet on screen, writes the RFIs and proposes a cloud at each.", "arguments": [] }),
        json!({ "name": "drawing_quantities", "title": "This drawing's quantities",
                "description": "The takeoff of the drawing open in Excalibur View, by subject, with what was left out.", "arguments": [] }),
        json!({ "name": "job_quantities", "title": "A job's quantities",
                "description": "The current takeoff of a job from the office server, by subject, with what was left out.", "arguments": [job.clone()] }),
        json!({ "name": "what_changed", "title": "What a reissue changed",
                "description": "Drawings reissued on a job, what moved and what it is worth, and the carried-forward takeoff still to check.", "arguments": [job.clone()] }),
        json!({ "name": "cut_list", "title": "Cut list for a job",
                "description": "The job's lengths as a cut list, with sticks to buy when a stock length is given.",
                "arguments": [job, { "name": "stock_length_feet", "description": "The stock length the shop buys, e.g. 40.", "required": false }] }),
    ]
}

/// One ready-made request, filled in.
pub fn prompt(name: &str, args: &Value) -> Result<Value, String> {
    let arg = |k: &str| args.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|v| !v.is_empty());
    let job = || arg("job").ok_or_else(|| format!("{name} needs the job: its number or name."));
    let text = match name {
        "check_takeoff" => {
            let focus = arg("focus").map(|f| format!(" — only {f}")).unwrap_or_default();
            format!(
                "In Excalibur View, check the takeoff on the sheet on screen{focus}.\n\
                 1. window_status, then markups_on_sheet for what is already taken off.\n\
                 2. read_sheet for the words — sizes, marks, notes — and look_at_sheet a quarter of the sheet at a \
                 time to see the members themselves.\n\
                 3. Tell me what is taken off, what is shown but not taken off, and anything that looks wrong: a \
                 subject that doesn't match the size on the drawing, a count that looks short, a measurement on a \
                 sheet with no scale.\n\
                 4. Put what is missing up with propose_markups, in one batch, with a `why` on each saying what you \
                 saw. Use the subjects in our tool chest. A wrong subject or quantity on an existing markup is \
                 proposed too (kind subject or quantity, by its id). Don't say you drew anything: I click Fix on \
                 what I agree with."
            )
        }
        "set_scales" => "Go through the drawing open in Excalibur View a sheet at a time (go_to each page). For each sheet \
             whose scale is not set, read its title block and view titles with read_sheet — the bottom-right corner \
             first. If one scale covers the sheet, propose it with propose_markups, kind \"scale\", on that page, \
             with the text as the sheet writes it. If views on one sheet have different scales, or it says NTS or \
             AS NOTED, don't propose one — list it for me instead. Finish with a short table: sheet, scale found, \
             proposed or not and why."
            .to_string(),
        "draft_rfis" => "Read the sheet on screen in Excalibur View (read_sheet, and look_at_sheet in parts). Find what a \
             steel fabricator can't build or price from it: member sizes not given, connections not detailed or \
             \"by others\" with no design loads, missing elevations or dimensions, notes that contradict each other \
             or another sheet. For each, write an RFI: the question, the sheet and grid location, and why it matters \
             to fabrication. Then propose a cloud at each location (propose_markups, kind cloud) with the question as \
             its text. Nothing is drawn until I click Fix."
            .to_string(),
        "drawing_quantities" => "From the drawing open in Excalibur View: takeoff_totals. Give me the quantities by \
             subject — count, length or area, weight — and the total weight. Say plainly which sheets have no scale \
             and what was left out because of it, and which subjects have no unit weight; never fill a weight in. \
             If a sheet has no scale, offer to read its title block and propose one."
            .to_string(),
        "job_quantities" => format!(
            "From the office server: find job {} with list_projects, its current drawing sets with list_sets \
             (leave out any a newer issue replaced), and the takeoff of each with takeoff. Give me the quantities \
             by subject — count, length, weight — and a total weight. Say plainly what was left out for want of a \
             scale and what has no unit weight; never fill a weight in. List takeoff carried forward that still \
             needs a check, and anything counted_twice finds.",
            job()?
        ),
        "what_changed" => format!(
            "For job {}: list_sets, and find each drawing that was reissued (a set superseded by a newer one). For \
             each pair, run revision_cost with the old set as was_bid and the new one as arrived, and tell me what \
             moved and what it is worth. Then list the markups carried forward that still need checking, by sheet, \
             and offer to open them in Excalibur View (open_drawing, go_to).",
            job()?
        ),
        "cut_list" => {
            let stock = match arg("stock_length_feet") {
                Some(feet) => format!(" with a stock length of {feet} feet"),
                None => " (no stock length: don't nest, and don't invent one)".into(),
            };
            format!(
                "For job {}: find its current drawing sets with list_sets, and make the cut list for each with \
                 shop_list{stock}. Summarize by shape: pieces, total length, weight, and sticks to buy and drop \
                 where there is a stock length. Say what was left out for want of a scale.",
                job()?
            )
        }
        other => return Err(format!("There is no ready-made request called {other}.")),
    };
    let description = prompts()
        .into_iter()
        .find(|p| p["name"] == name)
        .and_then(|p| p["description"].as_str().map(str::to_string))
        .unwrap_or_default();
    Ok(json!({
        "description": description,
        "messages": [{ "role": "user", "content": { "type": "text", "text": text } }],
    }))
}

/// The ready-made requests that need the office server: a job's shared
/// takeoff lives there. Without a server they are not offered.
pub const OFFICE_PROMPTS: &[&str] = &["job_quantities", "what_changed", "cut_list"];

/// The ready-made requests to offer: all of them with an office server, the
/// window's own without one.
pub fn prompts_for(office: bool) -> Vec<Value> {
    prompts()
        .into_iter()
        .filter(|p| office || !OFFICE_PROMPTS.iter().any(|name| p["name"] == *name))
        .collect()
}

/// Whether a tool is one of the window's rather than the office server's.
pub fn is_window_tool(name: &str) -> bool {
    tools().iter().any(|t| t["name"] == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("hv-desk-{}-{}-{n}", std::process::id(), millis()));
        std::fs::create_dir_all(dir.join("inbox")).unwrap();
        dir
    }

    #[test]
    fn a_question_left_on_the_desk_is_answered_by_the_window() {
        let root = scratch();
        std::fs::write(alive_file(&root), "1").unwrap();
        let window_root = root.clone();
        let window = std::thread::spawn(move || {
            for _ in 0..100 {
                if let Some(path) = list(&inbox(&window_root), "ask").into_iter().next() {
                    let ask: Ask = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    std::fs::remove_file(path).unwrap();
                    answer_in(&window_root, &Said::ok(&ask.id, format!("heard {}", ask.tool), json!({"n": 1})));
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let said = ask_in(&root, "window_status", json!({}), Duration::from_secs(5)).unwrap();
        window.join().unwrap();
        assert!(said.ok);
        assert_eq!(said.text, "heard window_status");
        assert!(list(&inbox(&root), "said").is_empty(), "an answer is taken once");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn with_no_window_open_it_says_so_rather_than_waiting() {
        let root = scratch();
        let started = Instant::now();
        let refused = ask_in(&root, "window_status", json!({}), Duration::from_secs(30)).unwrap_err();
        assert!(refused.contains("not open"), "{refused}");
        assert!(started.elapsed() < Duration::from_secs(1));
        // An old heartbeat is not a window.
        std::fs::write(alive_file(&root), "1").unwrap();
        let old = SystemTime::now() - Duration::from_secs(60);
        let f = std::fs::File::options().write(true).open(alive_file(&root)).unwrap();
        f.set_modified(old).unwrap();
        assert!(!window_is_open(&root));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_question_nobody_picks_up_is_taken_back() {
        let root = scratch();
        std::fs::write(alive_file(&root), "1").unwrap();
        let refused = ask_in(&root, "go_to", json!({}), Duration::from_millis(300)).unwrap_err();
        assert!(refused.contains("did not answer"), "{refused}");
        assert!(list(&inbox(&root), "ask").is_empty(), "it must not be done later, unasked");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_proposal_is_findings_with_fixes_and_draws_nothing() {
        let size = |p: u32| (p == 0).then_some((3024.0, 2160.0));
        let out = proposals(
            &json!({ "title": "Missed beams", "markups": [
                { "kind": "length", "subject": "W12x26", "points": [[0.1, 0.5], [0.4, 0.5]], "why": "grid 3, no takeoff" },
                { "kind": "count", "subject": "Moment Conn", "points": [[0.2, 0.2], [0.3, 0.2]] },
                { "kind": "cloud", "area": [0.5, 0.5, 0.6, 0.6], "text": "RFI: beam size missing" },
            ] }),
            0,
            size,
            |_| None,
        )
        .unwrap();
        assert_eq!(out.findings.len(), 3);
        assert!(out.findings.iter().all(|f| f.fixes.len() == 1), "every proposal waits for its click");
        assert!(out.summary.contains("Nothing has been drawn"));
        match &out.findings[0].fixes[0].actions[0] {
            plugin_api::Action::AddLength { points, subject, .. } => {
                assert_eq!(subject, "W12x26");
                assert!((points[0][0] - 302.4).abs() < 1e-6 && (points[1][0] - 1209.6).abs() < 1e-6);
                assert!((points[0][1] - 1080.0).abs() < 1e-6, "fractions are from the top-left");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(&out.findings[1].fixes[0].actions[0], plugin_api::Action::AddCount { points, .. } if points.len() == 2));
        assert!(out.findings[0].message.contains("grid 3"));
    }

    #[test]
    fn a_proposal_that_cannot_be_drawn_is_refused_in_words() {
        let size = |_: u32| Some((1000.0, 1000.0));
        for (bad, says) in [
            (json!({ "markups": [] }), "nothing"),
            (json!({ "markups": [{ "kind": "length", "points": [[0.1, 0.1]] }] }), "two points"),
            (json!({ "markups": [{ "kind": "delete", "points": [] }] }), "not something"),
            (json!({ "markups": [{ "kind": "text", "area": [0, 0, 0.1, 0.1] }] }), "needs `text`"),
        ] {
            let why = proposals(&bad, 0, size, |_| None).unwrap_err();
            assert!(why.contains(says), "{why}");
        }
        let off = proposals(&json!({ "markups": [{ "kind": "cloud", "page": 9, "area": [0, 0, 1, 1] }] }), 0, |p| (p == 0).then_some((1.0, 1.0)), |_| None);
        assert!(off.unwrap_err().contains("page 10"));
    }

    #[test]
    fn every_ready_made_request_fills_in_and_names_only_real_tools() {
        // The tools, and what their arguments are called: office and window both.
        let mut names = Vec::new();
        for t in tools().into_iter().chain(hyperview_server::mcp::tools()) {
            names.push(t["name"].as_str().unwrap().to_string());
            if let Some(props) = t["inputSchema"]["properties"].as_object() {
                names.extend(props.keys().cloned());
            }
        }
        for p in prompts() {
            let name = p["name"].as_str().unwrap();
            let needs_job = p["arguments"].as_array().unwrap().iter().any(|a| a["name"] == "job");
            if needs_job {
                assert!(prompt(name, &json!({})).unwrap_err().contains("job"), "{name}");
            }
            let filled = prompt(name, &json!({ "job": "26-114", "stock_length_feet": "40", "focus": "beams" })).unwrap();
            let text = filled["messages"][0]["content"]["text"].as_str().unwrap();
            assert!(!filled["description"].as_str().unwrap().is_empty());
            if needs_job {
                assert!(text.contains("26-114"), "{name}: {text}");
            }
            // Every snake_case word that reads like a tool must be one.
            for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                if word.contains('_') && word.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                    assert!(names.iter().any(|n| n == word), "{name} names {word}, which is not a tool");
                }
            }
        }
        assert!(prompt("nonsense", &json!({})).is_err());
    }

    #[test]
    fn scales_are_read_the_way_title_blocks_write_them() {
        for (said, ratio) in [
            ("1/8\" = 1'-0\"", 96.0),
            ("SCALE: 1/4\" = 1'-0\"", 48.0),
            ("3/32\"=1'-0\"", 128.0),
            ("1 1/2\" = 1'-0\"", 8.0),
            ("1-1/2\" = 1'", 8.0),
            ("3/4″ = 1′-0″", 16.0),
            ("1\" = 20'", 240.0),
            ("1\" = 100'-0\"", 1200.0),
            ("3\" = 1'-0\"", 4.0),
            ("1:50", 50.0),
            ("Scale: 1:100", 100.0),
            ("6\" = 1'-6\"", 3.0),
        ] {
            let got = scale_ratio(said).unwrap_or(f64::NAN);
            assert!((got - ratio).abs() < 1e-9, "{said}: {got}");
        }
        for nonsense in ["", "NTS", "AS NOTED", "1/0\" = 1'", "= 1'", "1:0", "W12x26"] {
            assert!(scale_ratio(nonsense).is_none(), "{nonsense}");
        }
        assert_eq!(tidy_scale("SCALE: 1/8\"=1'-0\""), "1/8\" = 1'-0\"");
        assert_eq!(tidy_scale("1:50"), "1:50");
    }

    #[test]
    fn proposals_about_markups_already_drawn_name_them() {
        let size = |_: u32| Some((1000.0, 1000.0));
        let there = |id: &str| (id == "#4").then_some((2u32, [10.0, 10.0, 90.0, 20.0]));
        let out = proposals(
            &json!({ "markups": [
                { "kind": "scale", "page": 0, "text": "SCALE: 1/4\" = 1'-0\"", "why": "title block" },
                { "kind": "subject", "markup": "#4", "subject": "W14x30" },
                { "kind": "quantity", "markup": "#4", "quantity": 6 },
                { "kind": "slope", "markup": "#4", "rise": 4 },
            ] }),
            0,
            size,
            there,
        )
        .unwrap();
        assert!(matches!(&out.findings[0].fixes[0].actions[0],
            plugin_api::Action::SetSheetScale { page: 0, ratio, text } if (*ratio - 48.0).abs() < 1e-9 && text == "1/4\" = 1'-0\""));
        assert!(matches!(&out.findings[1].fixes[0].actions[0],
            plugin_api::Action::SetSubject { markup, subject } if markup == "#4" && subject == "W14x30"));
        assert_eq!(out.findings[1].page, Some(2), "a markup's proposal is on the markup's sheet");
        assert_eq!(out.findings[1].area, Some([10.0, 10.0, 90.0, 20.0]));
        assert!(matches!(&out.findings[2].fixes[0].actions[0], plugin_api::Action::SetQuantity { quantity, .. } if *quantity == 6.0));
        assert!(matches!(&out.findings[3].fixes[0].actions[0],
            plugin_api::Action::SetSlope { rise, run, .. } if *rise == 4.0 && *run == 12.0));
        for (bad, says) in [
            (json!({ "markups": [{ "kind": "subject", "markup": "#9", "subject": "X" }] }), "no markup #9"),
            (json!({ "markups": [{ "kind": "subject", "subject": "X" }] }), "needs `markup`"),
            (json!({ "markups": [{ "kind": "scale", "text": "NTS" }] }), "not a scale"),
            (json!({ "markups": [{ "kind": "quantity", "markup": "#4", "quantity": -1 }] }), "above nothing"),
        ] {
            let why = proposals(&bad, 0, size, there).unwrap_err();
            assert!(why.contains(says), "{why}");
        }
    }

    #[test]
    fn no_window_tool_draws_or_deletes() {
        // The rule this whole module is written under, checked the way the
        // office server's MCP checks its own: by name, and by what each tool
        // says it does.
        for tool in tools() {
            let name = tool["name"].as_str().unwrap();
            for verb in ["add", "draw", "delete", "remove", "place", "create", "write", "save", "set_", "edit"] {
                assert!(!name.starts_with(verb), "{name} reads like a tool that changes a drawing");
            }
            assert!(!tool["description"].as_str().unwrap().is_empty());
        }
        let propose = tools().into_iter().find(|t| t["name"] == "propose_markups").unwrap();
        assert!(propose["description"].as_str().unwrap().contains("NOTHING IS DRAWN"));
    }

    #[test]
    fn fractions_turn_into_the_sheets_own_units_and_back() {
        let size = (3024.0, 2160.0);
        assert_eq!(from_fraction([0.5, 0.25], size), [1512.0, 540.0]);
        assert_eq!(from_fraction([1.5, -1.0], size), [3024.0, 0.0], "off the sheet is the edge of it");
        assert_eq!(area_from_fraction([0.5, 0.5, 0.25, 0.25], size), [756.0, 540.0, 1512.0, 1080.0]);
        assert_eq!(to_fraction([1512.0, 540.0], size), [0.5, 0.25]);
    }
}
