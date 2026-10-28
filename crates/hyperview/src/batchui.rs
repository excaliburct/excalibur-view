//! The Batch menu's window.
//!
//! One window for every batch job, because they differ only in what they do to
//! each file. Pick the files, pick the job, watch the list fill in, and copy the
//! report out when it is done.
//!
//! The list is the point. A batch that finishes and says "done" gives somebody
//! nothing to check; this one names every file, says what was written, and says
//! plainly which ones could not be done and why.

use std::path::PathBuf;

use egui::{Color32, RichText};

use crate::app::App;
use crate::batch::{self, Outcome, Report};

/// What is being set up and run.
#[derive(Clone)]
pub struct Batching {
    pub which: Which,
    pub files: Vec<PathBuf>,
    /// Where results go. Empty means beside each file it came from.
    pub into: String,
    /// Combine: the one file everything goes into.
    pub to: String,
    pub every: String,
    pub quarter_turns: i32,
    pub dpi: u32,
    pub stamps: Vec<(crate::stamps::Spot, String)>,
    pub stamp_size: f32,
    pub only_scans: bool,
    pub seal_picture: Option<PathBuf>,
    pub seal_width: f32,
    pub revisions: Option<PathBuf>,
    pub keep_superseded: bool,
    /// Compare: the folder holding the issue each file is measured against.
    pub older: Option<PathBuf>,
    /// How dark a mark has to be before the comparison counts it as ink.
    pub darkness: f32,
    /// Crop: the box every sheet is trimmed to, in points.
    pub crop: [f32; 4],
    /// Apply Stamp: which corner it goes in.
    pub stamp_spot: crate::stamps::Spot,
    /// Summary: where the one report goes.
    pub report_to: String,
    /// Security: the passwords, and what a reader is asked to allow.
    pub open_password: String,
    pub open_password_again: String,
    pub owner_password: String,
    pub allowed: pdf::crypt::Allowed,
    pub running: bool,
    pub report: Report,
    pub of: usize,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Which {
    Combine,
    Split,
    Stamp,
    Rotate,
    Flatten,
    Shrink,
    Ocr,
    Seal,
    SlipSheet,
    Compare,
    Crop,
    ApplyStamp,
    Overlay,
    Print,
    Summary,
    Secure,
}

impl Which {
    pub const ALL: &'static [Which] = &[
        Which::Combine,
        Which::Split,
        Which::Stamp,
        Which::Rotate,
        Which::Flatten,
        Which::Shrink,
        Which::Ocr,
        Which::Seal,
        Which::SlipSheet,
        Which::Compare,
        Which::Overlay,
        Which::Crop,
        Which::ApplyStamp,
        Which::Print,
        Which::Summary,
        Which::Secure,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Which::Combine => "Combine into one",
            Which::Split => "Split each one",
            Which::Stamp => "Headers and footers",
            Which::Rotate => "Rotate",
            Which::Flatten => "Flatten markups",
            Which::Shrink => "Reduce file size",
            Which::Ocr => "OCR",
            Which::Seal => "Sign and seal",
            Which::SlipSheet => "Slip sheet",
            Which::Compare => "Compare documents",
            Which::Crop => "Crop and page setup",
            Which::ApplyStamp => "Apply stamp",
            Which::Overlay => "Overlay pages",
            Which::Print => "Print",
            Which::Summary => "Summary",
            Which::Secure => "Security",
        }
    }
}

impl Batching {
    pub fn new(which: Which) -> Batching {
        Batching {
            which,
            files: Vec::new(),
            into: String::new(),
            to: String::new(),
            every: "1".into(),
            quarter_turns: 1,
            dpi: 150,
            stamps: crate::stamps::Spot::ALL
                .iter()
                .map(|spot| (*spot, String::new()))
                .collect(),
            stamp_size: 12.0,
            only_scans: true,
            seal_picture: None,
            seal_width: 180.0,
            revisions: None,
            keep_superseded: false,
            older: None,
            darkness: 0.65,
            crop: [36.0, 36.0, 576.0, 756.0],
            stamp_spot: crate::stamps::Spot::BottomRight,
            report_to: String::new(),
            open_password: String::new(),
            open_password_again: String::new(),
            owner_password: String::new(),
            allowed: pdf::crypt::Allowed::default(),
            running: false,
            report: Report::default(),
            of: 0,
            error: None,
        }
    }

    /// The job this describes, or `None` when it is not ready.
    pub fn job(&self) -> Option<batch::Job> {
        let into = {
            let text = self.into.trim();
            (!text.is_empty()).then(|| PathBuf::from(text))
        };
        Some(match self.which {
            Which::Combine => {
                let to = self.to.trim();
                if to.is_empty() {
                    return None;
                }
                batch::Job::CombineAll {
                    to: crate::docops::free_name(&PathBuf::from(to)),
                }
            }
            Which::Split => {
                let every = self.every.trim().parse::<usize>().ok().filter(|e| *e > 0)?;
                batch::Job::Split {
                    every,
                    into: into.clone().or_else(|| {
                        self.files.first().and_then(|f| f.parent().map(|p| p.to_path_buf()))
                    })?,
                }
            }
            Which::Stamp => {
                let stamps: Vec<crate::stamps::Stamp> = self
                    .stamps
                    .iter()
                    .filter(|(_, text)| !text.trim().is_empty())
                    .map(|(spot, text)| crate::stamps::Stamp {
                        spot: *spot,
                        text: text.clone(),
                        size: self.stamp_size,
                        ..Default::default()
                    })
                    .collect();
                if stamps.is_empty() {
                    return None;
                }
                batch::Job::Stamp { stamps, into }
            }
            Which::Rotate => batch::Job::Rotate {
                quarter_turns: self.quarter_turns,
                into,
            },
            Which::Flatten => batch::Job::Flatten { into },
            Which::Shrink => batch::Job::Shrink {
                dpi: self.dpi,
                into,
            },
            Which::Ocr => batch::Job::Ocr {
                dpi: self.dpi,
                only_scans: self.only_scans,
                into,
            },
            Which::Seal => batch::Job::Seal {
                picture: self.seal_picture.clone()?,
                width: self.seal_width,
                into,
            },
            Which::SlipSheet => batch::Job::SlipSheet {
                revisions: self.revisions.clone()?,
                keep_superseded: self.keep_superseded,
                into,
            },
            Which::Compare => batch::Job::Compare {
                older: self.older.clone()?,
                darkness: self.darkness,
                into,
            },
            Which::Overlay => batch::Job::Overlay {
                older: self.older.clone()?,
                darkness: self.darkness,
                into,
            },
            Which::Crop => batch::Job::Crop {
                box_: self.crop,
                into,
            },
            Which::ApplyStamp => batch::Job::ApplyStamp {
                picture: self.seal_picture.clone()?,
                width: self.seal_width,
                spot: self.stamp_spot,
                into,
            },
            Which::Secure => {
                if self.open_password != self.open_password_again {
                    return None;
                }
                batch::Job::Secure {
                    open_password: self.open_password.clone(),
                    owner_password: self.owner_password.clone(),
                    allowed: self.allowed,
                    into,
                }
            }
            Which::Print => batch::Job::Print,
            Which::Summary => {
                let to = self.report_to.trim();
                if to.is_empty() {
                    return None;
                }
                batch::Job::Summary {
                    to: PathBuf::from(to),
                }
            }
        })
    }
}

/// How long after the last of Explorer's starts the Combine window opens.
/// Explorer starts one for every file selected, all at once, so this is long
/// enough for a hundred of them to land and short enough to feel like the
/// click did it.
const GATHERING: std::time::Duration = std::time::Duration::from_millis(600);

/// The Combine window with Explorer's selection added: each file once, in the
/// order a person reads the names, going into Combined.pdf beside the first
/// unless somewhere was already chosen. Files already in the list keep their
/// place, in case somebody has put them in order by hand.
pub fn gathered(mut batching: Batching, mut files: Vec<PathBuf>) -> Batching {
    let name = |p: &PathBuf| p.file_name().unwrap_or_default().to_string_lossy().to_string();
    files.sort_by(|a, b| batch::naturally(&name(a), &name(b)));
    for file in files {
        if !batching.files.contains(&file) {
            batching.files.push(file);
        }
    }
    name_the_combined(&mut batching); // WHAT: Combined.pdf beside the first, unless somewhere was chosen. WHY: the same rule however the files arrived.
    batching
}

/// Adds files to a batch's list, each once, in the order given, and names
/// the combined file when nobody has yet.
pub fn add_to_list(batching: &mut Batching, files: impl IntoIterator<Item = PathBuf>) {
    for file in files {
        if !batching.files.contains(&file) {
            batching.files.push(file); // WHAT: on the end. WHY: each file once, in the order chosen.
        }
    }
    name_the_combined(batching); // WHAT: a file for the result. WHY: without one, Combine stayed greyed out until somebody typed a path.
}

/// The file a combine goes into, when nobody has said: Combined.pdf beside
/// the first file. Before this, only files handed over by Windows' Explorer
/// got a name; on Linux, where nothing hands files over that way, the box
/// stayed empty and the Combine button stayed grey.
pub fn name_the_combined(batching: &mut Batching) {
    if batching.to.trim().is_empty() {
        if let Some(folder) = batching.files.first().and_then(|f| f.parent()) {
            batching.to = folder.join("Combined.pdf").display().to_string(); // WHAT: beside the first file. WHY: where somebody would look for it.
        }
    }
}

impl App {
    pub fn begin_batch(&mut self, which: Which) {
        self.batching = Some(Batching::new(which));
    }

    /// Opens the Combine window once Explorer has finished handing files
    /// over. A batch already running is left to finish first.
    pub fn combine_when_gathered(&mut self, ctx: &egui::Context) {
        let Some(at) = self.to_combine_at else {
            return;
        };
        let waited = at.elapsed();
        if waited < GATHERING {
            ctx.request_repaint_after(GATHERING - waited);
            return;
        }
        if self.batching.as_ref().is_some_and(|b| b.running) {
            ctx.request_repaint_after(GATHERING);
            return;
        }
        let files = std::mem::take(&mut self.to_combine);
        self.to_combine_at = None;
        let batching = match self.batching.take() {
            Some(open) if open.which == Which::Combine => open,
            _ => Batching::new(Which::Combine),
        };
        self.batching = Some(gathered(batching, files));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    pub fn batch_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut batching) = self.batching.take() else {
            return;
        };
        let theme = self.chrome.theme;
        let mut keep = true;
        let mut go = false;
        let mut add_files = false;
        let mut add_folder = false;
        let mut add_open = false; // WHAT: Add the open drawings pressed. WHY: acted on after the window is drawn.
        let mut add_from_list = false; // WHAT: Pick from a list pressed. WHY: the same.
        let mut copy_report = false;
        let mut pick_seal = false;
        let mut pick_revisions = false;
        let mut pick_older = false;

        // A width it can't grow past: the file list and the path box fill
        // whatever width they're given, and without a limit the window kept
        // taking more of it until it reached both edges of the screen.
        egui::Window::new("Batch")
            .collapsible(false)
            .resizable(true)
            .default_width(620.0)
            .min_width(440.0)
            .max_width(760.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                // ---- what ---------------------------------------------------
                ui.label(RichText::new("Do this").color(theme.faint).size(11.0));
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    for which in Which::ALL {
                        ui.selectable_value(&mut batching.which, *which, which.name());
                    }
                });
                ui.add_space(12.0);

                // ---- to what ------------------------------------------------
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("To {} file(s)", batching.files.len()))
                            .color(theme.faint)
                            .size(11.0),
                    );
                    if ui.small_button("Add files…").clicked() {
                        add_files = true;
                    }
                    if ui.small_button("Add a folder…").clicked() {
                        add_folder = true;
                    }
                    if !self.docs.is_empty() && ui.small_button("Add the open drawings").on_hover_text("Every drawing open in tabs now").clicked() {
                        add_open = true; // WHAT: the drawings already open. WHY: a way to fill the list that needs no file window at all.
                    }
                    if crate::files::offer_own_list()
                        && ui
                            .small_button("Pick from a list…")
                            .on_hover_text("Choose the files in Excalibur View's own list, for when the system's file window does not open or will not take several files. Dragging PDFs onto this window adds them too.")
                            .clicked()
                    {
                        add_from_list = true; // WHAT: the program's own list. WHY: Linux's file window can be missing or refuse several files.
                    }
                    if !batching.files.is_empty() && ui.small_button("Clear").clicked() {
                        batching.files.clear();
                    }
                });
                if !batching.files.is_empty() {
                    ui.add_space(4.0);
                    // Combining goes in this order, so the order can be
                    // changed here. Any file can be taken off the list.
                    let ordered = batching.which == Which::Combine;
                    let last = batching.files.len() - 1;
                    let mut moved: Option<(usize, usize)> = None;
                    let mut dropped: Option<usize> = None;
                    egui::ScrollArea::vertical()
                        .id_salt("batch-files")
                        .max_height(if ordered { 220.0 } else { 110.0 })
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (i, file) in batching.files.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    if ordered {
                                        if ui
                                            .add_enabled(i > 0, egui::Button::new("⏶").small())
                                            .on_hover_text("Earlier in the combined file")
                                            .clicked()
                                        {
                                            moved = Some((i, i - 1));
                                        }
                                        if ui
                                            .add_enabled(i < last, egui::Button::new("⏷").small())
                                            .on_hover_text("Later in the combined file")
                                            .clicked()
                                        {
                                            moved = Some((i, i + 1));
                                        }
                                    }
                                    if ui
                                        .add_enabled(!batching.running, egui::Button::new("×").small())
                                        .on_hover_text("Take it off the list")
                                        .clicked()
                                    {
                                        dropped = Some(i);
                                    }
                                    ui.label(
                                        RichText::new(
                                            file.file_name().unwrap_or_default().to_string_lossy(),
                                        )
                                        .size(11.0),
                                    )
                                    .on_hover_text(file.display().to_string());
                                });
                            }
                        });
                    if !batching.running {
                        if let Some((from, to)) = moved {
                            batching.files.swap(from, to);
                        }
                        if let Some(i) = dropped {
                            batching.files.remove(i);
                        }
                    }
                }
                ui.add_space(12.0);

                // ---- the job's own settings ---------------------------------
                match batching.which {
                    Which::Combine => {
                        ui.label(
                            RichText::new("All of them into").color(theme.faint).size(11.0),
                        );
                        ui.horizontal(|ui| {
                            let width = ui::chrome::room_beside(ui, "Browse…");
                            ui.add(
                                egui::TextEdit::singleline(&mut batching.to)
                                    .hint_text("Combined.pdf")
                                    .desired_width(width),
                            );
                            if ui.button("Browse…").clicked() {
                                self.filing.one(
                                    crate::files::Choose::save("Combined.pdf")
                                        .filter("Drawing sets (PDF)", &["pdf"]),
                                    |app, path| {
                                        if let Some(b) = app.batching.as_mut() {
                                            b.to = path.display().to_string();
                                        }
                                    },
                                );
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "In the order listed above. Files sort the way you read \
                                 them — 2 before 10.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                    }
                    Which::Split => {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Sheets per file").color(theme.faint).size(11.0),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut batching.every)
                                    .desired_width(60.0),
                            );
                        });
                    }
                    Which::Rotate => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Turn").color(theme.faint).size(11.0));
                            ui.selectable_value(&mut batching.quarter_turns, 1, "90° right");
                            ui.selectable_value(&mut batching.quarter_turns, 2, "180°");
                            ui.selectable_value(&mut batching.quarter_turns, 3, "90° left");
                        });
                    }
                    Which::Shrink => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Render at").color(theme.faint).size(11.0));
                            for dpi in [100u32, 150, 200, 300] {
                                ui.selectable_value(
                                    &mut batching.dpi,
                                    dpi,
                                    format!("{dpi} dpi"),
                                );
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Every sheet becomes a picture: no text to search, and \
                                 markups become part of the page. The originals are \
                                 untouched.",
                            )
                            .color(theme.warn)
                            .size(10.0),
                        );
                    }
                    Which::Flatten => {
                        ui.label(
                            RichText::new(
                                "Markups become part of the page — no list, no totals, no \
                                 editing. Each original keeps its own.",
                            )
                            .color(theme.warn)
                            .size(10.0),
                        );
                    }
                    Which::Ocr => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Read at").color(theme.faint).size(11.0));
                            for dpi in [150u32, 200, 300, 400] {
                                ui.selectable_value(
                                    &mut batching.dpi,
                                    dpi,
                                    format!("{dpi} dpi"),
                                );
                            }
                        });
                        ui.add_space(6.0);
                        ui.checkbox(
                            &mut batching.only_scans,
                            "Skip sheets that already have text",
                        );
                        if crate::ocr::engine_here().is_none() {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(crate::ocr::NO_ENGINE)
                                    .color(theme.warn)
                                    .size(10.0),
                            );
                        }
                    }
                    Which::Seal => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("The seal").color(theme.faint).size(11.0));
                            match &batching.seal_picture {
                                Some(path) => {
                                    ui.label(
                                        RichText::new(
                                            path.file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy(),
                                        )
                                        .size(11.0),
                                    );
                                }
                                None => {
                                    ui.label(
                                        RichText::new("none chosen")
                                            .color(theme.faint)
                                            .size(11.0),
                                    );
                                }
                            }
                            if ui.small_button("Choose…").clicked() {
                                pick_seal = true;
                            }
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Width").color(theme.faint).size(11.0));
                            ui.add(
                                egui::Slider::new(&mut batching.seal_width, 60.0..=400.0)
                                    .suffix(" pt"),
                            );
                        });
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "A picture on each sheet, not a digital signature.",
                            )
                            .color(theme.warn)
                            .size(10.0),
                        );
                    }
                    Which::SlipSheet => {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Revisions folder")
                                    .color(theme.faint)
                                    .size(11.0),
                            );
                            match &batching.revisions {
                                Some(path) => {
                                    ui.label(RichText::new(path.display().to_string()).size(11.0));
                                }
                                None => {
                                    ui.label(
                                        RichText::new("none chosen")
                                            .color(theme.faint)
                                            .size(11.0),
                                    );
                                }
                            }
                            if ui.small_button("Choose…").clicked() {
                                pick_revisions = true;
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Each set is slipped against the revision with the same \
                                 file name. A set with no revision of its name is listed \
                                 as not done, rather than paired with something else.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                        ui.add_space(6.0);
                        ui.checkbox(
                            &mut batching.keep_superseded,
                            "Keep the sheets they replace, at the back",
                        );
                    }
                    Which::Compare => {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Older issue folder")
                                    .color(theme.faint)
                                    .size(11.0),
                            );
                            match &batching.older {
                                Some(path) => {
                                    ui.label(RichText::new(path.display().to_string()).size(11.0));
                                }
                                None => {
                                    ui.label(
                                        RichText::new("none chosen")
                                            .color(theme.faint)
                                            .size(11.0),
                                    );
                                }
                            }
                            if ui.small_button("Choose…").clicked() {
                                pick_older = true;
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Each set above is compared against the file with the \
                                 same name in that folder, sheet for sheet, and a copy \
                                 of the newer one is written with a cloud round every \
                                 difference. A set with no older issue of its name is \
                                 listed as not done, rather than compared against \
                                 something else. Nothing is measured — the clouds say \
                                 where to look.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Counts as ink").color(theme.faint).size(11.0));
                            ui.add(
                                egui::Slider::new(&mut batching.darkness, 0.35..=0.9)
                                    .show_value(false),
                            );
                            ui.label(
                                RichText::new(if batching.darkness < 0.5 {
                                    "only solid lines"
                                } else if batching.darkness < 0.75 {
                                    "plotted linework"
                                } else {
                                    "faint lines too"
                                })
                                .color(theme.faint)
                                .size(10.0),
                            );
                        });
                    }
                    Which::Overlay => {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Older issue folder")
                                    .color(theme.faint)
                                    .size(11.0),
                            );
                            match &batching.older {
                                Some(path) => {
                                    ui.label(RichText::new(path.display().to_string()).size(11.0));
                                }
                                None => {
                                    ui.label(
                                        RichText::new("none chosen")
                                            .color(theme.faint)
                                            .size(11.0),
                                    );
                                }
                            }
                            if ui.small_button("Choose…").clicked() {
                                pick_older = true;
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Each set above is laid over the file with the same name \
                                 in that folder. What moved shows as colour; what did not \
                                 shows dark. Nothing is measured — it is a picture to \
                                 look at, which is what most people find easier than a \
                                 list of regions.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                    }
                    Which::Crop => {
                        ui.label(
                            RichText::new("Trim every sheet to this box, in points from \
                                           the bottom left")
                                .color(theme.faint)
                                .size(11.0),
                        );
                        ui.add_space(4.0);
                        egui::Grid::new("batch-crop")
                            .num_columns(4)
                            .spacing([8.0, 6.0])
                            .show(ui, |ui| {
                                for (label, at) in
                                    [("Left", 0), ("Bottom", 1), ("Right", 2), ("Top", 3)]
                                {
                                    ui.label(
                                        RichText::new(label).color(theme.faint).size(11.0),
                                    );
                                    ui.add(
                                        egui::DragValue::new(&mut batching.crop[at])
                                            .speed(1.0)
                                            .range(0.0..=5000.0),
                                    );
                                }
                            });
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Nothing is thrown away: a crop box is a window onto the \
                                 page, and widening it later brings back what was outside \
                                 it. Sheets smaller than the box are left as they are \
                                 rather than being grown.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                    }
                    Which::ApplyStamp => {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("The stamp").color(theme.faint).size(11.0),
                            );
                            match &batching.seal_picture {
                                Some(path) => {
                                    ui.label(
                                        RichText::new(
                                            path.file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy()
                                                .to_string(),
                                        )
                                        .size(11.0),
                                    );
                                }
                                None => {
                                    ui.label(
                                        RichText::new("none chosen")
                                            .color(theme.faint)
                                            .size(11.0),
                                    );
                                }
                            }
                            if ui.small_button("Choose…").clicked() {
                                pick_seal = true;
                            }
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("In the").color(theme.faint).size(11.0));
                            egui::ComboBox::from_id_salt("stamp-spot")
                                .selected_text(batching.stamp_spot.name())
                                .show_ui(ui, |ui| {
                                    for spot in crate::stamps::Spot::ALL {
                                        ui.selectable_value(
                                            &mut batching.stamp_spot,
                                            *spot,
                                            spot.name(),
                                        );
                                    }
                                });
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Width").color(theme.faint).size(11.0));
                            ui.add(
                                egui::Slider::new(&mut batching.seal_width, 60.0..=400.0)
                                    .suffix(" pt"),
                            );
                        });
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "The corner is worked out on each sheet, so a set of \
                                 letter pages and a set of ARCH E1 sheets both get it in \
                                 the same place on the paper.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                    }
                    Which::Secure => {
                        ui.label(
                            RichText::new("Password to open them")
                                .color(theme.faint)
                                .size(11.0),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut batching.open_password)
                                .password(true)
                                .hint_text("leave empty and anybody may open them")
                                .desired_width(220.0),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(
                                    &mut batching.open_password_again,
                                )
                                .password(true)
                                .hint_text("and again")
                                .desired_width(220.0),
                            );
                            if batching.open_password != batching.open_password_again {
                                ui.label(
                                    RichText::new("These two do not match.")
                                        .color(theme.warn)
                                        .size(11.0),
                                );
                            }
                        });
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("Owner's password")
                                .color(theme.faint)
                                .size(11.0),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut batching.owner_password)
                                .password(true)
                                .hint_text("leave empty to use the same one")
                                .desired_width(220.0),
                        );
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new("Whoever opens them may")
                                .color(theme.faint)
                                .size(11.0),
                        );
                        egui::Grid::new("batch-permissions")
                            .num_columns(2)
                            .spacing([24.0, 4.0])
                            .show(ui, |ui| {
                                ui.checkbox(&mut batching.allowed.print, "Print them");
                                ui.checkbox(
                                    &mut batching.allowed.print_well,
                                    "Print at full size",
                                );
                                ui.end_row();
                                ui.checkbox(&mut batching.allowed.copy, "Copy text out");
                                ui.checkbox(&mut batching.allowed.annotate, "Mark them up");
                                ui.end_row();
                                ui.checkbox(&mut batching.allowed.change, "Change them");
                                ui.checkbox(
                                    &mut batching.allowed.assemble,
                                    "Take sheets out or put sheets in",
                                );
                                ui.end_row();
                            });
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(
                                "The same password on all of them. Permissions are a \
                                 request a reader honours, not a lock — the password is \
                                 the lock, and Excalibur View keeps no copy of it.",
                            )
                            .color(theme.warn)
                            .size(10.0),
                        );
                        ui.add_space(10.0);
                    }
                    Which::Print => {
                        ui.label(
                            RichText::new(
                                "Every file above is sent to whatever this machine \
                                 prints with, one after another. Nothing is written and \
                                 nothing is changed. The report says which ones went.",
                            )
                            .color(theme.faint)
                            .size(11.0),
                        );
                    }
                    Which::Summary => {
                        ui.label(
                            RichText::new("One report covering all of them, written to")
                                .color(theme.faint)
                                .size(11.0),
                        );
                        ui.horizontal(|ui| {
                            let width = ui::chrome::room_beside(ui, "Browse…");
                            ui.add(
                                egui::TextEdit::singleline(&mut batching.report_to)
                                    .hint_text("Takeoff summary.pdf")
                                    .desired_width(width),
                            );
                            if ui.button("Browse…").clicked() {
                                self.filing.one(
                                    crate::files::Choose::save("Takeoff summary.pdf")
                                        .filter("Reports (PDF)", &["pdf"]),
                                    |app, path| {
                                        if let Some(b) = app.batching.as_mut() {
                                            b.report_to = path.display().to_string();
                                        }
                                    },
                                );
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "Every measurement in every set, against the scale \
                                 written into the sheet it is on. A sheet with no scale \
                                 has its measurements reported and left out of the \
                                 totals — never counted as zero.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                    }
                    Which::Stamp => {
                        for (spot, text) in batching.stamps.iter_mut() {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("{:<14}", spot.name()))
                                        .color(theme.faint)
                                        .size(11.0),
                                );
                                ui.add(
                                    egui::TextEdit::singleline(text)
                                        .desired_width(f32::INFINITY),
                                );
                            });
                        }
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("Fields: <n> <total> <sheet> <file> <date>")
                                .color(theme.faint)
                                .size(10.0),
                        );
                    }
                }

                // ---- where ---------------------------------------------------
                if !matches!(
                    batching.which,
                    Which::Combine | Which::Print | Which::Summary
                ) {
                    ui.add_space(12.0);
                    ui.label(RichText::new("Put the results").color(theme.faint).size(11.0));
                    ui.horizontal(|ui| {
                        let width = ui::chrome::room_beside(ui, "Browse…");
                        ui.add(
                            egui::TextEdit::singleline(&mut batching.into)
                                .hint_text("beside each file it came from")
                                .desired_width(width),
                        );
                        if ui.button("Browse…").clicked() {
                            self.filing.one(crate::files::Choose::folder(), |app, path| {
                                if let Some(b) = app.batching.as_mut() {
                                    b.into = path.display().to_string();
                                }
                            });
                        }
                    });
                }

                // ---- what happened -------------------------------------------
                if !batching.report.lines.is_empty() {
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(batching.report.says()).size(11.0));
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.small_button("Copy the report").clicked() {
                                    copy_report = true;
                                }
                            },
                        );
                    });
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .id_salt("batch-report")
                        .max_height(200.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for line in &batching.report.lines {
                                let name = line
                                    .source
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string();
                                match &line.outcome {
                                    Outcome::Done { said, .. } => {
                                        ui.label(RichText::new(name).size(11.0));
                                        ui.label(
                                            RichText::new(format!("    {said}"))
                                                .color(theme.faint)
                                                .size(10.0),
                                        );
                                    }
                                    Outcome::Failed(why) => {
                                        ui.label(
                                            RichText::new(name)
                                                .color(Color32::from_rgb(235, 120, 120))
                                                .size(11.0),
                                        );
                                        ui.label(
                                            RichText::new(format!("    not done — {why}"))
                                                .color(Color32::from_rgb(235, 120, 120))
                                                .size(10.0),
                                        );
                                    }
                                }
                            }
                        });
                }

                if let Some(error) = &batching.error {
                    ui.add_space(8.0);
                    ui.colored_label(Color32::from_rgb(235, 120, 120), error);
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let ready =
                        !batching.running && !batching.files.is_empty() && batching.job().is_some();
                    if ui.add_enabled(ready, egui::Button::new("Run")).clicked() {
                        go = true;
                    }
                    if ui.button("Close").clicked() {
                        keep = false;
                    }
                    if batching.running {
                        ui.add_space(8.0);
                        ui.spinner();
                        ui.label(
                            RichText::new(format!(
                                "{} of {}…",
                                batching.report.lines.len(),
                                batching.of.max(batching.files.len())
                            ))
                            .color(theme.faint)
                            .size(11.0),
                        );
                    }
                });
            });

        if add_files || add_from_list {
            let choose = crate::files::Choose::open_many().filter("Drawing sets (PDF)", &["pdf"]); // WHAT: several PDFs. WHY: a batch is a list of sets.
            let choose = if add_from_list { choose.here() } else { choose }; // WHAT: the program's own list, when asked for. WHY: see Pick from a list.
            self.filing.many(choose, |app, picked| {
                if let Some(b) = app.batching.as_mut() {
                    add_to_list(b, picked); // WHAT: each once, and the combined file named. WHY: Combine can start straight away.
                }
            });
        }
        if add_open {
            let open: Vec<PathBuf> = self.docs.iter().map(|d| d.path.clone()).collect(); // WHAT: every open drawing's file. WHY: in tab order.
            add_to_list(&mut batching, open); // WHAT: added. WHY: Add the open drawings.
        }
        if add_folder {
            self.filing.one(crate::files::Choose::folder(), |app, folder| {
                let Some(batching) = app.batching.as_mut() else { return };
                let found = batch::pdfs_in(&folder);
                if found.is_empty() {
                    batching.error = Some(format!(
                        "There are no PDFs in {}.",
                        folder.file_name().unwrap_or_default().to_string_lossy()
                    ));
                } else {
                    batching.error = None;
                    add_to_list(batching, found); // WHAT: each once, and the combined file named. WHY: as for files added one by one.
                }
            });
        }
        if pick_seal {
            self.filing.one(
                crate::files::Choose::open()
                    .filter("Pictures", &["png", "jpg", "jpeg", "PNG", "JPG", "JPEG"])
                    .title("The engineer's seal"),
                |app, path| {
                    if let Some(b) = app.batching.as_mut() {
                        b.seal_picture = Some(path);
                    }
                },
            );
        }
        if pick_revisions {
            self.filing.one(
                crate::files::Choose::folder().title("The folder of revisions"),
                |app, folder| {
                    if let Some(b) = app.batching.as_mut() {
                        b.revisions = Some(folder);
                    }
                },
            );
        }
        if pick_older {
            self.filing.one(
                crate::files::Choose::folder().title("The folder holding the older issue"),
                |app, folder| {
                    if let Some(b) = app.batching.as_mut() {
                        b.older = Some(folder);
                    }
                },
            );
        }
        if copy_report {
            ctx.copy_text(batching.report.as_text());
            self.status = "The report is on the clipboard.".into();
        }
        if go {
            if let Some(work) = batching.job() {
                batching.running = true;
                batching.error = None;
                batching.report = Report::default();
                batching.of = batching.files.len();
                self.batch_task += 1;
                let job = self.batch_task;
                self.svc.send(crate::render::ToWorker::Batch {
                    job,
                    work: Box::new(work),
                    files: batching.files.clone(),
                });
            }
        }
        if keep {
            self.batching = Some(batching);
        }
    }

    pub fn batch_line(&mut self, job: u64, line: batch::Line, of: usize) {
        if job != self.batch_task {
            return;
        }
        if let Some(batching) = self.batching.as_mut() {
            batching.of = of;
            batching.report.lines.push(line);
        }
    }

    pub fn batch_finished(&mut self, job: u64) {
        if job != self.batch_task {
            return;
        }
        if let Some(batching) = self.batching.as_mut() {
            batching.running = false;
            self.status = batching.report.says();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_with_no_files_is_not_ready() {
        let batching = Batching::new(Which::Flatten);
        assert!(batching.files.is_empty());
        // Flatten needs nothing else, so the job itself is describable — it is
        // the empty file list that stops it, which the button checks.
        assert!(batching.job().is_some());
    }

    #[test]
    fn combining_needs_somewhere_to_put_the_result() {
        let mut batching = Batching::new(Which::Combine);
        assert!(batching.job().is_none());
        batching.to = "/tmp/All.pdf".into();
        assert!(batching.job().is_some());
    }

    #[test]
    fn explorer_selection_is_combined_in_the_order_the_names_read() {
        let files = ["S-110.pdf", "S-102.pdf", "S-9.pdf", "S-101.pdf"]
            .iter()
            .map(|n| PathBuf::from("jobs").join("6742").join(n))
            .collect();
        let batching = gathered(Batching::new(Which::Combine), files);
        let names: Vec<String> = batching
            .files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["S-9.pdf", "S-101.pdf", "S-102.pdf", "S-110.pdf"]);
        assert_eq!(
            PathBuf::from(&batching.to),
            PathBuf::from("jobs").join("6742").join("Combined.pdf"),
            "beside the drawings, never over one of them"
        );
        assert!(batching.job().is_some(), "ready to run");
    }

    #[test]
    fn a_second_selection_joins_the_first_without_undoing_its_order() {
        let mut batching = Batching::new(Which::Combine);
        batching.files = vec![PathBuf::from("B.pdf"), PathBuf::from("A.pdf")];
        batching.to = "All.pdf".into();
        let batching = gathered(batching, vec![PathBuf::from("C.pdf"), PathBuf::from("A.pdf")]);
        assert_eq!(
            batching.files,
            vec![PathBuf::from("B.pdf"), PathBuf::from("A.pdf"), PathBuf::from("C.pdf")]
        );
        assert_eq!(batching.to, "All.pdf", "a place already chosen stays chosen");
    }

    #[test]
    fn stamping_nothing_is_not_a_job() {
        let mut batching = Batching::new(Which::Stamp);
        assert!(batching.job().is_none(), "every corner is empty");
        batching.stamps[0].1 = "EXAMPLE STEEL 24-118".into();
        assert!(batching.job().is_some());
    }

    #[test]
    fn splitting_by_nothing_is_not_a_job() {
        let mut batching = Batching::new(Which::Split);
        batching.files = vec![PathBuf::from("/jobs/Set.pdf")];
        batching.every = "0".into();
        assert!(batching.job().is_none());
        batching.every = "5".into();
        assert!(batching.job().is_some());
    }

    #[test]
    fn leaving_the_folder_empty_means_beside_each_file() {
        let batching = Batching::new(Which::Rotate);
        match batching.job().expect("a rotate") {
            batch::Job::Rotate { into, .. } => assert!(into.is_none()),
            _ => panic!("a rotate"),
        }
    }
}
