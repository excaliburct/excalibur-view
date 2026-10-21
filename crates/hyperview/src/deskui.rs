//! The window's side of the desk ([`crate::desk`]): answering what the assistant
//! asks, from the state on screen.
//!
//! Everything here reads the window, moves the view, or puts findings up with
//! a Fix button. Nothing here writes a markup: the only path from a proposal
//! to a drawing is a person clicking Fix, which runs through
//! [`App::carry_out`] exactly as a plugin's fix does.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::app::App;
use crate::desk::{self, Ask, Said};

/// Something asked that can only be answered once something else has
/// happened: a drawing opening, a plugin finishing.
pub struct Waiting {
    pub ask: Ask,
    pub until: Instant,
    pub what: Awaiting,
}

pub enum Awaiting {
    /// A drawing opening, from the office server or from this computer.
    Open { set: Option<String>, path: Option<PathBuf>, page: Option<u32> },
    /// A plugin command run for the assistant, identified by the job number it got.
    Check { job: u64 },
}

/// Where to turn once a drawing has opened: asked for by a `hyperview://`
/// link or by the assistant.
pub struct TurnTo {
    pub set: Option<String>,
    pub path: Option<PathBuf>,
    pub page: u32,
    pub since: Instant,
}

impl App {
    /// Called every frame: new questions, and old ones that can now be
    /// answered.
    pub fn drain_desk(&mut self, ctx: &egui::Context) {
        let asks: Vec<Ask> = self.desk.try_iter().collect();
        for ask in asks {
            log::info!("the assistant asked the window: {}", ask.tool);
            if let Some(said) = self.answer_desk(ask, ctx) {
                desk::answer(&said);
            }
        }
        self.poll_desk_waits();
        self.turn_after_open();
        // Nobody may be touching the window while the assistant works it: keep
        // looking until everything asked for has been answered.
        if !self.desk_waits.is_empty() || self.turn_to.is_some() || !self.links_waiting.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        if !self.links_waiting.is_empty() {
            let ready = self.standing.signed_in();
            let links = std::mem::take(&mut self.links_waiting);
            for (link, since) in links {
                if ready || since.elapsed() > Duration::from_secs(20) {
                    self.open_link(&link);
                } else {
                    self.links_waiting.push((link, since));
                }
            }
        }
    }

    fn answer_desk(&mut self, ask: Ask, ctx: &egui::Context) -> Option<Said> {
        let id = ask.id.clone();
        let args = ask.args.clone();
        let said = match ask.tool.as_str() {
            "window_status" => self.desk_status(&id),
            "where" => self.desk_where(&id, &args),
            "go_to" => self.desk_go(&id, &args),
            "markups_on_sheet" => self.desk_markups(&id, &args),
            "takeoff_totals" => self.desk_takeoff(&id, &args),
            "show" => self.desk_show(&id, &args),
            "propose_markups" => self.desk_propose(&id, &args),
            "open_drawing" => return self.desk_open(ask, ctx),
            "run_check" => return self.desk_check(ask, ctx),
            other => Said::no(&id, format!("The window does not know how to {other}.")),
        };
        Some(said)
    }

    fn page_arg(&self, args: &Value) -> Result<u32, String> {
        let doc = self.doc().ok_or("No drawing is open in Excalibur View.")?;
        match args.get("page").and_then(|p| p.as_u64()) {
            None => Ok(doc.page),
            Some(0) => Err("Pages are counted from 1.".into()),
            Some(p) if p as usize > doc.pages.len() => Err(format!(
                "This drawing has {} sheets; there is no page {p}.",
                doc.pages.len()
            )),
            Some(p) => Ok(p as u32 - 1),
        }
    }

    fn sheet_words(&self, page: u32) -> (String, String) {
        self.doc()
            .and_then(|d| d.labels.get(page as usize))
            .map(|l| (l.number.clone(), l.title.clone()))
            .unwrap_or_default()
    }

    fn desk_status(&self, id: &str) -> Said {
        let tabs: Vec<Value> = self
            .docs
            .iter()
            .enumerate()
            .map(|(i, d)| {
                json!({
                    "tab": i + 1,
                    "name": crate::app::name_of(&d.path),
                    "set": d.attached.as_ref().map(|a| a.set.clone()),
                    "sheets": d.pages.len(),
                    "on_screen": i == self.current,
                })
            })
            .collect();
        let commands: Vec<Value> = self
            .plugins
            .plugins
            .iter()
            .flat_map(|p| {
                p.manifest.commands.iter().map(move |c| {
                    json!({ "plugin": p.manifest.name, "command": c.name, "id": c.id, "about": c.description })
                })
            })
            .collect();
        let mut text = String::new();
        if self.docs.is_empty() {
            text.push_str("Excalibur View is open with no drawing in it.\n");
        }
        let mut current = Value::Null;
        if let Some(doc) = self.doc() {
            let (number, title) = self.sheet_words(doc.page);
            let scale = doc.scale_of(doc.page).map(|m| m.ratio.clone());
            let chosen: Vec<Value> = doc
                .selected
                .into_iter()
                .chain(doc.also.iter().copied())
                .filter_map(|i| doc.marks.get(i).map(|m| (i, m)))
                .map(|(i, m)| json!({
                    "id": mark_id(i, m),
                    "subject": m.markup.subject(),
                    "kind": m.kind().name(),
                    "measurement": m.markup.contents(),
                }))
                .collect();
            text.push_str(&format!(
                "On screen: {} — sheet {} of {}{}{}. Scale: {}.\n",
                crate::app::name_of(&doc.path),
                doc.page + 1,
                doc.pages.len(),
                if number.is_empty() { String::new() } else { format!(", {number}") },
                if title.is_empty() { String::new() } else { format!(" ({title})") },
                scale.clone().unwrap_or_else(|| "none set — measurements on this sheet are not counted".into()),
            ));
            if !chosen.is_empty() {
                text.push_str(&format!("Selected: {} markup(s).\n", chosen.len()));
            }
            current = json!({
                "page": doc.page + 1, "sheets": doc.pages.len(), "sheet_number": number, "sheet_title": title,
                "scale": scale, "selected": chosen, "set": doc.attached.as_ref().map(|a| a.set.clone()),
                "markups": doc.marks.iter().filter(|m| !m.gone && m.page == doc.page).count(),
            });
        }
        if tabs.len() > 1 {
            text.push_str(&format!("{} drawings are open in tabs:\n", tabs.len()));
            for t in &tabs {
                text.push_str(&format!(
                    "  {}. {} — {} sheet(s){}{}\n",
                    t["tab"],
                    t["name"].as_str().unwrap_or(""),
                    t["sheets"],
                    t["set"].as_str().map(|s| format!(", set {s}")).unwrap_or_default(),
                    if t["on_screen"] == true { " (on screen)" } else { "" },
                ));
            }
        }
        text.push_str(&if self.standing.signed_in() {
            format!("Signed in to the office server {}.\n", self.standing.name)
        } else {
            "Not signed in to an office server.\n".to_string()
        });
        if !commands.is_empty() {
            text.push_str("Checks that can be run (run_check): ");
            text.push_str(
                &commands
                    .iter()
                    .map(|c| c["command"].as_str().unwrap_or("").to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            text.push('\n');
        }
        Said::ok(id, text, json!({ "tabs": tabs, "current": current, "checks": commands }))
    }

    /// For the connector's own reading and looking: which file, which sheet,
    /// how big.
    fn desk_where(&self, id: &str, args: &Value) -> Said {
        let page = match self.page_arg(args) {
            Ok(p) => p,
            Err(why) => return Said::no(id, why),
        };
        let doc = self.doc().expect("page_arg found one");
        let size = doc.pages.get(page as usize).copied().unwrap_or(doc.size());
        let (number, title) = self.sheet_words(page);
        let unsaved = doc.dirty;
        Said::ok(
            id,
            "",
            json!({
                "path": doc.path.to_string_lossy(),
                "page": page, "sheets": doc.pages.len(),
                "width": size.width, "height": size.height,
                "sheet_number": number, "sheet_title": title,
                "scale": doc.scale_of(page).map(|m| m.ratio),
                "unsaved": unsaved,
            }),
        )
    }

    fn desk_go(&mut self, id: &str, args: &Value) -> Said {
        if self.doc().is_none() {
            return Said::no(id, "No drawing is open in Excalibur View.");
        }
        let mut page = match self.page_arg(args) {
            Ok(p) => p,
            Err(why) => return Said::no(id, why),
        };
        if let Some(sheet) = args.get("sheet").and_then(|s| s.as_str()) {
            let wanted = sheet.trim().to_lowercase().replace(' ', "");
            let doc = self.doc().expect("checked");
            match doc.labels.iter().position(|l| l.number.to_lowercase().replace(' ', "") == wanted) {
                Some(p) => page = p as u32,
                None => return Said::no(id, format!("No sheet in this drawing is numbered {sheet}.")),
            }
        }
        let mut focus: Option<[f64; 4]> = None;
        if let Some(markup) = args.get("markup").and_then(|m| m.as_str()) {
            let doc = self.doc().expect("checked");
            match find_mark(doc, markup) {
                Some(i) => {
                    page = doc.marks[i].page;
                    focus = sheet_bounds(doc, i);
                    self.go_to(page);
                    if let Some(doc) = self.doc_mut() {
                        doc.choose(Some(i));
                    }
                }
                None => return Said::no(id, format!("There is no markup {markup} on this drawing.")),
            }
        }
        self.go_to(page);
        if let Some(area) = args.get("area").and_then(desk::area_arg) {
            let size = self.doc().and_then(|d| d.pages.get(page as usize).copied());
            if let Some(size) = size {
                focus = Some(desk::area_from_fraction(area, (size.width as f64, size.height as f64)));
            }
        }
        if let Some(area) = focus {
            self.frame_area(area);
        }
        let (number, title) = self.sheet_words(page);
        Said::ok(
            id,
            format!(
                "Showing sheet {}{}{}.",
                page + 1,
                if number.is_empty() { String::new() } else { format!(", {number}") },
                if title.is_empty() { String::new() } else { format!(" ({title})") }
            ),
            json!({ "page": page + 1 }),
        )
    }

    /// Zooms so `area` (sheet units) fills most of the canvas.
    fn frame_area(&mut self, area: [f64; 4]) {
        let canvas = self.last_canvas;
        if let Some(doc) = self.doc_mut() {
            let w = (area[2] - area[0]).abs().max(20.0) as f32;
            let h = (area[3] - area[1]).abs().max(20.0) as f32;
            let zoom = (canvas.width() / w).min(canvas.height() / h) * 0.85;
            doc.view.zoom = zoom.clamp(0.05, 8.0);
            doc.view.centre_on([(area[0] + area[2]) * 0.5, (area[1] + area[3]) * 0.5], canvas);
        }
    }

    fn desk_markups(&self, id: &str, args: &Value) -> Said {
        let page = match self.page_arg(args) {
            Ok(p) => p,
            Err(why) => return Said::no(id, why),
        };
        let doc = self.doc().expect("page_arg found one");
        let size = doc.pages.get(page as usize).copied().unwrap_or(doc.size());
        let mut rows = Vec::new();
        let mut text = String::new();
        for (i, m) in doc.marks.iter().enumerate() {
            if m.gone || m.page != page {
                continue;
            }
            let area = sheet_bounds(doc, i).map(|b| {
                let p = desk::to_fraction([b[0], b[1]], (size.width as f64, size.height as f64));
                let q = desk::to_fraction([b[2], b[3]], (size.width as f64, size.height as f64));
                [p[0], p[1], q[0], q[1]]
            });
            let quantity = takeoff::row::quantity_of(&m.markup);
            let row = json!({
                "id": mark_id(i, m),
                "subject": m.markup.subject(),
                "kind": m.kind().name(),
                "measurement": m.markup.contents(),
                "quantity": quantity,
                "status": m.markup.text_of("BSIStatus"),
                "author": m.markup.text_of("T"),
                "area": area,
            });
            text.push_str(&format!(
                "{} · {} · {}{}{}{}\n",
                row["id"].as_str().unwrap_or(""),
                if m.markup.subject().is_empty() { m.kind().name().to_string() } else { m.markup.subject() },
                m.kind().name(),
                if m.markup.contents().is_empty() { String::new() } else { format!(" · {}", m.markup.contents()) },
                if quantity != 1.0 { format!(" · ×{}", takeoff::row::quantity_text(quantity)) } else { String::new() },
                area.map(|a| format!(" · at [{:.3}, {:.3}, {:.3}, {:.3}]", a[0], a[1], a[2], a[3])).unwrap_or_default(),
            ));
            rows.push(row);
        }
        if rows.is_empty() {
            text = format!("No markups on sheet {}.", page + 1);
        } else {
            text.insert_str(0, &format!("{} markup(s) on sheet {}:\n", rows.len(), page + 1));
        }
        Said::ok(id, text, json!({ "page": page + 1, "markups": rows }))
    }

    /// The markups list's totals, for the assistant: the same rows and the same
    /// arithmetic as the list along the bottom of the window, so what the assistant
    /// says and what the person sees cannot disagree.
    fn desk_takeoff(&self, id: &str, args: &Value) -> Said {
        let every = args.get("every_open_drawing").and_then(|v| v.as_bool()).unwrap_or(false);
        let only = if args.get("page").is_some_and(|p| !p.is_null()) {
            match self.page_arg(args) {
                Ok(page) => Some(page),
                Err(why) => return Said::no(id, why),
            }
        } else {
            None
        };
        let docs: Vec<&crate::sheet::Doc> = if every {
            self.docs.iter().collect()
        } else {
            self.doc().into_iter().collect()
        };
        if docs.is_empty() {
            return Said::no(id, "No drawing is open in Excalibur View.");
        }
        let weights = crate::panelbody::weights();
        let mut text = String::new();
        let mut drawings = Vec::new();
        let mut pounds = 0.0;
        for doc in docs {
            let name = doc
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let rows: Vec<takeoff::Row> = doc
                .rows()
                .into_iter()
                .filter(|r| only.is_none_or(|page| r.page == page as usize))
                .collect();
            let summary = takeoff::summarise(&rows, weights);
            pounds += summary.pounds;
            let sheets = |pages: &[usize]| -> Vec<String> { pages.iter().map(|p| doc.sheet_name(*p as u32)).collect() };
            text.push_str(&format!(
                "{name}{}:\n",
                only.map(|p| format!(", sheet {}", doc.sheet_name(p))).unwrap_or_default()
            ));
            if summary.groups.is_empty() {
                text.push_str("  nothing measured\n");
            }
            let mut groups = Vec::new();
            for group in &summary.groups {
                let total = crate::panelbody::total_of(group, doc);
                let weight = group.weighed.then(|| group.pounds.round());
                text.push_str(&format!(
                    "  {} · {} · {} pick{} · {}{}{} · {}\n",
                    group.subject,
                    group.kind.name(),
                    group.picks,
                    if group.picks == 1 { "" } else { "s" },
                    total,
                    weight.map(|lb| format!(" · {} lb", thousands(lb))).unwrap_or_default(),
                    if group.unscaled > 0 { format!(" · {} left out, no scale", group.unscaled) } else { String::new() },
                    sheets(&group.pages).join(", "),
                ));
                groups.push(json!({
                    "subject": group.subject,
                    "kind": group.kind.name(),
                    "picks": group.picks,
                    "count": group.count,
                    "total": total,
                    "pounds": weight,
                    "left_out": group.unscaled,
                    "sheets": sheets(&group.pages),
                }));
            }
            if summary.pounds > 0.0 {
                text.push_str(&format!(
                    "  Total weight {} lb ({:.2} tons)\n",
                    thousands(summary.pounds.round()),
                    summary.tons()
                ));
            }
            if summary.is_short() {
                text.push_str(&format!(
                    "  {} measurement{} left out of every total: no scale on {}\n",
                    summary.unscaled,
                    if summary.unscaled == 1 { "" } else { "s" },
                    sheets(&summary.unscaled_pages).join(", ")
                ));
            }
            drawings.push(json!({
                "drawing": name,
                "groups": groups,
                "pounds": summary.pounds.round(),
                "left_out": summary.unscaled,
                "sheets_without_scale": sheets(&summary.unscaled_pages),
            }));
        }
        Said::ok(id, text.trim_end().to_string(), json!({ "drawings": drawings, "pounds": pounds.round() }))
    }

    fn desk_show(&mut self, id: &str, args: &Value) -> Said {
        if self.doc().is_none() {
            return Said::no(id, "No drawing is open in Excalibur View.");
        }
        let wanted: Vec<String> = args
            .get("markups")
            .and_then(|m| m.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let note = args.get("note").and_then(|n| n.as_str()).unwrap_or("").trim().to_string();
        let mut shown = 0;
        if !wanted.is_empty() {
            let doc = self.doc().expect("checked");
            let found: Vec<usize> = wanted.iter().filter_map(|w| find_mark(doc, w)).collect();
            if found.is_empty() {
                return Said::no(id, "None of those markups are on this drawing.");
            }
            let page = doc.marks[found[0]].page;
            let bounds = found
                .iter()
                .filter(|i| doc.marks[**i].page == page)
                .filter_map(|i| sheet_bounds(doc, *i))
                .reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]);
            self.go_to(page);
            if let Some(doc) = self.doc_mut() {
                doc.choose(Some(found[0]));
                let same: Vec<usize> =
                    found.iter().skip(1).copied().filter(|i| doc.marks[*i].page == page).collect();
                for i in same {
                    doc.choose_also(i);
                }
            }
            if let Some(b) = bounds {
                self.frame_area([b[0] - 30.0, b[1] - 30.0, b[2] + 30.0, b[3] + 30.0]);
            }
            shown = found.len();
        } else if let Some(area) = args.get("area").and_then(desk::area_arg) {
            let page = match self.page_arg(args) {
                Ok(p) => p,
                Err(why) => return Said::no(id, why),
            };
            self.go_to(page);
            if let Some(size) = self.doc().and_then(|d| d.pages.get(page as usize).copied()) {
                self.frame_area(desk::area_from_fraction(area, (size.width as f64, size.height as f64)));
            }
        } else {
            return Said::no(id, "Say which markups to show, or an area.");
        }
        if !note.is_empty() {
            self.status = format!("Assistant: {note}");
        }
        Said::ok(
            id,
            if shown > 0 { format!("Selected and showing {shown} markup(s).") } else { "Showing that area.".into() },
            json!({ "shown": shown }),
        )
    }

    fn desk_propose(&mut self, id: &str, args: &Value) -> Said {
        let Some(doc) = self.doc() else {
            return Said::no(id, "Open a drawing first: proposals go on a drawing.");
        };
        let doc_id = doc.id;
        let page = doc.page;
        let sizes: Vec<(f64, f64)> = doc.pages.iter().map(|p| (p.width as f64, p.height as f64)).collect();
        let drawing = crate::app::name_of(&doc.path);
        let marks: Vec<(String, u32, [f64; 4])> = doc
            .marks
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.gone)
            .filter_map(|(i, m)| Some((mark_id(i, m), m.page, sheet_bounds(doc, i)?)))
            .collect();
        let find = |id: &str| {
            let id = id.trim();
            marks.iter().find(|(m, _, _)| m == id).map(|(_, p, b)| (*p, *b))
        };
        let output = match desk::proposals(args, page, |p| sizes.get(p as usize).copied(), find) {
            Ok(o) => o,
            Err(why) => return Said::no(id, why),
        };
        let n = output.findings.len();
        let title = output.title.clone();
        self.plugin_shown = Some(crate::pluginui::Shown {
            plugin: ASSISTANT.into(),
            plugin_name: "Assistant".into(), // WHAT: the name shown over the assistant's proposed markups. WHY: says who proposed them in plain words.
            command: plugin_api::Command {
                id: "proposals".into(),
                name: title.clone(),
                description: "Proposed by the assistant. Nothing is drawn until someone clicks Fix.".into(),
                scope: plugin_api::Scope::Sheet,
                needs: Default::default(),
            },
            doc: doc_id,
            drawing,
            result: Ok(output),
            selected: None,
            applied: Default::default(),
            took: Duration::ZERO,
            open: true,
            hide_notes: false,
        });
        self.status = format!("The assistant proposed {n} markup(s): {title}. Nothing is drawn until you click Fix.");
        Said::ok(
            id,
            format!(
                "{n} proposal(s) are up in the Excalibur View window under \"{title}\", each outlined on the \
                 sheet with a Fix button. Nothing has been drawn; the person at the screen decides."
            ),
            json!({ "proposed": n }),
        )
    }

    fn desk_open(&mut self, ask: Ask, _ctx: &egui::Context) -> Option<Said> {
        let id = ask.id.clone();
        let page = ask.args.get("page").and_then(|p| p.as_u64()).filter(|p| *p >= 1).map(|p| p as u32 - 1);
        if let Some(set) = ask.args.get("set").and_then(|s| s.as_str()).map(str::to_string) {
            if !self.standing.signed_in() {
                return Some(Said::no(&id, "This copy of Excalibur View is not signed in to the office server, so it cannot fetch that set."));
            }
            // Already open: bring it forward.
            if let Some(at) = self.docs.iter().position(|d| d.attached.as_ref().is_some_and(|a| a.set == set)) {
                self.current = at;
                if let Some(p) = page {
                    self.go_to(p);
                }
                return Some(Said::ok(&id, "That drawing was already open; it is on screen now.", json!({})));
            }
            let name = self.standing.sets.iter().find(|s| s.id == set).map(|s| s.name.clone()).unwrap_or_else(|| set.clone());
            self.ask(crate::server::Ask::Open { set: set.clone(), name });
            self.desk_waits.push(Waiting {
                ask,
                until: Instant::now() + OPEN_ANSWER_WITHIN,
                what: Awaiting::Open { set: Some(set), path: None, page },
            });
            return None;
        }
        if let Some(file) = ask.args.get("file").and_then(|s| s.as_str()) {
            let path = PathBuf::from(file);
            if !path.is_file() {
                return Some(Said::no(&id, format!("There is no file at {file}.")));
            }
            self.open(path.clone());
            self.desk_waits.push(Waiting {
                ask,
                until: Instant::now() + OPEN_ANSWER_WITHIN,
                what: Awaiting::Open { set: None, path: Some(path), page },
            });
            return None;
        }
        Some(Said::no(&id, "Say which drawing: a set id from the office server, or a file path."))
    }

    fn desk_check(&mut self, ask: Ask, ctx: &egui::Context) -> Option<Said> {
        let id = ask.id.clone();
        let wanted = ask.args.get("command").and_then(|c| c.as_str()).unwrap_or("").trim().to_lowercase();
        let all: Vec<(String, String, String)> = self
            .plugins
            .plugins
            .iter()
            .flat_map(|p| {
                p.manifest.commands.iter().map(move |c| (p.manifest.id.clone(), c.id.clone(), c.name.clone()))
            })
            .collect();
        if all.is_empty() {
            return Some(Said::no(&id, "This copy of Excalibur View has no plugins, so there are no checks to run."));
        }
        if wanted.is_empty() {
            let list = all.iter().map(|(_, c, n)| format!("{n} ({c})")).collect::<Vec<_>>().join("; ");
            return Some(Said::ok(&id, format!("Checks that can be run: {list}."), json!({})));
        }
        let Some((plugin, command, _)) = all
            .iter()
            .find(|(_, c, n)| c.to_lowercase() == wanted || n.to_lowercase() == wanted)
            .or_else(|| all.iter().find(|(_, c, n)| n.to_lowercase().contains(&wanted) || c.to_lowercase().contains(&wanted)))
            .cloned()
        else {
            return Some(Said::no(&id, format!("No check is called \"{wanted}\".")));
        };
        if self.doc().is_none() {
            return Some(Said::no(&id, "Open a drawing first: the checks run on the drawing on screen."));
        }
        if self.plugin_job.is_some() {
            return Some(Said::no(&id, "A check is already running in the window. Ask again when it is done."));
        }
        self.start_plugin(&crate::plugins::fire_id(&plugin, &command), ctx);
        let Some(job) = self.plugin_job.as_ref().map(|j| j.id) else {
            return Some(Said::no(&id, self.status.clone()));
        };
        self.desk_waits.push(Waiting {
            ask,
            until: Instant::now() + CHECK_ANSWER_WITHIN,
            what: Awaiting::Check { job },
        });
        None
    }

    fn poll_desk_waits(&mut self) {
        let waiting = std::mem::take(&mut self.desk_waits);
        for w in waiting {
            match self.desk_ready(&w) {
                Some(said) => desk::answer(&said),
                None if Instant::now() > w.until => desk::answer(&self.desk_gave_up(&w)),
                None => self.desk_waits.push(w),
            }
        }
    }

    /// What the assistant is told when something it asked for is still going when
    /// the answer has to go back. A big set is still downloading: that is not
    /// a failure, and it will turn to the sheet asked for when it arrives.
    fn desk_gave_up(&mut self, w: &Waiting) -> Said {
        match &w.what {
            Awaiting::Open { set, path, page } => {
                self.turn_to = Some(TurnTo {
                    set: set.clone(),
                    path: path.clone(),
                    page: page.unwrap_or(0),
                    since: Instant::now(),
                });
                Said::ok(
                    &w.ask.id,
                    "Still opening — a large set takes a while to fetch. It will appear in the window on its own; \
                     ask window_status in a little while.",
                    json!({ "opening": true }),
                )
            }
            Awaiting::Check { .. } => Said::no(
                &w.ask.id,
                "The check is still running in the window. Its findings will appear there when it finishes; \
                 ask window_status later.",
            ),
        }
    }

    fn desk_ready(&mut self, w: &Waiting) -> Option<Said> {
        let id = w.ask.id.as_str();
        match &w.what {
            Awaiting::Open { set, path, page } => {
                let found = self.docs.iter().position(|d| {
                    set.as_ref().is_some_and(|s| d.attached.as_ref().is_some_and(|a| &a.set == s))
                        || path.as_ref().is_some_and(|p| &d.path == p)
                });
                let Some(at) = found else {
                    // Failing is an answer too: say why now rather than
                    // leaving the assistant to wait for something that will not come.
                    if let Some(why) = self.library_error.as_ref() {
                        return Some(Said::no(id, format!("Excalibur View cannot read PDFs on this computer: {why}")));
                    }
                    if path.is_some() && !self.opening {
                        let why = self.error.clone().unwrap_or_else(|| "it did not open".into());
                        return Some(Said::no(id, format!("That drawing did not open: {why}")));
                    }
                    return None;
                };
                self.current = at;
                if let Some(p) = page {
                    self.go_to(*p);
                }
                let doc = &self.docs[at];
                Some(Said::ok(
                    id,
                    format!(
                        "Opened {} — {} sheets, {} markups.",
                        crate::app::name_of(&doc.path),
                        doc.pages.len(),
                        doc.marks.iter().filter(|m| !m.gone).count()
                    ),
                    json!({ "sheets": doc.pages.len() }),
                ))
            }
            Awaiting::Check { job } => {
                if self.plugin_job.as_ref().is_some_and(|j| j.id == *job) {
                    return None;
                }
                let shown = self.plugin_shown.as_ref()?;
                Some(match &shown.result {
                    Err(why) => Said::no(id, format!("{} did not finish: {why}", shown.command.name)),
                    Ok(out) => Said::ok(id, findings_text(out), json!({ "findings": out.findings.len() })),
                })
            }
        }
    }

    /// Turns to the sheet a link or the assistant asked for, once its drawing is open.
    fn turn_after_open(&mut self) {
        let Some(turn) = self.turn_to.as_ref() else { return };
        if turn.since.elapsed() > Duration::from_secs(180) {
            self.turn_to = None;
            return;
        }
        let at = self.docs.iter().position(|d| {
            turn.set.as_ref().is_some_and(|s| d.attached.as_ref().is_some_and(|a| &a.set == s))
                || turn.path.as_ref().is_some_and(|p| &d.path == p)
        });
        if let Some(at) = at {
            let page = turn.page;
            self.turn_to = None;
            self.current = at;
            self.go_to(page);
        }
    }

    /// `hyperview://open?set=…&page=…` or `?project=…`: a link from FabWire,
    /// an email or anywhere else, handed to this window.
    pub fn open_link(&mut self, link: &str) {
        let Some(query) = link.split_once('?').map(|(_, q)| q) else {
            self.status = "That Excalibur View link does not say what to open.".into();
            return;
        };
        let mut set = None;
        let mut project = None;
        let mut page = None;
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = percent_decode(v);
            match k {
                "set" => set = Some(v),
                "project" => project = Some(v),
                "page" => page = v.parse::<u32>().ok().filter(|p| *p >= 1).map(|p| p - 1),
                _ => {}
            }
        }
        if !self.standing.signed_in() {
            self.status = "That link is to a drawing on the office server. Sign in to it (Studio) and open the link again.".into();
            self.panel = "Studio".into();
            return;
        }
        if let Some(set) = set.filter(|s| !s.is_empty()) {
            if let Some(at) = self.docs.iter().position(|d| d.attached.as_ref().is_some_and(|a| a.set == set)) {
                self.current = at;
                if let Some(p) = page {
                    self.go_to(p);
                }
                return;
            }
            if let Some(p) = page {
                self.turn_to = Some(TurnTo { set: Some(set.clone()), path: None, page: p, since: Instant::now() });
            }
            self.ask(crate::server::Ask::Open { set: set.clone(), name: set });
            return;
        }
        if let Some(project) = project.filter(|p| !p.is_empty()) {
            self.panel = "Studio".into();
            self.standing.looking_at = Some(project.clone());
            self.standing.busy = Some("Looking…".into());
            self.ask(crate::server::Ask::Sets(project));
            return;
        }
        self.status = "That Excalibur View link does not say what to open.".into();
    }
}

impl App {
    /// Help → Test Assistant Connection. Runs in the background: part of it is
    /// starting the connector and asking the office server, which takes a
    /// moment on a slow network.
    pub fn test_assistant(&mut self) {
        if self.assistant_testing.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.assistant_testing = Some(rx);
        self.assistant_report = Some("Checking…".into());
        let _ = std::thread::Builder::new().name("hyperview-assistant-test".into()).spawn(move || {
            let report = crate::assistant::diagnose();
            let _ = crate::assistant::write_check(&report);
            let _ = tx.send(report);
        });
    }

    pub fn assistant_window(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.assistant_testing {
            match rx.try_recv() {
                Ok(report) => {
                    self.assistant_report = Some(report);
                    self.assistant_testing = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.assistant_testing = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(200)),
            }
        }
        let Some(report) = self.assistant_report.clone() else { return };
        let mut open = true;
        let mut again = false;
        egui::Window::new("The assistant on this computer") // WHAT: the title of the connection test's report window. WHY: says what the report is about in plain words.
            .open(&mut open)
            .default_width(560.0)
            .max_width(720.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                    ui.label(egui::RichText::new(&report).monospace().size(12.0));
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Copy").clicked() {
                        ctx.copy_text(report.clone());
                    }
                    if self.assistant_testing.is_none() && ui.button("Test again").clicked() {
                        again = true;
                    }
                    if let Some(log) = crate::assistant::log_file() {
                        ui.label(egui::RichText::new(format!("Log: {}", log.display())).size(11.0));
                    }
                });
            });
        if !open {
            self.assistant_report = None;
        }
        if again {
            self.test_assistant();
        }
    }
}

/// Whether something handed to the window is a `hyperview://` link rather
/// than a drawing.
pub fn is_link(s: &str) -> bool {
    s.len() > 12 && s[..12].eq_ignore_ascii_case("hyperview://")
}

/// The name the assistant's proposals go under in the window's findings list.
pub const ASSISTANT: &str = "assistant";

/// How long the window holds an answer about opening a drawing, or running
/// a check, before saying it is still going. Shorter than the connector
/// waits (`assistant::window_call`), which is shorter than the assistant waits.
pub const OPEN_ANSWER_WITHIN: Duration = Duration::from_secs(40);
pub const CHECK_ANSWER_WITHIN: Duration = Duration::from_secs(42);

/// A markup's id, as the assistant is told it: its name in the file when it has one,
/// its position when it does not.
fn mark_id(index: usize, mark: &crate::sheet::Mark) -> String {
    let name = mark.markup.name();
    if name.is_empty() { format!("#{index}") } else { name }
}

fn find_mark(doc: &crate::sheet::Doc, id: &str) -> Option<usize> {
    let index = match id.strip_prefix('#') {
        Some(n) => n.parse::<usize>().ok(),
        None => doc.marks.iter().position(|m| !m.gone && m.markup.name() == id),
    }?;
    doc.marks.get(index).filter(|m| !m.gone).map(|_| index)
}

/// Where a markup is, in its sheet's own units.
fn sheet_bounds(doc: &crate::sheet::Doc, index: usize) -> Option<[f64; 4]> {
    let mark = doc.marks.get(index)?;
    let b = mark.markup.bounds()?;
    let frame = doc.frame_of(mark.page);
    let p = frame.to_sheet([b[0], b[1]]);
    let q = frame.to_sheet([b[2], b[3]]);
    Some([p[0].min(q[0]), p[1].min(q[1]), p[0].max(q[0]), p[1].max(q[1])])
}

fn findings_text(out: &plugin_api::Output) -> String {
    let mut text = format!("{}", out.title);
    if !out.summary.is_empty() {
        text.push_str(&format!("\n{}", out.summary));
    }
    if out.findings.is_empty() {
        text.push_str("\nNothing found.");
    }
    for f in out.findings.iter().take(200) {
        let level = match f.level {
            plugin_api::Level::Problem => "PROBLEM",
            plugin_api::Level::Check => "CHECK",
            plugin_api::Level::Note => "NOTE",
        };
        text.push_str(&format!(
            "\n- {level}{}: {}{}",
            f.page.map(|p| format!(" (sheet {})", p + 1)).unwrap_or_default(),
            f.message,
            if f.fixes.is_empty() { String::new() } else { format!(" [fix offered: {}]", f.fixes[0].label) }
        ));
    }
    if out.findings.len() > 200 {
        text.push_str(&format!("\n… and {} more, in the window.", out.findings.len() - 200));
    }
    for t in out.tables.iter().take(3) {
        text.push_str(&format!("\n\n{} ({} rows)\n{}", t.title, t.rows.len(), t.columns.join(" | ")));
        for r in t.rows.iter().take(40) {
            text.push_str(&format!("\n{}", r.join(" | ")));
        }
    }
    text
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_from_the_office_goes_by_its_own_name() {
        use crate::app::{name_of, without_digest};
        assert_eq!(without_digest("8c0028fc2425-S-101 Framing Plan.pdf"), Some("S-101 Framing Plan.pdf"));
        assert_eq!(without_digest("S-101-A.pdf"), None, "a sheet number is not a digest");
        assert_eq!(without_digest("deadbeefcafe-"), None);
        let cached = crate::server::cache_folder().join("8c0028fc2425-S-101.pdf");
        assert_eq!(name_of(&cached), "S-101.pdf");
        let elsewhere = std::path::Path::new("/jobs/8c0028fc2425-S-101.pdf");
        assert_eq!(name_of(elsewhere), "8c0028fc2425-S-101.pdf", "only the cache is renamed");
    }

    #[test]
    fn a_link_is_read_the_way_it_was_written() {
        assert_eq!(percent_decode("set_abc%2D1"), "set_abc-1");
        assert_eq!(percent_decode("S-101+Framing"), "S-101 Framing");
        assert_eq!(percent_decode("100%"), "100%", "a stray percent is kept, not dropped");
    }
}

/// 12345.0 as "12,345".
fn thousands(value: f64) -> String {
    let digits = format!("{:.0}", value.abs());
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if value < 0.0 {
        out.insert(0, '-');
    }
    out
}
