//! The Document menu's windows.
//!
//! One window shape for all of them, because they are all the same shape of
//! question: which sheets, where does the result go, and — said before anything
//! runs — what exactly is about to happen.
//!
//! That last part is not decoration. Somebody choosing "Slip Sheet" on a set
//! they are three hours into marking up wants to know, before they press
//! anything, that their markups are safe and their original file is untouched.
//! So every one of these windows says what it will do in a sentence, and the
//! one operation that loses something says so twice.

use std::path::PathBuf;

use egui::{Color32, RichText};

use crate::app::App;
use crate::docops::{self, Operation};

/// Which of the Document menu's jobs is being set up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Job {
    Combine,
    Split,
    Extract,
    Insert,
    Delete,
    Rotate,
    Crop,
    SlipSheet,
    Flatten,
    Stamp,
    Shrink,
    Ocr,
    Seal,
    Repair,
    Unflatten,
    PageLabels,
    Redact,
    Export,
    Secure,
    NewBlank,
    FromPictures,
}

impl Job {
    pub fn title(self) -> &'static str {
        match self {
            Job::Combine => "Combine",
            Job::Split => "Split",
            Job::Extract => "Extract Pages",
            Job::Insert => "Insert Pages",
            Job::Delete => "Delete Pages",
            Job::Rotate => "Rotate Pages",
            Job::Crop => "Crop Pages",
            Job::SlipSheet => "Slip Sheet",
            Job::Flatten => "Flatten Markups",
            Job::Stamp => "Headers and Footers",
            Job::Shrink => "Reduce File Size",
            Job::Ocr => "OCR",
            Job::Seal => "Sign and Seal",
            Job::Repair => "Repair PDF",
            Job::Unflatten => "Unflatten Markups",
            Job::PageLabels => "Create Page Labels",
            Job::Redact => "Apply Redactions",
            Job::Export => "Export",
            Job::Secure => "Security",
            Job::NewBlank => "New PDF",
            Job::FromPictures => "Create PDF from Pictures",
        }
    }

    fn explanation(self) -> &'static str {
        match self {
            Job::Combine => "Several drawing sets into one, in the order you list them.",
            Job::Split => "One set into several files.",
            Job::Extract => "Chosen sheets into a file of their own.",
            Job::Insert => "Another file's sheets into this one.",
            Job::Delete => "A copy without the sheets you choose.",
            Job::Rotate => "Turn sheets that were scanned sideways.",
            Job::Crop => "Trim what is shown. Nothing is thrown away.",
            Job::SlipSheet => "Swap in revised sheets, matched by sheet number.",
            Job::Flatten => "Turn markups into part of the page.",
            Job::Stamp => "Put a job number, a date or a sheet count in the corners.",
            Job::Shrink => "Make a set small enough to email.",
            Job::Ocr => "Read the words off a scanned set so it can be searched.",
            Job::Seal => "Put the engineer's seal on the sheets.",
            Job::Repair => "Rebuild a file whose table of contents no longer matches it.",
            Job::Unflatten => "Lift markups Excalibur View flattened back out into markups.",
            Job::PageLabels => "Write each sheet's own number into the file, for other readers.",
            Job::Redact => "Take out what is under every redaction mark, for good.",
            Job::Export => "Write the sheets out as picture files.",
            Job::Secure => "Lock the set with a password, and say who may do what with it.",
            Job::NewBlank => "A new drawing with nothing on it, to draw on.",
            Job::FromPictures => "A folder of scans or photographs into one drawing set.",
        }
    }

    /// Whether this one acts on chosen sheets rather than the whole drawing.
    fn picks_sheets(self) -> bool {
        matches!(
            self,
            Job::Extract
                | Job::Delete
                | Job::Rotate
                | Job::Crop
                | Job::Flatten
                | Job::Stamp
                | Job::Ocr
                | Job::Seal
                | Job::Export
        )
    }
}

/// What is being filled in.
#[derive(Clone, Debug)]
pub struct Setting {
    pub job: Job,
    /// "3, 7, 12-18", or empty for all of them.
    pub range: String,
    /// Other files, for the jobs that take them.
    pub others: Vec<PathBuf>,
    /// Where the result goes.
    pub to: String,
    /// Split: sheets per file.
    pub every: String,
    /// Rotate: quarter turns clockwise.
    pub quarter_turns: i32,
    /// Extract: also produce the remainder.
    pub and_remove: bool,
    /// Slip sheet: keep what was replaced.
    pub keep_superseded: bool,
    /// Headers and footers: what goes in each corner.
    pub stamps: Vec<(crate::stamps::Spot, String)>,
    pub stamp_size: f32,
    /// Reduce File Size and OCR: dots per inch.
    pub dpi: u32,
    /// OCR: leave alone sheets that already have text.
    pub only_scans: bool,
    /// Sign and Seal: the picture, and how wide to draw it.
    pub seal_picture: Option<PathBuf>,
    pub seal_width: f32,
    /// Page Labels: the sheet numbers already read off the drawings.
    pub labels: Vec<(u32, String)>,
    /// Apply Redactions: turn the affected sheets into pictures rather than
    /// taking the objects out.
    pub redact_as_pictures: bool,
    /// Export: `png` or `jpg`.
    pub picture_format: String,
    /// Security: the password to open it, the owner's password, and what a
    /// reader is asked to allow.
    pub open_password: String,
    pub open_password_again: String,
    pub owner_password: String,
    pub allowed: pdf::crypt::Allowed,
    /// New PDF: how many sheets, and what paper.
    pub new_sheets: String,
    pub paper: usize,
    pub error: Option<String>,
    pub running: bool,
}

impl Setting {
    pub fn new(job: Job, source: &std::path::Path) -> Setting {
        Setting {
            job,
            range: String::new(),
            others: Vec::new(),
            to: docops::beside(source, suffix(job)).display().to_string(),
            every: "1".into(),
            quarter_turns: 1,
            and_remove: false,
            keep_superseded: false,
            stamps: crate::stamps::Spot::ALL
                .iter()
                .map(|spot| (*spot, String::new()))
                .collect(),
            stamp_size: 12.0,
            dpi: 150,
            only_scans: true,
            seal_picture: None,
            seal_width: 180.0,
            labels: Vec::new(),
            redact_as_pictures: false,
            picture_format: "png".into(),
            open_password: String::new(),
            open_password_again: String::new(),
            owner_password: String::new(),
            allowed: pdf::crypt::Allowed::default(),
            new_sheets: "1".into(),
            paper: 0,
            error: None,
            running: false,
        }
    }
}

/// The papers a drawing office actually uses, in points.
pub const PAPERS: &[(&str, [f32; 2])] = &[
    ("Letter — 8½ × 11 in", [612.0, 792.0]),
    ("Letter landscape — 11 × 8½ in", [792.0, 612.0]),
    ("Legal — 8½ × 14 in", [612.0, 1008.0]),
    ("Tabloid — 11 × 17 in", [792.0, 1224.0]),
    ("ANSI C — 22 × 17 in", [1584.0, 1224.0]),
    ("ANSI D — 34 × 22 in", [2448.0, 1584.0]),
    ("ANSI E — 44 × 34 in", [3168.0, 2448.0]),
    ("ARCH C — 24 × 18 in", [1728.0, 1296.0]),
    ("ARCH D — 36 × 24 in", [2592.0, 1728.0]),
    ("ARCH E1 — 42 × 30 in", [3024.0, 2160.0]),
    ("ARCH E — 48 × 36 in", [3456.0, 2592.0]),
    ("A4 — 210 × 297 mm", [595.0, 842.0]),
    ("A3 — 297 × 420 mm", [842.0, 1191.0]),
    ("A1 — 594 × 841 mm", [1684.0, 2384.0]),
    ("A0 — 841 × 1189 mm", [2384.0, 3370.0]),
];

fn suffix(job: Job) -> &'static str {
    match job {
        Job::Combine => "combined",
        Job::Split => "split",
        Job::Extract => "extract",
        Job::Insert => "with insert",
        Job::Delete => "trimmed",
        Job::Rotate => "turned",
        Job::Crop => "cropped",
        Job::SlipSheet => "slipped",
        Job::Flatten => "flattened",
        Job::Stamp => "stamped",
        Job::Shrink => "small",
        Job::Ocr => "searchable",
        Job::Seal => "sealed",
        Job::Repair => "repaired",
        Job::Unflatten => "unflattened",
        Job::PageLabels => "labelled",
        Job::Redact => "redacted",
        Job::Export => "pictures",
        Job::Secure => "locked",
        Job::NewBlank => "new",
        Job::FromPictures => "from pictures",
    }
}

impl App {
    pub fn begin_document_job(&mut self, job: Job) {
        // Making a new drawing is the one thing that does not need one open.
        if matches!(job, Job::NewBlank | Job::FromPictures) {
            let beside = self
                .doc()
                .map(|d| d.path.clone())
                .or_else(|| self.prefs.recent.first().cloned())
                .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let mut setting = Setting::new(job, &beside.join("drawing.pdf"));
            setting.to = docops::free_name(&beside.join(if job == Job::NewBlank {
                "New drawing.pdf"
            } else {
                "From pictures.pdf"
            }))
            .display()
            .to_string();
            setting.dpi = 300;
            self.doc_job = Some(setting);
            return;
        }
        let Some(doc) = self.doc() else {
            self.status = "Open a drawing first.".into();
            return;
        };
        let source = doc.path.clone();
        let mut setting = Setting::new(job, &source);
        if job == Job::PageLabels {
            // The numbers Hyperview has already read off the drawings. This is
            // the whole point of the job: the work of finding them is done,
            // and writing them in is what makes every other reader show them.
            setting.labels = doc
                .labels
                .iter()
                .enumerate()
                .filter_map(|(page, label)| {
                    let number = label.number.trim();
                    (!number.is_empty()).then(|| (page as u32, number.to_string()))
                })
                .collect();
        }
        // Sheets picked in the Thumbnails panel are the ones meant.
        if job.picks_sheets() {
            if let Some(picked) = doc.picks.offered(doc.page) {
                setting.range = crate::picks::as_range(&picked);
            }
        }
        if job == Job::Export {
            setting.to = source
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default()
                .join(format!(
                    "{} pictures",
                    source
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "drawing".into())
                ))
                .display()
                .to_string();
        }
        self.doc_job = Some(setting);
    }

    pub fn document_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut setting) = self.doc_job.take() else {
            return;
        };
        let theme = self.chrome.theme;
        let makes_one = matches!(setting.job, Job::NewBlank | Job::FromPictures);
        let (source, sheets, here) = match self.doc() {
            Some(doc) => (doc.path.clone(), doc.pages.len(), doc.page as usize + 1),
            None if makes_one => (PathBuf::new(), 0, 1),
            None => return,
        };

        let mut keep = true;
        let mut go = false;
        let mut pick_others = false;
        let mut add_open = false; // WHAT: Add the other open drawings pressed. WHY: acted on after the window is drawn.
        let mut pick_from_list = false; // WHAT: Pick from a list pressed. WHY: the same.
        let others_open: Vec<PathBuf> = self.docs.iter().map(|d| d.path.clone()).filter(|p| *p != source).collect(); // WHAT: the other drawings open in tabs. WHY: what Add the other open drawings adds.
        let others_open_count = others_open.len(); // WHAT: how many. WHY: the button only shows when there are some.

        egui::Window::new(setting.job.title())
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .max_width(710.0)
            .show(ctx, |ui| {
                ui.set_min_width(470.0);
                ui.label(
                    RichText::new(setting.job.explanation())
                        .color(theme.faint)
                        .size(11.0),
                );
                ui.add_space(12.0);

                // ---- which sheets ------------------------------------------
                if setting.job.picks_sheets() {
                    ui.label(RichText::new("Sheets").color(theme.faint).size(11.0));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut setting.range)
                                .hint_text(format!("all {sheets} — or 3, 7, 12-18"))
                                .desired_width(220.0),
                        );
                        if ui.small_button("This sheet").clicked() {
                            setting.range = here.to_string();
                        }
                        if ui.small_button("All").clicked() {
                            setting.range.clear();
                        }
                    });
                    let picked = chosen(&setting.range, sheets);
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(format!(
                            "{} sheet{}.",
                            picked.len(),
                            if picked.len() == 1 { "" } else { "s" }
                        ))
                        .color(theme.faint)
                        .size(10.0),
                    );
                    ui.add_space(10.0);
                }

                // ---- the job's own questions -------------------------------
                match setting.job {
                    Job::NewBlank => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Sheets").color(theme.faint).size(11.0));
                            ui.add(
                                egui::TextEdit::singleline(&mut setting.new_sheets)
                                    .desired_width(60.0),
                            );
                        });
                        ui.add_space(8.0);
                        ui.label(RichText::new("Paper").color(theme.faint).size(11.0));
                        ui.add_space(4.0);
                        egui::ComboBox::from_id_salt("paper")
                            .selected_text(PAPERS[setting.paper.min(PAPERS.len() - 1)].0)
                            .width(260.0)
                            .show_ui(ui, |ui| {
                                for (at, (name, _)) in PAPERS.iter().enumerate() {
                                    ui.selectable_value(&mut setting.paper, at, *name);
                                }
                            });
                        ui.add_space(10.0);
                    }
                    Job::FromPictures => {
                        ui.label(
                            RichText::new("Pictures, in the order they go in")
                                .color(theme.faint)
                                .size(11.0),
                        );
                        ui.add_space(4.0);
                        if setting.others.is_empty() {
                            ui.label(
                                RichText::new("Nothing chosen yet.")
                                    .color(theme.faint)
                                    .size(11.0),
                            );
                        }
                        egui::ScrollArea::vertical()
                            .id_salt("from-pictures")
                            .max_height(120.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for other in &setting.others {
                                    ui.label(
                                        RichText::new(format!(
                                            "· {}",
                                            other
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy()
                                        ))
                                        .size(11.0),
                                    );
                                }
                            });
                        ui.add_space(4.0);
                        if ui.button("Choose…").clicked() {
                            pick_others = true;
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Treat them as").color(theme.faint).size(11.0),
                            );
                            for dpi in [150u32, 200, 300, 400, 600] {
                                ui.selectable_value(&mut setting.dpi, dpi, format!("{dpi} dpi"));
                            }
                        });
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(
                                "This decides how big each sheet comes out. A 300 dpi \
                                 scan of a D-size drawing at 300 dpi is D-size; at 72 it \
                                 is the size of a wall.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                        ui.add_space(10.0);
                    }
                    Job::Combine | Job::Insert | Job::SlipSheet => {
                        ui.label(
                            RichText::new(match setting.job {
                                Job::Combine => "Files to combine with this one",
                                Job::Insert => "File to insert",
                                _ => "The file with the revised sheets",
                            })
                            .color(theme.faint)
                            .size(11.0),
                        );
                        ui.add_space(4.0);
                        if setting.others.is_empty() {
                            ui.label(
                                RichText::new("Nothing chosen yet.")
                                    .color(theme.faint)
                                    .size(11.0),
                            );
                        }
                        for other in &setting.others {
                            ui.label(
                                RichText::new(format!(
                                    "· {}",
                                    other.file_name().unwrap_or_default().to_string_lossy()
                                ))
                                .size(11.0),
                            );
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button("Choose…").clicked() {
                                pick_others = true;
                            }
                            if setting.job == Job::Combine {
                                if others_open_count > 0 && ui.button("Add the other open drawings").on_hover_text("Every other drawing open in tabs now").clicked() {
                                    add_open = true; // WHAT: the drawings already open. WHY: a way to choose that needs no file window.
                                }
                                if crate::files::offer_own_list()
                                    && ui
                                        .button("Pick from a list…")
                                        .on_hover_text("Choose the files in Excalibur View's own list, for when the system's file window does not open or will not take several files. Dragging PDFs onto this window adds them too.")
                                        .clicked()
                                {
                                    pick_from_list = true; // WHAT: the program's own list. WHY: Linux's file window can be missing or refuse several files.
                                }
                                if !setting.others.is_empty() && ui.button("Clear").clicked() {
                                    setting.others.clear(); // WHAT: start the list again. WHY: choosing now adds rather than replaces.
                                }
                            }
                        });
                        if setting.job == Job::SlipSheet {
                            ui.add_space(8.0);
                            ui.checkbox(
                                &mut setting.keep_superseded,
                                "Keep the sheets they replace, at the back",
                            );
                            ui.label(
                                RichText::new(
                                    "Worth doing on a bid: it keeps what was superseded \
                                     where somebody can still look at it.",
                                )
                                .color(theme.faint)
                                .size(10.0),
                            );
                        }
                        if setting.job == Job::Insert {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(format!("They go in after sheet {here}."))
                                    .color(theme.faint)
                                    .size(11.0),
                            );
                        }
                        ui.add_space(10.0);
                    }
                    Job::Split => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Sheets per file").color(theme.faint).size(11.0));
                            ui.add(
                                egui::TextEdit::singleline(&mut setting.every).desired_width(60.0),
                            );
                        });
                        let every = setting.every.trim().parse::<usize>().unwrap_or(0);
                        if every > 0 {
                            let files = docops::in_runs_of(sheets, every).len();
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new(format!("{files} files."))
                                    .color(theme.faint)
                                    .size(10.0),
                            );
                        }
                        ui.add_space(10.0);
                    }
                    Job::Rotate => {
                        ui.label(RichText::new("Turn").color(theme.faint).size(11.0));
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut setting.quarter_turns, 1, "90° right");
                            ui.selectable_value(&mut setting.quarter_turns, 2, "180°");
                            ui.selectable_value(&mut setting.quarter_turns, 3, "90° left");
                        });
                        ui.add_space(10.0);
                    }
                    Job::Stamp => {
                        ui.label(
                            RichText::new("What goes where").color(theme.faint).size(11.0),
                        );
                        ui.add_space(4.0);
                        for (spot, text) in setting.stamps.iter_mut() {
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
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Size").color(theme.faint).size(11.0));
                            ui.add(
                                egui::Slider::new(&mut setting.stamp_size, 6.0..=36.0)
                                    .suffix(" pt"),
                            );
                        });
                        ui.add_space(6.0);
                        ui.label(RichText::new("Fields").color(theme.faint).size(10.0));
                        for (field, means) in crate::stamps::FIELDS {
                            ui.label(
                                RichText::new(format!("   {field}  —  {means}"))
                                    .color(theme.faint)
                                    .size(10.0),
                            );
                        }
                        ui.add_space(10.0);
                    }
                    Job::Shrink => {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Render at").color(theme.faint).size(11.0),
                            );
                            ui.selectable_value(&mut setting.dpi, 100, "100 dpi");
                            ui.selectable_value(&mut setting.dpi, 150, "150 dpi");
                            ui.selectable_value(&mut setting.dpi, 200, "200 dpi");
                            ui.selectable_value(&mut setting.dpi, 300, "300 dpi");
                        });
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(
                                "150 is readable on a screen. 300 prints. Higher is a \
                                 bigger file, which is the thing being avoided.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                        ui.add_space(10.0);
                    }
                    Job::Seal => {
                        ui.label(RichText::new("The seal").color(theme.faint).size(11.0));
                        ui.horizontal(|ui| {
                            match &setting.seal_picture {
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
                                        RichText::new("No picture chosen yet.")
                                            .color(theme.faint)
                                            .size(11.0),
                                    );
                                }
                            }
                            if ui.small_button("Choose…").clicked() {
                                pick_others = true;
                            }
                        });
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(
                                "A PNG or JPEG of your engineer's seal — theirs, scanned \
                                 or exported. A transparent PNG sits on the drawing \
                                 without a white box round it.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Width").color(theme.faint).size(11.0));
                            ui.add(
                                egui::Slider::new(&mut setting.seal_width, 60.0..=400.0)
                                    .suffix(" pt"),
                            );
                        });
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(
                                "It goes in the bottom-right corner of each sheet, keeping \
                                 its proportions.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                        ui.add_space(10.0);
                    }
                    Job::Ocr => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Read at").color(theme.faint).size(11.0));
                            for dpi in [150u32, 200, 300, 400] {
                                ui.selectable_value(
                                    &mut setting.dpi,
                                    dpi,
                                    format!("{dpi} dpi"),
                                );
                            }
                        });
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(
                                "300 is the usual answer for a scan. Higher takes longer \
                                 and rarely reads more.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                        ui.add_space(8.0);
                        ui.checkbox(
                            &mut setting.only_scans,
                            "Skip sheets that already have text",
                        );
                        ui.label(
                            RichText::new(
                                "Worth leaving on. Reading a drawing that was never \
                                 scanned puts a second, worse copy of every word under \
                                 the good one, and search then finds things twice.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                        ui.add_space(8.0);
                        match crate::ocr::engine_here() {
                            Some(engine) => {
                                ui.label(
                                    RichText::new(format!("Reading with {}.", engine.name()))
                                        .color(theme.faint)
                                        .size(10.0),
                                );
                            }
                            None => {
                                ui.label(
                                    RichText::new(crate::ocr::NO_ENGINE)
                                        .color(theme.warn)
                                        .size(10.0),
                                );
                            }
                        }
                        ui.add_space(10.0);
                    }
                    Job::Secure => {
                        ui.label(
                            RichText::new("Password to open it")
                                .color(theme.faint)
                                .size(11.0),
                        );
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut setting.open_password)
                                    .password(true)
                                    .hint_text("leave empty and anybody may open it")
                                    .desired_width(220.0),
                            );
                        });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut setting.open_password_again)
                                    .password(true)
                                    .hint_text("and again")
                                    .desired_width(220.0),
                            );
                            if setting.open_password != setting.open_password_again {
                                ui.label(
                                    RichText::new("These two do not match.")
                                        .color(theme.warn)
                                        .size(11.0),
                                );
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "This is the lock. Without it the drawings are \
                                 unreadable — Excalibur View keeps no copy of it and nobody \
                                 can recover it. Write it down before you send the set.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );

                        ui.add_space(12.0);
                        ui.label(
                            RichText::new("Owner's password")
                                .color(theme.faint)
                                .size(11.0),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut setting.owner_password)
                                .password(true)
                                .hint_text("leave empty to use the same one")
                                .desired_width(220.0),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(
                                "The password that gets past the permissions below, for \
                                 whoever has to change them later.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );

                        ui.add_space(12.0);
                        ui.label(RichText::new("Whoever opens it may").color(theme.faint).size(11.0));
                        ui.add_space(4.0);
                        egui::Grid::new("permissions")
                            .num_columns(2)
                            .spacing([24.0, 4.0])
                            .show(ui, |ui| {
                                ui.checkbox(&mut setting.allowed.print, "Print it");
                                ui.checkbox(
                                    &mut setting.allowed.print_well,
                                    "Print it at full size",
                                );
                                ui.end_row();
                                ui.checkbox(&mut setting.allowed.copy, "Copy text out of it");
                                ui.checkbox(&mut setting.allowed.annotate, "Mark it up");
                                ui.end_row();
                                ui.checkbox(&mut setting.allowed.change, "Change it");
                                ui.checkbox(
                                    &mut setting.allowed.assemble,
                                    "Take sheets out or put sheets in",
                                );
                                ui.end_row();
                                ui.checkbox(&mut setting.allowed.fill_forms, "Fill in forms");
                                ui.checkbox(
                                    &mut setting.allowed.read_aloud,
                                    "Read it with a screen reader",
                                );
                                ui.end_row();
                            });
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(
                                "These are a request the format makes of the reader, not \
                                 a lock. Acrobat and Revu honour them; somebody \
                                 determined with a different program does not have to. \
                                 The password above is the only thing that really keeps \
                                 a set shut.",
                            )
                            .color(theme.warn)
                            .size(10.0),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!(
                                "Written as AES-256, which Acrobat X and everything \
                                 after it, Revu, and every current reader opens. {}",
                                setting.allowed.in_words()
                            ))
                            .color(theme.faint)
                            .size(10.0),
                        );
                        ui.add_space(10.0);
                    }
                    Job::Export => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("As").color(theme.faint).size(11.0));
                            ui.selectable_value(
                                &mut setting.picture_format,
                                "png".into(),
                                "PNG",
                            );
                            ui.selectable_value(
                                &mut setting.picture_format,
                                "jpg".into(),
                                "JPEG",
                            );
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("At").color(theme.faint).size(11.0));
                            for dpi in [150u32, 200, 300, 600] {
                                ui.selectable_value(&mut setting.dpi, dpi, format!("{dpi} dpi"));
                            }
                        });
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(
                                "150 is for putting a sheet in an email. 300 prints. \
                                 600 on a D-size drawing is a very large file.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                        ui.add_space(10.0);
                    }
                    Job::Redact => {
                        ui.checkbox(
                            &mut setting.redact_as_pictures,
                            "Turn the affected sheets into pictures",
                        );
                        ui.label(
                            RichText::new(
                                "Leave it off and the things under each box are taken out \
                                 of the drawing, which keeps the sheet a drawing — \
                                 searchable, and still line-work. Anything that reaches \
                                 outside a box goes with it, and you are told how much.\n\n\
                                 Turn it on and those sheets become pictures. That is the \
                                 only way to be certain nothing of the words is left \
                                 anywhere in the file, and it is what to do when the set \
                                 is going outside the company.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                        if setting.redact_as_pictures {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("At").color(theme.faint).size(11.0));
                                for dpi in [150u32, 200, 300] {
                                    ui.selectable_value(
                                        &mut setting.dpi,
                                        dpi,
                                        format!("{dpi} dpi"),
                                    );
                                }
                            });
                        }
                        ui.add_space(10.0);
                    }
                    Job::PageLabels => {
                        if setting.labels.is_empty() {
                            ui.label(
                                RichText::new(
                                    "No sheet numbers have been read off this set yet. \
                                     They are read as the sheets are opened; on a scanned \
                                     set, run OCR first and they will be there.",
                                )
                                .color(theme.warn)
                                .size(11.0),
                            );
                        } else {
                            ui.label(
                                RichText::new(format!(
                                    "{} sheet number{} read off the drawings:",
                                    setting.labels.len(),
                                    if setting.labels.len() == 1 { "" } else { "s" }
                                ))
                                .color(theme.faint)
                                .size(11.0),
                            );
                            ui.add_space(4.0);
                            egui::ScrollArea::vertical()
                                .id_salt("page-labels")
                                .max_height(140.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    for (page, label) in &setting.labels {
                                        ui.label(
                                            RichText::new(format!(
                                                "sheet {}  →  {label}",
                                                page + 1
                                            ))
                                            .size(11.0),
                                        );
                                    }
                                });
                        }
                        ui.add_space(10.0);
                    }
                    Job::Extract => {
                        ui.checkbox(
                            &mut setting.and_remove,
                            "Also write the sheets that are left over",
                        );
                        ui.label(
                            RichText::new(
                                "Two files rather than one. The drawing you have open is \
                                 not changed either way.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                        ui.add_space(10.0);
                    }
                    _ => {}
                }

                // ---- where it goes -----------------------------------------
                ui.label(
                    RichText::new(if matches!(setting.job, Job::Split | Job::Export) {
                        "Into this folder"
                    } else {
                        "Write it to"
                    })
                    .color(theme.faint)
                    .size(11.0),
                );
                ui.horizontal(|ui| {
                    let width = ui::chrome::room_beside(ui, "Browse…");
                    ui.add(
                        egui::TextEdit::singleline(&mut setting.to)
                            .desired_width(width),
                    );
                    if ui.button("Browse…").clicked() {
                        let choose = if setting.job == Job::Split {
                            crate::files::Choose::folder()
                        } else {
                            crate::files::Choose::save(
                                std::path::Path::new(&setting.to)
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy(),
                            )
                            .filter("Drawing sets (PDF)", &["pdf"])
                        };
                        self.filing.one(choose, |app, picked| {
                            if let Some(setting) = app.doc_job.as_mut() {
                                setting.to = picked.display().to_string();
                            }
                        });
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "The drawing you have open is never changed. If that name is \
                         taken, a number is added rather than anything being written over.",
                    )
                    .color(theme.faint)
                    .size(10.0),
                );

                // ---- the one that loses something --------------------------
                if let Some(op) = build(&setting, &source, sheets, here) {
                    if let Some(warning) = op.needs_a_warning() {
                        ui.add_space(10.0);
                        egui::Frame::NONE
                            .fill(theme.sunken)
                            .inner_margin(egui::Margin::same(8))
                            .corner_radius(4)
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(warning).color(theme.warn).size(11.0),
                                );
                            });
                    }
                }

                if let Some(error) = &setting.error {
                    ui.add_space(8.0);
                    ui.colored_label(Color32::from_rgb(235, 120, 120), error);
                }

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let ready = !setting.running
                        && build(&setting, &source, sheets, here).is_some();
                    if ui
                        .add_enabled(ready, egui::Button::new(setting.job.title()))
                        .clicked()
                    {
                        go = true;
                    }
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                    if setting.running {
                        ui.add_space(8.0);
                        ui.spinner();
                    }
                });
            });

        if pick_others && setting.job == Job::Seal {
            self.filing.one(
                crate::files::Choose::open()
                    .filter("Pictures", &["png", "jpg", "jpeg", "PNG", "JPG", "JPEG"])
                    .title("The engineer's seal"),
                |app, path| {
                    if let Some(setting) = app.doc_job.as_mut() {
                        setting.seal_picture = Some(path);
                    }
                },
            );
        } else if pick_others || pick_from_list {
            let choose = if setting.job == Job::FromPictures {
                crate::files::Choose::open_many()
                    .filter(
                        "Pictures",
                        &["png", "jpg", "jpeg", "PNG", "JPG", "JPEG", "tif", "tiff"],
                    )
                    .title("The pictures to make a drawing set from")
            } else if setting.job == Job::Combine {
                crate::files::Choose::open_many().filter("Drawing sets (PDF)", &["pdf"])
            } else {
                crate::files::Choose::open().filter("Drawing sets (PDF)", &["pdf"])
            };
            let choose = if pick_from_list { choose.here() } else { choose }; // WHAT: the program's own list, when asked for. WHY: see Pick from a list.
            self.filing.many(choose, |app, picked| {
                if let Some(setting) = app.doc_job.as_mut() {
                    if setting.job == Job::Combine {
                        add_others(setting, picked); // WHAT: added to the list, each once. WHY: files from two folders can be combined, a few at a time.
                    } else {
                        setting.others = picked; // WHAT: the one file. WHY: inserting or slip-sheeting takes one.
                    }
                }
            });
        }
        if add_open {
            add_others(&mut setting, others_open); // WHAT: the other open drawings added. WHY: Add the other open drawings.
        }

        if go {
            if let Some(op) = build(&setting, &source, sheets, here) {
                setting.running = true;
                setting.error = None;
                self.doc_task += 1;
                let job = self.doc_task;
                self.svc.send(crate::render::ToWorker::Document {
                    job,
                    op: Box::new(op),
                });
            }
        }
        if keep {
            self.doc_job = Some(setting);
        }
    }

    pub fn document_done(&mut self, job: u64, done: docops::Done) {
        if job != self.doc_task {
            return;
        }
        if self.quiet_task.take() == Some(job) {
            // Picked sheets saved from the Thumbnails panel: say where they
            // went, and stay on the drawing being worked on.
            self.status = match done.wrote.first() {
                Some(path) => format!("Saved: {}", path.display()),
                None => done.said.clone(),
            };
            if std::mem::take(&mut self.quiet_left_out) {
                self.status.push_str(
                    " Markups not yet saved into this drawing aren't in it; Save As first to include them.",
                );
            }
            return;
        }
        self.doc_job = None;
        self.status = done.said.clone();
        // Opened straight away when it is one file, because the next thing
        // anybody does after combining four sets is look at the result.
        if done.wrote.len() == 1 {
            let path = done.wrote[0].clone();
            self.open(path);
        }
    }

    pub fn document_failed(&mut self, job: u64, why: String) {
        if job != self.doc_task {
            return;
        }
        if self.quiet_task.take() == Some(job) {
            self.quiet_left_out = false;
            self.error = Some(format!("Could not save the sheets: {why}"));
            return;
        }
        match self.doc_job.as_mut() {
            Some(setting) => {
                setting.running = false;
                setting.error = Some(why);
            }
            None => self.error = Some(why),
        }
    }
}

/// Reads the sheet box, treating empty as "all of them".
fn chosen(range: &str, sheets: usize) -> Vec<u32> {
    if range.trim().is_empty() {
        return (0..sheets as u32).collect();
    }
    crate::print::read_range(range, sheets)
}

/// Adds files to a Combine's list, each once and in the order given. Before
/// this, choosing replaced the list, so files in two folders could not be
/// combined, and on a desktop whose file window picks one file at a time
/// (a Linux one without its portal) only one could ever be listed.
pub fn add_others(setting: &mut Setting, files: impl IntoIterator<Item = PathBuf>) {
    for file in files {
        if !setting.others.contains(&file) {
            setting.others.push(file); // WHAT: on the end. WHY: combined in the order chosen.
        }
    }
}

/// Turns what is on the window into the operation it describes, or `None` when
/// it does not describe one yet — which is also what greys out the button.
fn build(
    setting: &Setting,
    source: &std::path::Path,
    sheets: usize,
    here: usize,
) -> Option<Operation> {
    let to = PathBuf::from(setting.to.trim());
    if setting.to.trim().is_empty() {
        return None;
    }
    let pages = chosen(&setting.range, sheets);
    if setting.job.picks_sheets() && pages.is_empty() {
        return None;
    }
    Some(match setting.job {
        Job::Combine => {
            if setting.others.is_empty() {
                return None;
            }
            let mut sources = vec![source.to_path_buf()];
            sources.extend(setting.others.iter().cloned());
            Operation::Combine { sources, to }
        }
        Job::Split => {
            let every = setting.every.trim().parse::<usize>().ok().filter(|e| *e > 0)?;
            Operation::Split {
                source: source.to_path_buf(),
                into: to,
                every,
            }
        }
        Job::Extract => Operation::Extract {
            source: source.to_path_buf(),
            pages,
            to,
            and_remove: setting.and_remove,
        },
        Job::Insert => Operation::Insert {
            source: source.to_path_buf(),
            insert: setting.others.first()?.clone(),
            at: here,
            to,
        },
        Job::Delete => Operation::Delete {
            source: source.to_path_buf(),
            pages,
            to,
        },
        Job::Rotate => Operation::Rotate {
            source: source.to_path_buf(),
            pages,
            quarter_turns: setting.quarter_turns,
            to,
        },
        Job::Crop => Operation::Crop {
            source: source.to_path_buf(),
            pages,
            // Set from the view's own crop rectangle when there is one; the
            // whole page otherwise, which is a no-op somebody can see.
            box_: [0.0, 0.0, 0.0, 0.0],
            to,
        },
        Job::SlipSheet => Operation::SlipSheet {
            source: source.to_path_buf(),
            revisions: setting.others.first()?.clone(),
            to,
            keep_superseded: setting.keep_superseded,
        },
        Job::Flatten => Operation::Flatten {
            source: source.to_path_buf(),
            pages,
            to,
        },
        Job::Stamp => {
            let stamps: Vec<crate::stamps::Stamp> = setting
                .stamps
                .iter()
                .filter(|(_, text)| !text.trim().is_empty())
                .map(|(spot, text)| crate::stamps::Stamp {
                    spot: *spot,
                    text: text.clone(),
                    size: setting.stamp_size,
                    ..Default::default()
                })
                .collect();
            if stamps.is_empty() {
                return None;
            }
            Operation::Stamp {
                source: source.to_path_buf(),
                pages,
                stamps,
                to,
            }
        }
        Job::Shrink => Operation::Shrink {
            source: source.to_path_buf(),
            to,
            dpi: setting.dpi,
        },
        Job::Seal => Operation::Seal {
            source: source.to_path_buf(),
            picture: setting.seal_picture.clone()?,
            pages,
            placement: crate::seal::Placement {
                x: 0.0,
                y: 0.0,
                width: setting.seal_width,
                corner: None,
            },
            to,
        },
        Job::Ocr => {
            if crate::ocr::engine_here().is_none() {
                return None;
            }
            Operation::Ocr {
                source: source.to_path_buf(),
                pages,
                to,
                dpi: setting.dpi,
                only_scans: setting.only_scans,
            }
        }
        Job::Repair => Operation::Repair {
            source: source.to_path_buf(),
            to,
        },
        Job::Unflatten => Operation::Unflatten {
            source: source.to_path_buf(),
            to,
        },
        Job::PageLabels => Operation::PageLabels {
            source: source.to_path_buf(),
            labels: setting.labels.clone(),
            to,
        },
        Job::Redact => Operation::ApplyRedactions {
            source: source.to_path_buf(),
            to,
            as_pictures: setting.redact_as_pictures,
            dpi: setting.dpi,
        },
        Job::Secure => {
            // Typed twice, and they must match: a password nobody can check is
            // a set nobody gets back.
            if setting.open_password != setting.open_password_again {
                return None;
            }
            Operation::Secure {
                source: source.to_path_buf(),
                to,
                open_password: setting.open_password.clone(),
                owner_password: setting.owner_password.clone(),
                allowed: setting.allowed,
            }
        }
        Job::NewBlank => {
            let sheets = setting.new_sheets.trim().parse::<usize>().ok().filter(|n| *n > 0)?;
            Operation::NewBlank {
                to,
                sheets,
                size: PAPERS.get(setting.paper)?.1,
            }
        }
        Job::FromPictures => {
            if setting.others.is_empty() {
                return None;
            }
            Operation::FromPictures {
                pictures: setting.others.clone(),
                to,
                dpi: setting.dpi,
            }
        }
        Job::Export => Operation::ExportPictures {
            source: source.to_path_buf(),
            pages,
            into: to,
            dpi: setting.dpi,
            format: setting.picture_format.clone(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choosing_files_to_combine_again_adds_to_the_list_rather_than_replacing_it() {
        let mut setting = Setting::new(Job::Combine, std::path::Path::new("Set.pdf")); // WHAT: Document, Combine. WHY: the window that is filled.
        add_others(&mut setting, vec![PathBuf::from("/a/One.pdf")]); // WHAT: one file chosen. WHY: a file window that picks one at a time.
        add_others(&mut setting, vec![PathBuf::from("/b/Two.PDF"), PathBuf::from("/a/One.pdf")]); // WHAT: then another, and the first again. WHY: chosen a second time, from another folder.
        assert_eq!(setting.others, vec![PathBuf::from("/a/One.pdf"), PathBuf::from("/b/Two.PDF")]); // WHAT: both, once each, in order. WHY: choosing used to replace the list, so only the last pick was combined.
        assert!(matches!(build(&setting, std::path::Path::new("Set.pdf"), 1, 1), Some(Operation::Combine { ref sources, .. }) if sources.len() == 3)); // WHAT: this set and both others. WHY: what gets combined.
    }

    #[test]
    fn an_empty_sheet_box_means_all_of_them() {
        assert_eq!(chosen("", 4), vec![0, 1, 2, 3]);
        assert_eq!(chosen("   ", 4), vec![0, 1, 2, 3]);
        assert_eq!(chosen("2", 4), vec![1]);
    }

    #[test]
    fn a_job_with_nowhere_to_write_is_not_ready() {
        let mut setting = Setting::new(Job::Delete, std::path::Path::new("Set.pdf"));
        setting.to = String::new();
        assert!(build(&setting, std::path::Path::new("Set.pdf"), 4, 1).is_none());
    }

    #[test]
    fn combining_with_nothing_chosen_is_not_ready() {
        let setting = Setting::new(Job::Combine, std::path::Path::new("Set.pdf"));
        assert!(build(&setting, std::path::Path::new("Set.pdf"), 4, 1).is_none());
    }

    #[test]
    fn the_drawing_that_is_open_comes_first_when_combining() {
        // Because "combine" from an open drawing means "this one, then those".
        let mut setting = Setting::new(Job::Combine, std::path::Path::new("Set.pdf"));
        setting.others = vec![PathBuf::from("Other.pdf")];
        match build(&setting, std::path::Path::new("Set.pdf"), 4, 1) {
            Some(Operation::Combine { sources, .. }) => {
                assert_eq!(sources[0], PathBuf::from("Set.pdf"));
                assert_eq!(sources[1], PathBuf::from("Other.pdf"));
            }
            _ => panic!("that is a combine"),
        }
    }

    #[test]
    fn splitting_by_a_number_that_is_not_one_is_not_ready() {
        let mut setting = Setting::new(Job::Split, std::path::Path::new("Set.pdf"));
        setting.every = "0".into();
        assert!(build(&setting, std::path::Path::new("Set.pdf"), 4, 1).is_none());
        setting.every = "banana".into();
        assert!(build(&setting, std::path::Path::new("Set.pdf"), 4, 1).is_none());
        setting.every = "2".into();
        assert!(build(&setting, std::path::Path::new("Set.pdf"), 4, 1).is_some());
    }

    #[test]
    fn every_job_starts_pointed_at_a_new_file_beside_the_old_one() {
        for job in [
            Job::Combine, Job::Extract, Job::Delete, Job::Rotate,
            Job::Crop, Job::SlipSheet, Job::Flatten, Job::Insert,
        ] {
            let setting = Setting::new(job, std::path::Path::new("/tmp/Set.pdf"));
            // Never the file it came from. That is the whole rule.
            assert_ne!(setting.to, "/tmp/Set.pdf", "{job:?}");
            assert!(setting.to.ends_with(".pdf"), "{job:?}: {}", setting.to);
        }
    }
}
