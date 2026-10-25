//! Running a plugin, showing what it found, and carrying out the fixes it
//! offers. The plugin itself never touches the drawing; everything here that
//! changes one does so because somebody clicked, and each click is one step
//! on the Undo list.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use egui::{Color32, RichText, Stroke};
use plugin_api::{Action, Command, Input, Level, Output, Scope};

use crate::app::App;
use crate::plugins::{self, from_fire_id};
use crate::render::{Reading, ToWorker};

/// A plugin run under way: waiting for sheets to be read, then running.
pub struct Job {
    pub id: u64,
    pub plugin: String,
    pub plugin_name: String,
    pub command: Command,
    pub doc: u64,
    pub waiting: BTreeSet<u32>,
    pub of: usize,
    pub readings: BTreeMap<u32, Reading>,
    pub input: Input,
    pub running: Option<mpsc::Receiver<Result<Output, String>>>,
    pub started: Instant,
    pub ctx: egui::Context,
    /// Not a run: the input is written here, for somebody writing a plugin.
    pub save_to: Option<std::path::PathBuf>,
}

/// What a plugin found, on screen.
pub struct Shown {
    pub plugin: String,
    pub plugin_name: String,
    pub command: Command,
    pub doc: u64,
    pub drawing: String,
    pub result: Result<Output, String>,
    pub selected: Option<usize>,
    /// Fixes already carried out, as (finding, fix).
    pub applied: HashSet<(usize, usize)>,
    pub took: Duration,
    pub open: bool,
    /// Which levels are showing.
    pub hide_notes: bool,
}

fn level_colour(level: Level) -> Color32 {
    match level {
        Level::Problem => Color32::from_rgb(225, 70, 60),
        Level::Check => Color32::from_rgb(235, 160, 40),
        Level::Note => Color32::from_rgb(90, 160, 235),
    }
}

fn level_word(level: Level) -> &'static str {
    match level {
        Level::Problem => "PROBLEM",
        Level::Check => "CHECK",
        Level::Note => "NOTE",
    }
}

impl App {
    /// Reads the plugins folders again and rebuilds the menu.
    pub fn reload_plugins(&mut self) {
        self.plugins = if crate::edition::runs_plugins_itself() {
            plugins::Shelf::load(&crate::install::plugin_trusted())
        } else {
            plugins::Shelf::from_office(&self.standing.plugins)
        };
        self.chrome.plugins = self.plugins.menu();
    }

    /// A Plugins menu item was clicked.
    pub fn start_plugin(&mut self, fire: &str, ctx: &egui::Context) {
        let Some((plugin_id, command_id)) = from_fire_id(fire) else { return };
        if self.plugin_job.is_some() {
            self.status = "A plugin is already running. It will say when it is done.".into();
            return;
        }
        let Some(plugin) = self.plugins.find(&plugin_id).cloned() else {
            self.status = "That plugin is no longer on this computer.".into();
            return;
        };
        let Some(command) = plugin.manifest.commands.iter().find(|c| c.id == command_id).cloned() else {
            return;
        };
        let Some(doc) = self.doc() else {
            self.status = format!("Open a drawing first, then run {}.", command.name);
            return;
        };
        let doc_id = doc.id;
        let pages: Vec<u32> = match command.scope {
            Scope::Sheet => vec![doc.page],
            Scope::Drawing => (0..doc.pages.len() as u32).collect(),
        };
        let settings = plugins::settings_for(&plugin.manifest, &self.plugin_settings);
        let input = self.plugin_input(&command, &pages, settings);
        self.next_plugin_job += 1;
        let id = self.next_plugin_job;
        let read = command.needs.words || command.needs.lines;
        if read {
            for page in &pages {
                self.svc.send(ToWorker::Read {
                    doc: doc_id,
                    page: *page,
                    job: id,
                    words: command.needs.words,
                    lines: command.needs.lines,
                });
            }
        }
        log::info!("running plugin {} {} on {} sheet(s)", plugin.manifest.id, command.id, pages.len());
        self.plugin_job = Some(Job {
            id,
            plugin: plugin.manifest.id.clone(),
            plugin_name: plugin.manifest.name.clone(),
            command,
            doc: doc_id,
            waiting: if read { pages.iter().copied().collect() } else { BTreeSet::new() },
            of: pages.len(),
            readings: BTreeMap::new(),
            input,
            running: None,
            started: Instant::now(),
            ctx: ctx.clone(),
            save_to: None,
        });
        self.launch_plugin_if_ready();
    }

    /// Plugins ▸ Save This Sheet (or This Drawing) for a Plugin Test…
    ///
    /// Reads the sheet on screen, or every sheet, exactly as a command that
    /// wants words and line-work would, and writes the input to a file
    /// instead of handing it to anything. Somebody writing a plugin tests
    /// against that file, with no signing and no program in the way.
    pub fn save_plugin_input(&mut self, ctx: &egui::Context, whole: bool) {
        if self.plugin_job.is_some() {
            self.status = "A plugin is running. Save the sheet once it has finished.".into();
            return;
        }
        let Some(doc) = self.doc() else {
            self.status = "Open a drawing first, then save a sheet from it.".into();
            return;
        };
        let (doc_id, page) = (doc.id, doc.page);
        let pages: Vec<u32> = if whole { (0..doc.pages.len() as u32).collect() } else { vec![page] };
        let stem = doc
            .path
            .file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "sheet".into());
        let choose = crate::files::Choose::save(if whole {
            format!("{stem}.json")
        } else {
            format!("{stem} - sheet {}.json", page + 1)
        })
        .title(if whole { "Save this drawing for a plugin test" } else { "Save this sheet for a plugin test" })
        .filter("Plugin input (JSON)", &["json"]);
        let ctx = ctx.clone();
        self.filing.one(choose, move |app, save_to| {
            app.save_plugin_input_to(&ctx, whole, doc_id, pages, save_to)
        });
    }

    fn save_plugin_input_to(
        &mut self,
        ctx: &egui::Context,
        whole: bool,
        doc_id: u64,
        pages: Vec<u32>,
        save_to: std::path::PathBuf,
    ) {
        if self.plugin_job.is_some() {
            self.status = "A plugin is running. Save the sheet once it has finished.".into();
            return;
        }
        let command = Command {
            id: "test".into(),
            name: if whole { "Saving the drawing".into() } else { "Saving the sheet".into() },
            description: String::new(),
            scope: if whole { Scope::Drawing } else { Scope::Sheet },
            needs: plugin_api::Needs { words: true, lines: true },
        };
        let input = self.plugin_input(&command, &pages, BTreeMap::new());
        self.next_plugin_job += 1;
        let id = self.next_plugin_job;
        for page in &pages {
            self.svc.send(ToWorker::Read { doc: doc_id, page: *page, job: id, words: true, lines: true });
        }
        self.plugin_job = Some(Job {
            id,
            plugin: String::new(),
            plugin_name: "Plugin test".into(),
            command,
            doc: doc_id,
            waiting: pages.iter().copied().collect(),
            of: pages.len(),
            readings: BTreeMap::new(),
            input,
            running: None,
            started: Instant::now(),
            ctx: ctx.clone(),
            save_to: Some(save_to),
        });
    }

    /// Everything a plugin reads, bar the words and line-work, which the
    /// drawing engine reads separately.
    pub fn plugin_input(
        &self,
        command: &Command,
        pages: &[u32],
        settings: BTreeMap<String, serde_json::Value>,
    ) -> Input {
        let Some(doc) = self.doc() else {
            return Input::default();
        };
        let user = crate::panelbody::user_columns();
        let weights = crate::panelbody::weights();
        let wanted: HashSet<u32> = pages.iter().copied().collect();
        let sheets = pages
            .iter()
            .map(|&page| {
                let size = doc.pages.get(page as usize).copied();
                let name = match doc.labels.get(page as usize) {
                    Some(l) if !l.number.is_empty() && !l.title.is_empty() => format!("{} {}", l.number, l.title),
                    _ => doc.sheet_name(page),
                };
                plugin_api::Sheet {
                    page,
                    name,
                    width: size.map(|s| s.width as f64).unwrap_or(0.0),
                    height: size.map(|s| s.height as f64).unwrap_or(0.0),
                    scale: doc.scale_of(page).map(|m| plugins::scale_of(&m)),
                    ..Default::default()
                }
            })
            .collect();
        let mut markups = Vec::new();
        for (index, mark) in doc.live() {
            if !wanted.contains(&mark.page) {
                continue;
            }
            let Some(row) = doc.row(index) else { continue };
            let frame = doc.frame_of(mark.page);
            let feet = doc
                .scale_of(mark.page)
                .map(|m| plugins::feet_per_unit(&m))
                .unwrap_or(1.0);
            markups.push(plugin_api::Markup {
                id: plugins::markup_id(index, &mark.markup),
                page: mark.page,
                kind: plugins::kind_word(row.kind).into(),
                subject: row.subject.clone(),
                label: row.label.clone(),
                comments: row.comments.clone(),
                author: row.author.clone(),
                points: mark.on_sheet(&frame),
                length: row.length.filter(|_| row.scaled).map(|l| l * feet),
                area: row.area.filter(|_| row.scaled).map(|a| a * feet * feet),
                count: row.count,
                quantity: row.quantity,
                slope: row.slope.map(|rise| [rise, row.pitch_run]),
                columns: row.columns.to_vec(),
                pounds: row.pounds(weights),
            });
        }
        Input {
            command: command.id.clone(),
            drawing: doc
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            current_page: doc.page,
            sheets,
            markups,
            settings,
            columns: (0..6).map(|n| takeoff::user::name_of(&user, n)).collect(),
        }
    }

    /// A sheet came back from the drawing engine.
    pub fn plugin_read(&mut self, job: u64, page: u32, reading: Reading) {
        let Some(running) = self.plugin_job.as_mut() else { return };
        if running.id != job {
            return;
        }
        running.waiting.remove(&page);
        running.readings.insert(page, reading);
        self.launch_plugin_if_ready();
    }

    fn launch_plugin_if_ready(&mut self) {
        let Some(job) = self.plugin_job.as_mut() else { return };
        if !job.waiting.is_empty() || job.running.is_some() {
            return;
        }
        for sheet in job.input.sheets.iter_mut() {
            if let Some(reading) = job.readings.remove(&sheet.page) {
                sheet.words = reading.words;
                sheet.lines = reading.lines;
                sheet.lines_cut_short = reading.cut_short;
            }
        }
        if let Some(path) = job.save_to.take() {
            let written = serde_json::to_vec_pretty(&job.input)
                .map_err(|e| e.to_string())
                .and_then(|bytes| std::fs::write(&path, bytes).map_err(|e| e.to_string()));
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            self.status = match written {
                Ok(()) => format!("Saved {name} for a plugin test."),
                Err(e) => format!("Could not save {name}: {e}"),
            };
            self.plugin_job = None;
            return;
        }
        let Some(plugin) = self.plugins.find(&job.plugin).cloned() else {
            self.plugin_job = None;
            return;
        };
        let input = std::mem::take(&mut job.input);
        let (tx, rx) = mpsc::channel();
        job.running = Some(rx);
        // A copy that doesn't run plugins itself has the office server run
        // this one, in the same sandbox, and waits for what it found.
        if !crate::edition::runs_plugins_itself() {
            self.plugin_answer = Some(tx);
            self.ask(crate::server::Ask::RunPlugin { id: plugin.manifest.id.clone(), input });
            return;
        }
        let ctx = job.ctx.clone();
        let wasm = plugin.wasm.clone();
        std::thread::Builder::new()
            .name(format!("plugin {}", plugin.manifest.id))
            .spawn(move || {
                let result = std::panic::catch_unwind(|| plugins::run(&wasm, &input))
                    .unwrap_or_else(|_| Err("The plugin could not be run.".into()));
                let _ = tx.send(result);
                ctx.request_repaint();
            })
            .ok();
    }

    /// Picks up a finished run. Called every frame.
    pub fn poll_plugin(&mut self) {
        let Some(job) = self.plugin_job.as_ref() else { return };
        // The drawing it was reading was closed: nothing more is coming.
        if job.running.is_none() && !self.docs.iter().any(|d| d.id == job.doc) {
            self.plugin_job = None;
            return;
        }
        let Some(rx) = job.running.as_ref() else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("The plugin stopped without answering.".into()),
        };
        let job = self.plugin_job.take().unwrap();
        let took = job.started.elapsed();
        log::info!("plugin {} {} took {:.1}s", job.plugin, job.command.id, took.as_secs_f64());
        let drawing = self
            .docs
            .iter()
            .find(|d| d.id == job.doc)
            .and_then(|d| d.path.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_default();
        self.status = match &result {
            Ok(o) => format!("{}: {} found.", job.command.name, count_words(o)),
            Err(_) => format!("{} did not finish.", job.command.name),
        };
        self.plugin_shown = Some(Shown {
            plugin: job.plugin,
            plugin_name: job.plugin_name,
            command: job.command,
            doc: job.doc,
            drawing,
            result,
            selected: None,
            applied: HashSet::new(),
            took,
            open: true,
            hide_notes: false,
        });
    }

    /// Plugins ▸ Add Plugin…
    pub fn add_plugin(&mut self) {
        if !crate::edition::runs_plugins_itself() {
            self.status = "Plugins come from your office's server on this copy. \
                           An administrator adds them from Excalibur View on Windows."
                .into();
            return;
        }
        self.filing.one(
            crate::files::Choose::open()
                .title("Add a plugin")
                .filter("Excalibur View plugin", &[plugins::EXTENSION]),
            |app, path| app.add_plugin_from(&path),
        );
    }

    fn add_plugin_from(&mut self, path: &std::path::Path) {
        match plugins::add_from_file(&crate::install::plugin_trusted(), path) {
            Err(why) => self.status = why,
            Ok((plugin, _)) => {
                let name = format!("{} {}", plugin.manifest.name, plugin.manifest.version);
                self.reload_plugins();
                let admin = self.standing.who.as_ref().map(|w| w.role) == Some(hub::Role::Admin);
                if admin && !self.standing.base.is_empty() {
                    self.status = format!("{name} added. Sharing it with the office…");
                    self.ask(crate::server::Ask::SharePlugin(plugin.path));
                } else {
                    self.status = format!("{name} added on this computer. It is in the Plugins menu.");
                }
            }
        }
    }

    /// The plugin windows, drawn every frame.
    pub fn plugin_windows(&mut self, ctx: &egui::Context) {
        self.poll_plugin();
        self.plugin_progress(ctx);
        self.plugin_results(ctx);
        self.plugins_manager(ctx);
    }

    fn plugin_progress(&mut self, ctx: &egui::Context) {
        let Some(job) = self.plugin_job.as_ref() else { return };
        let theme = self.chrome.theme;
        let what = if job.running.is_some() {
            format!("{} is working…", job.command.name)
        } else {
            format!(
                "Reading sheet {} of {}…",
                (job.of - job.waiting.len() + 1).min(job.of),
                job.of
            )
        };
        let mut stop = false;
        egui::Window::new(format!("{} — {}", job.plugin_name, job.command.name))
            .id(egui::Id::new("plugin running"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -64.0))
            .max_width(560.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(what);
                });
                ui.label(
                    RichText::new(format!("{:.0} s", job.started.elapsed().as_secs_f64()))
                        .color(theme.faint)
                        .size(11.0),
                );
                if job.running.is_none() && ui.button("Stop").clicked() {
                    stop = true;
                }
            });
        if stop {
            self.plugin_job = None;
            self.status = "Stopped.".into();
        }
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn plugin_results(&mut self, ctx: &egui::Context) {
        let Some(mut shown) = self.plugin_shown.take() else { return };
        let theme = self.chrome.theme;
        let manifest = self.plugins.find(&shown.plugin).map(|p| p.manifest.clone());
        let mut settings = manifest
            .as_ref()
            .map(|m| plugins::settings_for(m, &self.plugin_settings))
            .unwrap_or_default();
        let mut settings_changed = false;
        let mut run_again = false;
        let mut go: Option<(u32, Option<[f64; 4]>)> = None;
        let mut fix: Vec<(usize, usize)> = Vec::new();
        let mut open = shown.open;
        let current_doc = self.doc().map(|d| d.id);
        let same_drawing = current_doc == Some(shown.doc);
        let sheet_names: BTreeMap<u32, String> = self
            .docs
            .iter()
            .find(|d| d.id == shown.doc)
            .map(|d| (0..d.pages.len() as u32).map(|p| (p, d.sheet_name(p))).collect())
            .unwrap_or_default();

        egui::Window::new(format!("{} — {}", shown.plugin_name, shown.command.name))
            .id(egui::Id::new("plugin results"))
            .open(&mut open)
            .default_width(620.0)
            .default_height(520.0)
            .resizable(true)
            .show(ctx, |ui| {
                // What can be set, and running again with it.
                if let Some(m) = manifest.as_ref().filter(|m| !m.settings.is_empty()) {
                    egui::CollapsingHeader::new(RichText::new("Settings").size(12.0))
                        .default_open(false)
                        .show(ui, |ui| {
                            for setting in &m.settings {
                                ui.horizontal(|ui| {
                                    let label = ui.label(RichText::new(&setting.name).size(12.0));
                                    if !setting.help.is_empty() {
                                        label.on_hover_text(&setting.help);
                                    }
                                    let value = settings
                                        .entry(setting.id.clone())
                                        .or_insert_with(|| setting.default_value());
                                    match &setting.kind {
                                        plugin_api::SettingKind::Number { min, max, unit, .. } => {
                                            let mut n = value.as_f64().unwrap_or(0.0);
                                            let mut drag = egui::DragValue::new(&mut n).speed(0.1).max_decimals(3);
                                            if let (Some(lo), Some(hi)) = (min, max) {
                                                drag = drag.range(*lo..=*hi);
                                            }
                                            if !unit.is_empty() {
                                                drag = drag.suffix(format!(" {unit}"));
                                            }
                                            if ui.add(drag).changed() {
                                                *value = serde_json::json!(n);
                                                settings_changed = true;
                                            }
                                        }
                                        plugin_api::SettingKind::Text { .. } => {
                                            let mut t = value.as_str().unwrap_or_default().to_string();
                                            if ui.text_edit_singleline(&mut t).changed() {
                                                *value = serde_json::json!(t);
                                                settings_changed = true;
                                            }
                                        }
                                        plugin_api::SettingKind::Toggle { .. } => {
                                            let mut b = value.as_bool().unwrap_or(false);
                                            if ui.checkbox(&mut b, "").changed() {
                                                *value = serde_json::json!(b);
                                                settings_changed = true;
                                            }
                                        }
                                    }
                                });
                            }
                        });
                }
                ui.horizontal(|ui| {
                    // The assistant's proposals are not a plugin: there is nothing to
                    // run again, only things to accept or leave.
                    if shown.plugin != crate::deskui::ASSISTANT && ui.button("Run again").clicked() {
                        run_again = true;
                    }
                    ui.label(
                        RichText::new(format!("{} · {:.1} s", shown.drawing, shown.took.as_secs_f64()))
                            .color(theme.faint)
                            .size(11.0),
                    );
                });
                ui.separator();

                let output = match &shown.result {
                    Err(why) => {
                        ui.label(RichText::new(why).color(theme.warn));
                        return;
                    }
                    Ok(output) => output.clone(),
                };
                ui.label(RichText::new(&output.title).strong().size(14.0));
                if !output.summary.is_empty() {
                    ui.label(RichText::new(&output.summary).size(12.0));
                }
                let counts = |level: Level| output.findings.iter().filter(|f| f.level == level).count();
                ui.horizontal(|ui| {
                    for level in [Level::Problem, Level::Check, Level::Note] {
                        let n = counts(level);
                        if n > 0 {
                            ui.label(
                                RichText::new(format!("{n} {}", level_word(level).to_lowercase()))
                                    .color(level_colour(level))
                                    .size(12.0),
                            );
                        }
                    }
                    if counts(Level::Note) > 0 {
                        ui.checkbox(&mut shown.hide_notes, "Hide notes");
                    }
                });
                if !same_drawing {
                    ui.label(
                        RichText::new(format!("Switch back to {} to go to these or fix them.", shown.drawing))
                            .color(theme.warn)
                            .size(11.0),
                    );
                }

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height((ui.available_height() - if output.tables.is_empty() { 0.0 } else { 160.0 }).max(120.0))
                    .id_salt("findings")
                    .show(ui, |ui| {
                        for (i, finding) in output.findings.iter().enumerate() {
                            if shown.hide_notes && finding.level == Level::Note {
                                continue;
                            }
                            let chosen = shown.selected == Some(i);
                            let frame = egui::Frame::new()
                                .inner_margin(egui::Margin::symmetric(6, 4))
                                .corner_radius(3.0)
                                .fill(if chosen { theme.hover } else { Color32::TRANSPARENT });
                            // The row is clickable (it goes to the finding),
                            // and so are the Show and Fix buttons in it. The
                            // row's click is registered underneath its
                            // buttons, or egui gives every click to the row
                            // and the buttons can never be pressed.
                            let row = ui.scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
                                frame.show(ui, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(
                                            RichText::new(level_word(finding.level))
                                                .color(level_colour(finding.level))
                                                .size(10.0)
                                                .strong(),
                                        );
                                        if let Some(page) = finding.page {
                                            let name = sheet_names.get(&page).cloned().unwrap_or_else(|| format!("Sheet {}", page + 1));
                                            ui.label(RichText::new(name).color(theme.faint).size(11.0));
                                        }
                                    });
                                    ui.label(RichText::new(&finding.message).size(12.0));
                                    ui.horizontal_wrapped(|ui| {
                                        if let Some(page) = finding.page {
                                            if ui.add_enabled(same_drawing, egui::Button::new("Show")).clicked() {
                                                shown.selected = Some(i);
                                                go = Some((page, finding.area));
                                            }
                                        }
                                        for (j, f) in finding.fixes.iter().enumerate() {
                                            let done = shown.applied.contains(&(i, j));
                                            let label = if done { format!("✔ {}", f.label) } else { f.label.clone() };
                                            if ui
                                                .add_enabled(same_drawing && !done, egui::Button::new(label))
                                                .clicked()
                                            {
                                                shown.selected = Some(i);
                                                fix.push((i, j));
                                            }
                                        }
                                    });
                                });
                            });
                            if row.response.clicked() {
                                shown.selected = Some(i);
                                if let Some(page) = finding.page {
                                    if same_drawing {
                                        go = Some((page, finding.area));
                                    }
                                }
                            }
                            ui.add_space(2.0);
                        }
                        if output.findings.is_empty() {
                            ui.label(RichText::new("Nothing to report.").color(theme.faint));
                        }
                    });

                for (t, table) in output.tables.iter().enumerate() {
                    ui.separator();
                    egui::CollapsingHeader::new(RichText::new(&table.title).strong().size(12.0))
                        .id_salt(("plugin table", t))
                        .default_open(true)
                        .show(ui, |ui| {
                            egui::ScrollArea::both()
                                .id_salt(("table scroll", t))
                                .max_height(220.0)
                                .show(ui, |ui| {
                                    egui::Grid::new(("plugin grid", t)).striped(true).show(ui, |ui| {
                                        for c in &table.columns {
                                            ui.label(RichText::new(c).strong().size(11.0));
                                        }
                                        ui.end_row();
                                        for row in &table.rows {
                                            for cell in row {
                                                ui.label(RichText::new(cell).size(11.0));
                                            }
                                            ui.end_row();
                                        }
                                        if !table.totals.is_empty() {
                                            for cell in &table.totals {
                                                ui.label(RichText::new(cell).strong().size(11.0));
                                            }
                                            ui.end_row();
                                        }
                                    });
                                });
                            ui.horizontal(|ui| {
                                if ui.button("Copy for Excel").clicked() {
                                    ui.ctx().copy_text(table.to_csv().replace(',', "\t"));
                                }
                                if ui.button("Save as CSV…").clicked() {
                                    let csv = table.to_csv();
                                    self.filing.one(
                                        crate::files::Choose::save(format!("{}.csv", table.title))
                                            .filter("CSV", &["csv"]),
                                        move |app, path| {
                                            if let Err(e) = std::fs::write(&path, csv) {
                                                app.error = Some(format!("Could not write {}: {e}", path.display()));
                                            }
                                        },
                                    );
                                }
                            });
                        });
                }
            });

        shown.open = open;
        if settings_changed {
            self.plugin_settings.insert(shown.plugin.clone(), settings);
            plugins::save_settings(&self.plugin_settings);
        }
        if let Some((page, area)) = go {
            self.go_to(page);
            if let Some(area) = area {
                let canvas = self.last_canvas;
                if let Some(doc) = self.doc_mut() {
                    doc.view.zoom = doc.view.zoom.max(0.5);
                    doc.view.centre_on([(area[0] + area[2]) * 0.5, (area[1] + area[3]) * 0.5], canvas);
                }
            }
        }
        for (i, j) in fix {
            self.apply_plugin_fix(&mut shown, i, j);
        }
        let fire = run_again.then(|| plugins::fire_id(&shown.plugin, &shown.command.id));
        if open {
            self.plugin_shown = Some(shown);
        }
        if let Some(fire) = fire {
            self.start_plugin(&fire, ctx);
        }
    }

    /// Carries out one fix, as one step on the Undo list.
    fn apply_plugin_fix(&mut self, shown: &mut Shown, finding: usize, which: usize) {
        let Ok(output) = &shown.result else { return };
        let Some(fix) = output.findings.get(finding).and_then(|f| f.fixes.get(which)).cloned() else {
            return;
        };
        if self.doc().map(|d| d.id) != Some(shown.doc) {
            return;
        }
        if let Some(doc) = self.doc_mut() {
            doc.checkpoint_named(&format!("{}: {}", shown.command.name, fix.label));
        }
        let mut trouble = Vec::new();
        for action in &fix.actions {
            if let Err(why) = self.carry_out(action) {
                trouble.push(why);
            }
        }
        shown.applied.insert((finding, which));
        self.status = if trouble.is_empty() {
            format!("{} — done. Undo takes it back.", fix.label)
        } else {
            trouble.join(" ")
        };
    }

    /// One action a plugin asked for. Only ever called from a click.
    pub fn carry_out(&mut self, action: &Action) -> Result<(), String> {
        let (units, denominator) = (self.units, self.denominator);
        let template = match action {
            Action::AddLength { subject, .. } => self.chest_tool_for(subject, annot::Kind::Length),
            Action::AddCount { subject, .. } => self.chest_tool_for(subject, annot::Kind::Count),
            Action::AddArea { subject, .. } => self.chest_tool_for(subject, annot::Kind::Area),
            _ => None,
        };
        let doc = self.doc_mut().ok_or("No drawing is open.")?;
        let find = |doc: &crate::sheet::Doc, id: &str| -> Result<usize, String> {
            let index = match id.strip_prefix('#') {
                Some(n) => n.parse::<usize>().ok(),
                None => doc.marks.iter().position(|m| !m.gone && m.markup.name() == id),
            };
            index
                .filter(|i| doc.marks.get(*i).map(|m| !m.gone).unwrap_or(false))
                .ok_or_else(|| "A markup the plugin named is not on the drawing any more.".to_string())
        };
        match action {
            Action::SetSheetScale { page, ratio, text } => {
                if *page as usize >= doc.pages.len() || !ratio.is_finite() || *ratio <= 0.0 {
                    return Err("The plugin asked for a scale that can't be set.".into());
                }
                let measure = crate::scale::named(text, *ratio, units, denominator);
                doc.set_scale(*page, Some(measure));
            }
            Action::SetSubject { markup, subject } => {
                let i = find(doc, markup)?;
                doc.marks[i].markup.set_subject(subject);
                doc.marks[i].changed = true;
            }
            Action::SetColumn { markup, column, value } => {
                let i = find(doc, markup)?;
                takeoff::row::set_column(&mut doc.marks[i].markup, *column as usize, value);
                doc.marks[i].changed = true;
            }
            Action::SetQuantity { markup, quantity } => {
                let i = find(doc, markup)?;
                takeoff::row::set_quantity(&mut doc.marks[i].markup, *quantity);
                doc.marks[i].changed = true;
            }
            Action::SetSlope { markup, rise, run } => {
                let i = find(doc, markup)?;
                takeoff::row::set_pitch(&mut doc.marks[i].markup, *rise, *run);
                doc.marks[i].changed = true;
                let page = doc.marks[i].page;
                if let Some(scale) = doc.scale_of(page) {
                    let kind = doc.marks[i].kind();
                    let caption = crate::sheet::caption_for(&doc.marks[i].markup, kind, &scale);
                    doc.marks[i].markup.set_contents(&caption);
                }
            }
            Action::AddLength { page, points, subject } => {
                if *page as usize >= doc.pages.len() || points.len() < 2 {
                    return Err("The plugin asked for a length that can't be drawn.".into());
                }
                let draft = crate::sheet::Draft {
                    tool: crate::app::Tool::Length,
                    points: points.clone(),
                    subject: subject.clone(),
                    template,
                    ..Default::default()
                };
                doc.place_quietly(*page, draft)
                    .ok_or("The length could not be drawn.")?;
            }
            Action::AddCount { page, points, subject } => {
                if *page as usize >= doc.pages.len() || points.is_empty() {
                    return Err("The count asked for has nothing to count.".into());
                }
                // One count markup per item, drawn as the ring a click draws,
                // so each can be moved or deleted on its own afterwards.
                for at in points {
                    let ring: Vec<[f64; 2]> = (0..24)
                        .map(|k| {
                            let a = k as f64 / 24.0 * std::f64::consts::TAU;
                            [at[0] + 9.0 * a.cos(), at[1] + 9.0 * a.sin()]
                        })
                        .collect();
                    let draft = crate::sheet::Draft {
                        tool: crate::app::Tool::Count,
                        points: ring,
                        subject: subject.clone(),
                        template: template.clone(),
                        ..Default::default()
                    };
                    doc.place_quietly(*page, draft)
                        .ok_or("The count could not be drawn.")?;
                }
            }
            Action::AddArea { page, points, subject } => {
                if *page as usize >= doc.pages.len() || points.len() < 3 {
                    return Err("An area needs at least three corners.".into());
                }
                let draft = crate::sheet::Draft {
                    tool: crate::app::Tool::Area,
                    points: points.clone(),
                    subject: subject.clone(),
                    template,
                    ..Default::default()
                };
                doc.place_quietly(*page, draft)
                    .ok_or("The area could not be drawn.")?;
            }
            Action::AddCloud { page, area, note } | Action::AddText { page, area, text: note } => {
                if *page as usize >= doc.pages.len() || !area.iter().all(|v| v.is_finite()) {
                    return Err("That is not a place on the sheet.".into());
                }
                let tool = if matches!(action, Action::AddCloud { .. }) {
                    crate::app::Tool::Cloud
                } else {
                    crate::app::Tool::Text
                };
                let draft = crate::sheet::Draft {
                    tool,
                    points: vec![[area[0], area[1]], [area[2], area[3]]],
                    says: note.clone(),
                    ..Default::default()
                };
                doc.place_quietly(*page, draft)
                    .ok_or("The markup could not be drawn.")?;
            }
        }
        doc.dirty = true;
        Ok(())
    }

    /// The office's tool for a subject, so a member a plugin takes off is
    /// the same markup the estimator would have drawn — weight and all.
    fn chest_tool_for(&self, subject: &str, kind: annot::Kind) -> Option<pdf::Dict> {
        let wanted = subject.trim().to_lowercase().replace(' ', "");
        let fits = |k: annot::Kind| match kind {
            annot::Kind::Length | annot::Kind::Polylength => {
                matches!(k, annot::Kind::Length | annot::Kind::Polylength)
            }
            other => k == other,
        };
        self.profile.as_ref()?.sets.iter().flat_map(|s| s.tools.iter()).find_map(|t| {
            let theirs = t.subject.trim().to_lowercase().replace(' ', "");
            (theirs == wanted && fits(t.kind)).then(|| t.annotation.clone())
        })
    }

    /// Plugins ▸ Manage Plugins…
    fn plugins_manager(&mut self, ctx: &egui::Context) {
        if !self.managing_plugins {
            return;
        }
        let theme = self.chrome.theme;
        let mut open = true;
        let admin = self.standing.who.as_ref().map(|w| w.role) == Some(hub::Role::Admin);
        let connected = !self.standing.base.is_empty() && self.standing.who.is_some();
        let mut remove_here: Option<std::path::PathBuf> = None;
        let mut share: Option<std::path::PathBuf> = None;
        let mut take_off: Option<String> = None;
        let mut add = false;
        egui::Window::new("Plugins")
            .open(&mut open)
            .default_width(520.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "An office's own tools. Each one is signed by its publisher and runs \
                         sealed off: it can't open files, reach the internet, or change a drawing \
                         unless you click one of the fixes it offers.",
                    )
                    .color(theme.faint)
                    .size(11.0),
                );
                ui.separator();
                if self.plugins.plugins.is_empty() {
                    ui.label("No plugins on this computer.");
                }
                for plugin in &self.plugins.plugins {
                    let m = &plugin.manifest;
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&m.name).strong());
                        ui.label(RichText::new(&m.version).color(theme.faint));
                        ui.label(
                            RichText::new(if plugin.from_office { "from the office" } else { "on this computer" })
                                .color(theme.faint)
                                .size(11.0),
                        );
                    });
                    if !m.description.is_empty() {
                        ui.label(RichText::new(&m.description).size(12.0));
                    }
                    ui.label(
                        RichText::new(format!(
                            "{}{}signed by {}",
                            m.publisher,
                            if m.publisher.is_empty() { "" } else { " · " },
                            plugin.key
                        ))
                        .color(theme.faint)
                        .size(11.0),
                    );
                    ui.horizontal(|ui| {
                        if plugin.from_office {
                            if admin && connected && ui.button("Take off the office's list").clicked() {
                                take_off = Some(m.id.clone());
                            }
                        } else {
                            if admin && connected && ui.button("Share with the office").clicked() {
                                share = Some(plugin.path.clone());
                            }
                            if ui.button("Remove from this computer").clicked() {
                                remove_here = Some(plugin.path.clone());
                            }
                        }
                    });
                    ui.separator();
                }
                for (file, why) in &self.plugins.refused {
                    ui.label(RichText::new(format!("{file}: {why}")).color(theme.warn).size(11.0));
                }
                if ui.button("Add Plugin…").clicked() {
                    add = true;
                }
            });
        self.managing_plugins = open;
        if let Some(path) = remove_here {
            match std::fs::remove_file(&path) {
                Ok(()) => self.status = "Removed from this computer.".into(),
                Err(e) => self.status = format!("Could not remove it: {e}"),
            }
            self.reload_plugins();
        }
        if let Some(path) = share {
            self.ask(crate::server::Ask::SharePlugin(path));
        }
        if let Some(id) = take_off {
            self.ask(crate::server::Ask::RemovePlugin(id));
        }
        if add {
            self.add_plugin();
        }
    }

    /// What the office's server says it hands out: fetch what this seat
    /// has not got, drop what the office no longer lists.
    pub fn take_plugin_list(&mut self, list: Vec<hub::PluginInfo>) {
        // Nothing is fetched to a copy that doesn't run plugins itself. Its
        // menu is built from what the server says each one can do.
        if !crate::edition::runs_plugins_itself() {
            self.standing.plugins = list;
            self.reload_plugins();
            return;
        }
        let have = self.plugins.office_digests();
        for plugin in &list {
            let digest = plugin.digest.to_lowercase();
            if have.contains(&digest) {
                continue;
            }
            if self.plugins_fetched.insert(digest) {
                log::info!("fetching the office's plugin {} {}", plugin.name, plugin.version);
                self.ask(crate::server::Ask::FetchPlugin {
                    id: plugin.id.clone(),
                    name: plugin.name.clone(),
                });
            }
        }
        let listed: HashSet<&str> = list.iter().map(|p| p.id.as_str()).collect();
        let mut dropped = false;
        for plugin in self.plugins.plugins.iter().filter(|p| p.from_office) {
            if !listed.contains(plugin.manifest.id.as_str()) {
                log::info!("the office no longer hands out {}", plugin.manifest.id);
                let _ = std::fs::remove_file(&plugin.path);
                dropped = true;
            }
        }
        self.standing.plugins = list;
        if dropped {
            self.reload_plugins();
        }
    }

    /// Outlines what the plugin found on the sheet on screen.
    pub fn paint_plugin_findings(&self, painter: &egui::Painter) {
        let Some(shown) = self.plugin_shown.as_ref() else { return };
        let Ok(output) = &shown.result else { return };
        let Some(doc) = self.doc() else { return };
        if doc.id != shown.doc {
            return;
        }
        for (i, finding) in output.findings.iter().enumerate() {
            if shown.hide_notes && finding.level == Level::Note {
                continue;
            }
            let (Some(page), Some(area)) = (finding.page, finding.area) else { continue };
            if page != doc.page {
                continue;
            }
            let a = doc.view.to_screen([area[0] as f32, area[1] as f32]);
            let b = doc.view.to_screen([area[2] as f32, area[3] as f32]);
            let rect = egui::Rect::from_two_pos(a, b).expand(3.0);
            let colour = level_colour(finding.level);
            let chosen = shown.selected == Some(i);
            if chosen {
                painter.rect_filled(rect, 3.0, colour.gamma_multiply(0.18));
            }
            painter.rect_stroke(
                rect,
                3.0,
                Stroke::new(if chosen { 2.5_f32 } else { 1.5_f32 }, colour.gamma_multiply(if chosen { 1.0 } else { 0.8 })), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                egui::StrokeKind::Middle,
            );
        }
    }
}

fn count_words(output: &Output) -> String {
    let n = output.findings.len();
    match n {
        0 => "nothing".into(),
        1 => "1 thing".into(),
        n => format!("{n} things"),
    }
}
