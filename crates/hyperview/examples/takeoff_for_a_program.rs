//! An office's server set up and taken off on the way a person does it, for a
//! program's own tests to run against the real server: Fabwire's pull of a
//! bid's takeoff is tested end to end with this.
//!
//! `office <url> <pin> <secret> <email> <password>` claims a fresh server from
//! this computer (the first to ask is its admin), makes the office's first
//! account, and makes a sync key named `Fabwire V2` from the Office page's
//! "Make a sync key": it prints `{"key": "…"}`.
//!
//! `computer <url> <pin> <secret> <email> <password>` asks to be let in as
//! this computer (the first to ask on a fresh server is approved as its admin
//! computer) and signs in with an account the server already has, as a
//! headless server's `HYPERVIEW_FIRST_ADMIN` one: it prints `{"approved": true}`.
//!
//! `takeoff <url> <pin> <secret> <email> <password> <project>` signs in on
//! the same computer and draws on every current drawing of the project with
//! the viewer's own tools (`sheet::Draft`, as a click on the sheet makes one),
//! a sheet with no scale calibrated first as a person calibrates it, then
//! syncs the scale and the markups up as the viewer does: it prints
//! `{"pushed": n, "scale_kept": true}` (false from a server before 0.7.4,
//! which can't keep a sheet's scale).
//!
//! `takeoff … <project> <chest>` draws one markup with every tool of the
//! chest's steel sets instead (an owner's own `.evtools`), as picking the
//! tool from the Tool Chest and clicking on the sheet does: it prints
//! `{"pushed": n, "tools": [[set, subject, kind], …]}`.
//!
//! `<pin>` is the server's certificate pin, or `none` for a server on plain
//! HTTP. `<secret>` is 64 hex digits: this computer's device key, so the second
//! command is the same computer as the first.

use std::sync::Arc;

use hyperview::app::Tool;
use hyperview::sheet::Draft;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect(); // WHAT: the words after the program's name. WHY: the command and its values
    let result = match args.first().map(String::as_str) {
        Some("office") if args.len() == 6 => office(&args[1], &args[2], &args[3], &args[4], &args[5]), // WHAT: a fresh server claimed and Fabwire's key made. WHY: what an owner does once
        Some("computer") if args.len() == 6 => let_in(&args[1], &args[2], &args[3], &args[4], &args[5]), // WHAT: this computer let in and signed in. WHY: a server set up headless already has its admin
        Some("takeoff") if args.len() == 7 || args.len() == 8 => takeoff(&args[1], &args[2], &args[3], &args[4], &args[5], &args[6], args.get(7).map(String::as_str)), // WHAT: a takeoff drawn, with the standard tools or a chest's. WHY: what an estimator does on each bid
        _ => Err("use: office URL PIN SECRET EMAIL PASSWORD, computer URL PIN SECRET EMAIL PASSWORD, or takeoff URL PIN SECRET EMAIL PASSWORD PROJECT [CHEST]".to_string()), // WHAT: how to use it. WHY: said, never guessed
    };
    match result {
        Ok(said) => println!("{said}"), // WHAT: the answer, one line of JSON. WHY: the test reads it
        Err(why) => {
            eprintln!("{why}"); // WHAT: why it failed. WHY: shown in the test's failure
            std::process::exit(1); // WHAT: a failure's exit code. WHY: the test stops there
        }
    }
}

/// This computer's client: the server's address held to its pin, and every
/// call signed by the device key `secret`.
fn computer(url: &str, pin: &str, secret: &str) -> Result<hub::Client, String> {
    let bytes = (0..32)
        .map(|i| secret.get(i * 2..i * 2 + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect::<Option<Vec<u8>>>()
        .filter(|_| secret.len() == 64)
        .ok_or("the secret is 64 hex digits")?; // WHAT: the key's 32 bytes. WHY: the same computer every time it is given
    let mut seed = [0u8; 32]; // WHAT: room for the key. WHY: DeviceKey takes an array
    seed.copy_from_slice(&bytes); // WHAT: the bytes in place. WHY: as above
    let signer = Arc::new(hub::device::Signer::new(hub::device::DeviceKey::from_secret(&seed))); // WHAT: this computer's device. WHY: every request after joining is signed by it
    let client = if pin == "none" { hub::Client::new(url) } else { hub::Client::pinned(url, pin).map_err(|e| e.to_string())? }; // WHAT: plain HTTP, or held to the pin. WHY: a test's server may run either way
    Ok(client.with_device(signer)) // WHAT: the client, signing. WHY: as a 0.7 seat connects
}

/// Lets this computer in and signs in with an account the server has.
fn let_in(url: &str, pin: &str, secret: &str, email: &str, password: &str) -> Result<String, String> {
    let mut client = computer(url, pin, secret)?; // WHAT: this computer. WHY: the first to ask is approved
    let answer = client.ask_to_be_let_in("view", "Estimator").map_err(|e| e.to_string())?.ok_or("the server has no device list")?; // WHAT: asked to be let in. WHY: as above
    if answer.state != "approved" {
        return Err(format!("this computer wasn't approved: {}", answer.state)); // WHAT: said. WHY: nothing else works until it is
    }
    client.sign_in(email, password).map_err(|e| e.to_string())?; // WHAT: signed in. WHY: proves the account works from this computer
    Ok(serde_json::json!({ "approved": true }).to_string()) // WHAT: done. WHY: the test reads it
}

/// Claims the server and makes Fabwire's sync key.
fn office(url: &str, pin: &str, secret: &str, email: &str, password: &str) -> Result<String, String> {
    let mut client = computer(url, pin, secret)?; // WHAT: the owner's computer. WHY: the first to ask is the office's admin
    let answer = client.ask_to_be_let_in("view", "Owner").map_err(|e| e.to_string())?.ok_or("the server has no device list")?; // WHAT: asked to be let in. WHY: a fresh server's first computer is approved at once
    if answer.state != "approved" {
        return Err(format!("the first computer wasn't approved: {}", answer.state)); // WHAT: said. WHY: nothing else works until it is
    }
    client
        .claim(&hub::Setup { company: "Mesa Fab".into(), name: "Owner".into(), email: email.into(), password: password.into() })
        .map_err(|e| e.to_string())?; // WHAT: the office's first account, signed in. WHY: only an administrator makes a sync key
    let key = client.make_key(hyperview::serverui::SYNC_PURPOSE, "Fabwire V2").map_err(|e| e.to_string())?; // WHAT: a sync key, as Make a sync key asks. WHY: the key Fabwire holds
    let made = key.key.ok_or("the server made a key but didn't hand it over")?; // WHAT: the key's text, shown once. WHY: pasted into Fabwire's Settings
    Ok(serde_json::json!({ "key": made }).to_string()) // WHAT: the key, as JSON. WHY: the test reads it
}

/// Takes off on every current drawing of `project`.
fn takeoff(url: &str, pin: &str, secret: &str, email: &str, password: &str, project: &str, chest: Option<&str>) -> Result<String, String> {
    use base64::Engine;
    let mut client = computer(url, pin, secret)?; // WHAT: the same computer. WHY: already approved
    client.sign_in(email, password).map_err(|e| e.to_string())?; // WHAT: signed in. WHY: markups are a person's
    let tools = match chest {
        Some(path) => Some(steel_tools(path)?), // WHAT: the chest's steel tools. WHY: drawn instead of the standard ones
        None => None, // WHAT: no chest. WHY: the standard tools
    };
    let mut drawn = Vec::new(); // WHAT: each chest tool drawn, as (set, subject, kind). WHY: said at the end, so the test knows what was asked
    let mut pushed = 0usize; // WHAT: markups sent. WHY: said at the end
    let mut kept = false; // WHAT: whether the office kept a calibrated sheet's scale. WHY: said at the end
    for set in client.sets(project).map_err(|e| e.to_string())?.into_iter().filter(|s| s.superseded_by.is_none()) {
        let sheets = client.sheets(&set.id).map_err(|e| e.to_string())?; // WHAT: its sheets, with any scale they read. WHY: an unscaled one is calibrated first
        let Some(sheet) = sheets.first() else { continue }; // WHAT: its first sheet. WHY: one sheet is enough to take off on
        let frame = annot::page::Frame::new([0.0, 0.0, sheet.width, sheet.height], sheet.rotation); // WHAT: the sheet's box. WHY: the drafts are made in it
        let scale = match sheet.scale.as_deref() {
            Some(named) if named.contains("1/4") => hyperview::scale::named(named, 48.0, hyperview::units::Units::FeetInches, 16), // WHAT: the scale the drawing says. WHY: the viewer reads it from the sheet
            _ => hyperview::scale::from_calibration(240.0, 160.0, hyperview::units::Units::FeetInches, 16).ok_or("no calibration")?, // WHAT: two clicks 240 points apart read as 13'-4". WHY: a sheet with no scale, calibrated as a person does (1/4" = 1'-0")
        };
        if sheet.scale.is_none() && sheet.measure.is_none() {
            let mut bytes = Vec::new(); // WHAT: the calibration, written as PDF. WHY: as the viewer's sync sends a sheet's scale
            pdf::write::write_object(&pdf::Object::Dict(scale.write()), &mut bytes);
            kept = client.set_sheet_scale(&set.id, sheet.page, Some(&base64::engine::general_purpose::STANDARD.encode(bytes))).is_ok(); // WHAT: the scale kept by the office, where the server can. WHY: a server before 0.7.4 has no such route, and says so here
        }
        let drafts: Vec<Draft> = if let Some(tools) = &tools {
            tools
                .iter()
                .enumerate()
                .map(|(i, (set, tool))| {
                    drawn.push(serde_json::json!([set, tool.subject, tool.kind.name()])); // WHAT: noted. WHY: the test compares what Fabwire made of each
                    from_the_chest(tool, i) // WHAT: the tool picked and clicked. WHY: as Tool Chest does
                })
                .collect()
        } else {
            vec![
            draft(Tool::Length, "W12x26", &[[100.0, 100.0], [340.0, 100.0]]), // WHAT: a beam, 240 points: 13'-4". WHY: a length tool with a shape for a subject
            draft(Tool::Length, "W12x26", &[[100.0, 140.0], [340.0, 140.0]]), // WHAT: the same beam again. WHY: two of one shape
            draft(Tool::Count, "Shear Conn", &[[400.0, 100.0]]), // WHAT: a shear connection, one click. WHY: a count tool puts one down a click
            draft(Tool::Count, "Shear Conn", &[[420.0, 100.0]]), // WHAT: a second. WHY: as above
            draft(Tool::Count, "Shear Conn", &[[440.0, 100.0]]), // WHAT: a third. WHY: as above
            draft(Tool::Area, "PL1/2", &[[100.0, 300.0], [172.0, 300.0], [172.0, 372.0], [100.0, 372.0]]), // WHAT: a plate, an inch of paper square: 16 sq ft. WHY: an area tool
            draft(Tool::Length, "Curved rail", &[[100.0, 500.0], [196.0, 500.0]]), // WHAT: a tool nobody taught Fabwire. WHY: said as one to teach
            ]
        };
        let mut sending = Vec::new(); // WHAT: the markups, as the viewer sends them. WHY: one push for the sheet
        for d in drafts {
            let markup = d.into_markup(&frame, Some(&scale)).ok_or("a draft that makes no markup")?; // WHAT: the annotation the click makes. WHY: the viewer's own code, measure and caption included
            let mut bytes = Vec::new(); // WHAT: the dictionary's bytes. WHY: as the viewer's sync writes it
            pdf::write::write_object(&pdf::Object::Dict(markup.dict), &mut bytes); // WHAT: written as PDF. WHY: the server keeps the dictionary
            sending.push(hub::NewMarkup { page: sheet.page, dictionary: base64::engine::general_purpose::STANDARD.encode(bytes), replaces: None }); // WHAT: one to send. WHY: as Ask::Sync sends it
        }
        pushed += client.push_markups(&set.id, &sending).map_err(|e| e.to_string())?.markups.len().min(sending.len()); // WHAT: sent, counted. WHY: said at the end
    }
    Ok(serde_json::json!({ "pushed": pushed, "tools": drawn, "scale_kept": kept }).to_string()) // WHAT: how many, and the chest's tools drawn, as JSON. WHY: the test reads it
}

/// A draft of `tool` named `subject`, clicked at `points`.
fn draft(tool: Tool, subject: &str, points: &[[f64; 2]]) -> Draft {
    let mut d = Draft::of(tool); // WHAT: the tool's draft. WHY: as the toolbar starts one
    d.subject = subject.to_string(); // WHAT: its subject. WHY: what the takeoff groups it by
    d.points = clicked(tool, points); // WHAT: where it was clicked, a count as the ring it draws. WHY: its geometry, as the canvas makes it
    d
}

/// The points a click makes: a count's click is the ring of 24 points the
/// canvas draws round it (`view.rs` `finish_draft`), anything else its clicks.
fn clicked(tool: Tool, points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    match (tool, points.first()) {
        (Tool::Count, Some(at)) => (0..24)
            .map(|k| {
                let a = f64::from(k) / 24.0 * std::f64::consts::TAU; // WHAT: the angle round the click. WHY: as the canvas draws it
                [at[0] + 9.0 * a.cos(), at[1] + 9.0 * a.sin()]
            })
            .collect(), // WHAT: the ring. WHY: one symbol a click
        _ => points.to_vec(), // WHAT: the clicks. WHY: a length, area or polylength is its clicks
    }
}

/// The sets of a chest that are not steel: the plain measurements and the
/// other trades.
const NOT_STEEL: &[&str] = &["General Measurements", "Electrical", "Mechanical", "Concrete", "Interior Finishes", "Fire, Life Safety & Security"]; // WHAT: the sets left out. WHY: a fabricator's pull is steel; the rest are other trades' tools

/// Every tool in the chest at `path` but those of `NOT_STEEL`, with its set.
fn steel_tools(path: &str) -> Result<Vec<(String, chest::Tool)>, String> {
    let profile = chest::Profile::open(path).ok_or_else(|| format!("{path} doesn't read as a tool chest"))?; // WHAT: the chest, read as the viewer reads it. WHY: the same tools
    Ok(profile
        .sets
        .into_iter()
        .filter(|set| !NOT_STEEL.contains(&set.title.as_str()))
        .flat_map(|set| {
            let title = set.title.clone();
            set.tools.into_iter().map(move |tool| (title.clone(), tool))
        })
        .collect()) // WHAT: each steel tool with its set. WHY: drawn one markup each
}

/// A chest tool picked and clicked on the sheet, the `i`th, in its own row.
fn from_the_chest(tool: &chest::Tool, i: usize) -> Draft {
    let (canvas, _) = Tool::for_chest(tool.kind, &tool.annotation); // WHAT: the canvas tool the chest tool arms. WHY: as pick_tool arms it
    let x = 60.0 + 36.0 * f64::from(u32::try_from(i % 60).unwrap_or(0)); // WHAT: across the sheet. WHY: sixty to a row
    let y = 60.0 + 30.0 * f64::from(u32::try_from(i / 60).unwrap_or(0)); // WHAT: down the sheet. WHY: a row each sixty
    let points: Vec<[f64; 2]> = match tool.kind {
        annot::Kind::Count => vec![[x, y]], // WHAT: one click. WHY: one of it
        annot::Kind::Area | annot::Kind::Volume => vec![[x, y], [x + 24.0, y], [x + 24.0, y + 24.0], [x, y + 24.0]], // WHAT: a small square. WHY: an area's corners
        annot::Kind::Polylength => vec![[x, y], [x + 12.0, y], [x + 24.0, y + 12.0]], // WHAT: two runs. WHY: a polylength's clicks
        _ => vec![[x, y], [x + 24.0, y]], // WHAT: two ends. WHY: a length
    };
    let mut d = Draft::of(canvas); // WHAT: the draft. WHY: as the canvas starts one
    d.subject.clone_from(&tool.subject); // WHAT: his subject. WHY: what the takeoff groups by
    d.template = Some(tool.annotation.clone()); // WHAT: his tool's own annotation. WHY: what lands on the sheet is his tool, its columns and weights
    d.points = clicked(canvas, &points); // WHAT: where it was clicked, a count as its ring. WHY: as the canvas makes it
    d
}
