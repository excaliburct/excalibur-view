//! Comparing two issues of a sheet, on screen.
//!
//! The morning job: Rev 2 landed, and the question is what moved. This picks
//! the two sheets, runs the comparison, and shows the differences both as a
//! list you can click through and as marks on the drawing.
//!
//! Then the part that saves the hour: **Cloud these**, which turns the
//! differences into real revision clouds — actual PDF annotations on the newer
//! sheet, which Revu and Acrobat will show and which travel with the file when
//! it goes to a general contractor.
//!
//! What it never does is put a number on any of it. A comparison says "look
//! here"; every quantity in this program still comes from somebody measuring.

use std::path::PathBuf;

use egui::{Color32, RichText};

use crate::app::App;
use crate::compare::Kind;

/// What is being compared, and what came of it.
#[derive(Clone, Debug)]
pub struct Comparing {
    /// The older issue. Empty until chosen.
    pub older: Option<PathBuf>,
    pub older_page: u32,
    /// The newer issue is the drawing that is open, on the sheet being looked
    /// at, because that is the one somebody just received.
    pub newer_page: u32,
    /// That drawing, kept from when the window opened, so the clouds go on
    /// the sheet that was compared whatever tab or page is showing later.
    pub newer: PathBuf, // WHAT: the compared drawing. WHY: clouds go on it whatever is in front later.
    /// How dark a pixel has to be to count as line-work.
    pub darkness: f32,
    pub running: bool,
    pub error: Option<String>,
    /// What was found, once it has been.
    pub found: Option<Found>,
}

#[derive(Clone, Debug)]
pub struct Found {
    pub said: String,
    pub could_not_align: bool,
    pub changes: Vec<(Kind, [f64; 4])>,
    /// Which one is being pointed at.
    pub showing: Option<usize>,
}

impl Comparing {
    pub fn new(newer: PathBuf, page: u32) -> Comparing { // WHAT: a compare window for this drawing and sheet. WHY: both kept for the window's life.
        Comparing {
            older: None,
            older_page: page,
            newer_page: page,
            newer, // WHAT: the drawing in front now. WHY: compared and clouded, even after a tab change.
            darkness: 0.65,
            running: false,
            error: None,
            found: None,
        }
    }
}

impl App {
    pub fn begin_compare(&mut self) {
        let Some(doc) = self.doc() else {
            self.status = "Open the newer issue first, then compare it with the older one.".into();
            return;
        };
        self.comparing = Some(Comparing::new(doc.path.clone(), doc.page)); // WHAT: the window, for this drawing and sheet. WHY: kept even if the tab or page changes.
    }

    pub fn compare_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut comparing) = self.comparing.take() else {
            return;
        };
        let theme = self.chrome.theme;
        let newer = comparing.newer.clone(); // WHAT: the drawing being compared. WHY: the one chosen when the window opened, not the tab in front.
        let sheets = self.docs.iter().find(|d| d.path == newer).map_or(0, |d| d.pages.len()); // WHAT: its sheet count, if it is still open. WHY: shown.

        let mut keep = true;
        let mut go = false;
        let mut pick_older = false;
        let mut cloud = false;
        let mut show: Option<usize> = None;

        egui::Window::new("Compare Documents")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .max_width(740.0)
            .show(ctx, |ui| {
                ui.set_min_width(500.0);

                ui.label(RichText::new("The newer issue").color(theme.faint).size(11.0));
                ui.label(
                    RichText::new(format!(
                        "{} — sheet {} of {sheets}",
                        newer.file_name().unwrap_or_default().to_string_lossy(),
                        comparing.newer_page + 1
                    ))
                    .size(11.0),
                );
                ui.add_space(10.0);

                ui.label(RichText::new("The older issue").color(theme.faint).size(11.0));
                ui.horizontal(|ui| {
                    match &comparing.older {
                        Some(path) => {
                            ui.label(
                                RichText::new(
                                    path.file_name().unwrap_or_default().to_string_lossy(),
                                )
                                .size(11.0),
                            );
                        }
                        None => {
                            ui.label(
                                RichText::new("Nothing chosen yet.")
                                    .color(theme.faint)
                                    .size(11.0),
                            );
                        }
                    }
                    if ui.small_button("Choose…").clicked() {
                        pick_older = true;
                    }
                });
                if comparing.older.is_some() {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Sheet").color(theme.faint).size(11.0));
                        let mut n = comparing.older_page + 1;
                        if ui
                            .add(egui::DragValue::new(&mut n).range(1..=9999))
                            .changed()
                        {
                            comparing.older_page = n.saturating_sub(1);
                        }
                        ui.label(
                            RichText::new("in the older file")
                                .color(theme.faint)
                                .size(10.0),
                        );
                    });
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Line darkness").color(theme.faint).size(11.0));
                    ui.add(egui::Slider::new(&mut comparing.darkness, 0.3..=0.9).show_value(false));
                    ui.label(
                        RichText::new("Raise it for a faint scan.")
                            .color(theme.faint)
                            .size(10.0),
                    );
                });

                // ---- what was found ----------------------------------------
                if let Some(found) = &comparing.found {
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(&found.said)
                            .color(if found.could_not_align {
                                theme.warn
                            } else {
                                theme.text
                            })
                            .size(11.0),
                    );

                    if !found.changes.is_empty() {
                        ui.add_space(8.0);
                        egui::ScrollArea::vertical()
                            .max_height(180.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for (n, (kind, area)) in found.changes.iter().enumerate() {
                                    let picked = found.showing == Some(n);
                                    let label = format!(
                                        "{}  {}  ·  {:.0} × {:.0}",
                                        n + 1,
                                        kind.word(),
                                        area[2] - area[0],
                                        area[3] - area[1]
                                    );
                                    if ui
                                        .selectable_label(picked, RichText::new(label).size(11.0))
                                        .clicked()
                                    {
                                        show = Some(n);
                                    }
                                }
                            });
                        ui.add_space(8.0);
                        if ui
                            .button("Cloud these on the drawing")
                            .on_hover_text(
                                "Draws a revision cloud round each difference, as real \
                                 markups. They go in the markups list, they save into the \
                                 file, and Revu will show them.",
                            )
                            .clicked()
                        {
                            cloud = true;
                        }
                    }
                }

                if let Some(error) = &comparing.error {
                    ui.add_space(8.0);
                    ui.colored_label(Color32::from_rgb(235, 120, 120), error);
                }

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let ready = comparing.older.is_some() && !comparing.running;
                    if ui.add_enabled(ready, egui::Button::new("Compare")).clicked() {
                        go = true;
                    }
                    if ui.button("Close").clicked() {
                        keep = false;
                    }
                    if comparing.running {
                        ui.add_space(8.0);
                        ui.spinner();
                        ui.label(
                            RichText::new("Looking at both sheets…")
                                .color(theme.faint)
                                .size(11.0),
                        );
                    }
                });
            });

        if pick_older {
            self.filing.one(
                crate::files::Choose::open()
                    .filter("Drawing sets (PDF)", &["pdf"])
                    .title("The older issue"),
                |app, path| {
                    if let Some(comparing) = app.comparing.as_mut() {
                        comparing.older = Some(path);
                        comparing.found = None;
                    }
                },
            );
        }
        if let Some(n) = show {
            if let Some(found) = comparing.found.as_mut() {
                found.showing = Some(n);
                let area = found.changes[n].1;
                self.centre_on_area(&comparing.newer, comparing.newer_page, area); // WHAT: shows that change on the compared sheet. WHY: not on whatever sheet happens to be showing.
            }
        }
        if go {
            if let Some(older) = comparing.older.clone() {
                comparing.running = true;
                comparing.error = None;
                comparing.found = None;
                self.compare_task += 1;
                let job = self.compare_task;
                self.svc.send(crate::render::ToWorker::Compare {
                    job,
                    older,
                    older_page: comparing.older_page,
                    newer: newer.clone(),
                    newer_page: comparing.newer_page,
                    darkness: comparing.darkness,
                });
            }
        }
        if cloud {
            if let Some(found) = comparing.found.clone() {
                let made = self.cloud_the_changes(&found.changes, &comparing.newer, comparing.newer_page); // WHAT: clouds on the compared sheet. WHY: never on whatever page or tab is in front now.
                self.status = made;
            }
        }
        if keep {
            self.comparing = Some(comparing);
        }
    }

    pub fn comparison_done(&mut self, job: u64, found: crate::render::Compared) {
        if job != self.compare_task {
            return;
        }
        if let Some(comparing) = self.comparing.as_mut() {
            comparing.running = false;
            comparing.found = Some(Found {
                said: found.said.clone(),
                could_not_align: found.could_not_align,
                changes: found.changes.clone(),
                showing: None,
            });
        }
        self.status = found.said;
    }

    pub fn comparison_failed(&mut self, job: u64, why: String) {
        if job != self.compare_task {
            return;
        }
        match self.comparing.as_mut() {
            Some(comparing) => {
                comparing.running = false;
                comparing.error = Some(why);
            }
            None => self.error = Some(why),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comparison_starts_on_the_sheet_being_looked_at() {
        // Because the sheet somebody is on is the one they just opened the
        // revision to look at.
        let comparing = Comparing::new(PathBuf::from("Rev 2.pdf"), 7); // WHAT: a window on sheet 8. WHY: the test.
        assert_eq!(comparing.newer_page, 7);
        assert_eq!(comparing.older_page, 7);
        assert!(comparing.older.is_none());
        assert!(comparing.found.is_none());
    }

    #[test]
    fn the_darkness_starts_where_a_plotted_drawing_wants_it() {
        let comparing = Comparing::new(PathBuf::from("Rev 2.pdf"), 0); // WHAT: a window on sheet 1. WHY: the test.
        assert!(comparing.darkness > 0.5 && comparing.darkness < 0.8);
    }
}

// ---- turning differences into markups --------------------------------------

impl App {
    /// Draws a revision cloud round each difference, as real annotations.
    ///
    /// A cloud is not a decoration this program invents: it is a Square
    /// annotation with a cloudy border effect, which is exactly what Revu
    /// writes and what every other reader already knows how to draw. So these
    /// go in the markups list, save into the file, and appear for whoever the
    /// set is sent to.
    ///
    /// They carry no measurement. A cloud says "this changed", and if somebody
    /// needs to know how much steel that is, they measure it with a tool — the
    /// same rule as everywhere else in this program.
    pub fn cloud_the_changes(&mut self, changes: &[(Kind, [f64; 4])], newer: &std::path::Path, page: u32) -> String { // WHAT: clouds the changes on the drawing and sheet that were compared. WHY: never on whatever is in front now.
        if changes.is_empty() {
            return "There was nothing to cloud.".into();
        }
        let author = self.author.clone();
        let Some(doc) = self.docs.iter_mut().find(|d| d.path == newer) else { // WHAT: the compared drawing, if still open. WHY: its sheet gets the clouds.
            return format!("{} is no longer open, so nothing was clouded. Open it and compare again.", newer.file_name().unwrap_or_default().to_string_lossy()); // WHAT: says so. WHY: clouds on another drawing would be on the wrong sheet.
        };
        if page as usize >= doc.pages.len() { // WHAT: the sheet is gone from it. WHY: nowhere right to put clouds.
            return "That sheet is no longer in the drawing, so nothing was clouded.".into(); // WHAT: says so. WHY: the same.
        }
        let frame = doc.frame_of(page);

        let mut made = 0usize;
        doc.checkpoint();
        for (kind, area) in changes {
            // A little room around it, so the cloud sits outside what changed
            // rather than through it.
            let margin = 6.0;
            let corners = [
                [area[0] - margin, area[1] - margin],
                [area[2] + margin, area[3] + margin],
            ];
            let colour = match kind {
                Kind::Added => [0x2f, 0x9e, 0x44, 0xff],
                Kind::Removed => [0xd0, 0x45, 0x3c, 0xff],
                Kind::Changed => [0xe0, 0x8b, 0x1a, 0xff],
            };
            let draft = crate::sheet::Draft {
                tool: crate::app::Tool::Rect,
                points: corners.to_vec(),
                strokes: Vec::new(),
                subject: format!("Revision — {}", kind.word()),
                colour,
                template: None,
                ..crate::sheet::Draft::default()
            };
            let Some(mut markup) = draft.into_markup(&frame, None) else {
                continue;
            };
            // The border effect that makes it a cloud rather than a box. `/I`
            // is how deep the scallops are; 2 is what Revu uses by default and
            // what people recognise.
            let mut effect = pdf::Dict::new();
            effect.set("S", pdf::Object::name("C"));
            effect.set("I", pdf::Object::Real(2.0));
            markup.dict.set("BE", pdf::Object::Dict(effect));
            markup.dict.set("T", pdf::Object::text(&author));
            doc.marks.push(crate::sheet::Mark::new(page, markup));
            made += 1;
        }
        doc.dirty = true;

        format!(
            "{made} revision cloud{} drawn on this sheet. They are markups like any \
             other — edit them, delete them, or save and send the set on. Nothing has \
             been measured.",
            if made == 1 { "" } else { "s" }
        )
    }

    /// Puts a difference in the middle of the window.
    pub fn centre_on_area(&mut self, newer: &std::path::Path, page: u32, area: [f64; 4]) { // WHAT: shows a change on the compared sheet. WHY: the list is about that sheet.
        let middle = [(area[0] + area[2]) * 0.5, (area[1] + area[3]) * 0.5];
        let canvas = self.last_canvas;
        let Some(at) = self.docs.iter().position(|d| d.path == newer) else { return }; // WHAT: the compared drawing's tab. WHY: the change is on it.
        self.current = at; // WHAT: brings it to the front. WHY: the change is shown where it is.
        self.go_to(page); // WHAT: turns to the compared sheet. WHY: the area is on that sheet.
        if let Some(doc) = self.doc_mut() {
            doc.view.centre_on(middle, canvas);
        }
    }
}

// ---- two issues, one on top of the other -----------------------------------

/// How the overlay window lines the two sheets up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineUp { // WHAT: the three ways to line up. WHY: chosen in the window.
    ByDrawing, // WHAT: found from the line-work. WHY: right nearly always, and nothing to do.
    ByPoints,  // WHAT: by two points picked on each sheet. WHY: for the sheet the drawing alone cannot line up.
    AsTheyAre, // WHAT: not lined up at all. WHY: to see the two exactly as they were issued.
}

/// The two sheets drawn small, for picking points on.
#[derive(Clone)]
pub struct Looks { // WHAT: both sheets drawn small with their sizes. WHY: points are picked on them.
    pub older: egui::TextureHandle, // WHAT: the older sheet's picture. WHY: clicked on.
    pub older_size: [f64; 2],       // WHAT: its size in points. WHY: a click becomes a point on it.
    pub newer: egui::TextureHandle, // WHAT: the newer sheet's picture. WHY: clicked on.
    pub newer_size: [f64; 2],       // WHAT: its size. WHY: the same.
}

/// The overlay window's state.
#[derive(Clone)]
pub struct Overlaying {
    pub older: Option<PathBuf>,
    pub older_page: u32,
    /// The newer issue: the drawing that was in front when the window opened.
    /// Kept, rather than read off whichever tab is in front each moment, so
    /// changing tabs never lays the older sheet over a different drawing.
    pub newer: PathBuf, // WHAT: the newer issue. WHY: kept for the window's life.
    pub newer_page: u32,
    pub newer_sheets: usize, // WHAT: how many sheets the newer issue has. WHY: its sheet number cannot go past them.
    pub darkness: f32,
    pub line_up: LineUp, // WHAT: how to line them up. WHY: the drawing, two points, or not at all.
    pub older_points: Vec<[f64; 2]>, // WHAT: points picked on the older sheet, in its points. WHY: the two-point line-up.
    pub newer_points: Vec<[f64; 2]>, // WHAT: points picked on the newer sheet. WHY: the same.
    pub looks: Option<Looks>,        // WHAT: both sheets drawn small, once asked for. WHY: points are picked on them.
    pub looking: bool,               // WHAT: waiting for them. WHY: shows a spinner and does not ask twice.
    pub running: bool,
    pub error: Option<String>,
    pub said: Option<String>,
    pub picture: Option<egui::TextureHandle>,
}

impl Overlaying {
    pub fn new(newer: PathBuf, page: u32, sheets: usize) -> Overlaying { // WHAT: an overlay window for this drawing and sheet. WHY: kept for its life.
        Overlaying {
            older: None,
            older_page: page,
            newer, // WHAT: the drawing in front now. WHY: kept for the life of the window.
            newer_page: page,
            newer_sheets: sheets, // WHAT: its sheet count. WHY: bounds the sheet number.
            darkness: 0.65,
            line_up: LineUp::ByDrawing, // WHAT: lined up by the drawing. WHY: the right answer nearly always.
            older_points: Vec::new(), // WHAT: none picked yet. WHY: picked when asked for.
            newer_points: Vec::new(), // WHAT: the same. WHY: the same.
            looks: None,              // WHAT: not drawn yet. WHY: only when picking points.
            looking: false,           // WHAT: not waiting. WHY: nothing asked for yet.
            running: false,
            error: None,
            said: None,
            picture: None,
        }
    }

    /// How the engine is to line them up, or None when two points are wanted
    /// on each sheet and not all four are picked yet.
    #[must_use]
    pub fn lining(&self) -> Option<crate::overlay::Lining> { // WHAT: the line-up to send, if there is one yet. WHY: Overlay stays greyed until there is.
        match self.line_up { // WHAT: by the way chosen. WHY: each needs something different.
            LineUp::ByDrawing => Some(crate::overlay::Lining::Auto), // WHAT: found. WHY: asked for.
            LineUp::AsTheyAre => Some(crate::overlay::Lining::AsTheyAre), // WHAT: uncorrected. WHY: asked for.
            LineUp::ByPoints => match (self.older_points.as_slice(), self.newer_points.as_slice()) { // WHAT: the points picked so far on each sheet. WHY: all four are needed.
                ([a, b], [c, d]) => Some(crate::overlay::Lining::Points { older: [*a, *b], newer: [*c, *d] }), // WHAT: the four points. WHY: two on each sheet.
                _ => None, // WHAT: not all picked. WHY: nothing to line up by yet.
            },
        }
    }

    /// Forgets what was picked and drawn, because a sheet or file changed.
    pub fn sheets_changed(&mut self) { // WHAT: forgets per-sheet work. WHY: a sheet or file changed.
        self.older_points.clear(); // WHAT: forgets the older points. WHY: they were on another sheet.
        self.newer_points.clear(); // WHAT: the same. WHY: the same.
        self.looks = None; // WHAT: forgets the pictures. WHY: of the old sheets.
        self.picture = None; // WHAT: forgets the overlay. WHY: of the old sheets.
        self.said = None; // WHAT: forgets what it said. WHY: about the old sheets.
    }
}

/// Shows a sheet to pick points on, `width` wide. A click adds a point, in
/// the sheet's own points; a third click starts that sheet's pair again.
///
/// The picked points are drawn as numbered rings, so it is plain which is
/// first and which is second — the first on one sheet meets the first on the
/// other.
pub fn pick_points(ui: &mut egui::Ui, texture: &egui::TextureHandle, size: [f64; 2], picks: &mut Vec<[f64; 2]>, width: f32, colour: Color32) -> egui::Response { // WHAT: a sheet to click points on. WHY: the two-point line-up.
    let tex = texture.size_vec2(); // WHAT: the picture's size. WHY: its proportions.
    let height = tex.y * width / tex.x.max(1.0); // WHAT: its height at this width. WHY: never stretched.
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click()); // WHAT: room for it, clickable. WHY: points are clicked.
    ui.painter().image(texture.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE); // WHAT: the sheet. WHY: what is clicked on.
    ui.painter().rect_stroke(rect, 0.0, egui::Stroke::new(1.0_f32, colour), egui::StrokeKind::Outside); // WHAT: a thin border in its colour. WHY: which sheet is which at a glance.
    if response.clicked() { // WHAT: a click on it. WHY: adds a point.
        if let Some(at) = response.interact_pointer_pos() { // WHAT: where the click was. WHY: turned into a point on the sheet.
            let p = [f64::from((at.x - rect.min.x) / rect.width()) * size[0], f64::from((at.y - rect.min.y) / rect.height()) * size[1]]; // WHAT: the click as a point on the sheet. WHY: the line-up works in sheet points.
            if picks.len() >= 2 { // WHAT: two already picked. WHY: a third click starts again.
                picks.clear(); // WHAT: starts the pair again. WHY: a third click means the first two were wrong.
            }
            picks.push(p); // WHAT: keeps it. WHY: one of the pair.
        }
    }
    for (n, p) in picks.iter().enumerate() { // WHAT: each picked point. WHY: drawn on the sheet.
        let at = egui::pos2(rect.min.x + (p[0] / size[0].max(1e-9)) as f32 * rect.width(), rect.min.y + (p[1] / size[1].max(1e-9)) as f32 * rect.height()); // WHAT: where it is on the picture. WHY: drawn there.
        ui.painter().circle_stroke(at, 6.0, egui::Stroke::new(2.0_f32, colour)); // WHAT: a ring round it. WHY: seen on any drawing.
        ui.painter().text(at + egui::vec2(8.0, -8.0), egui::Align2::LEFT_BOTTOM, (n + 1).to_string(), egui::FontId::proportional(13.0), colour); // WHAT: its number. WHY: first meets first.
    }
    response // WHAT: the sheet's response. WHY: the caller and the tests read where it went.
}

impl App {
    pub fn begin_overlay(&mut self) {
        let Some(doc) = self.doc() else {
            self.status = "Open one of the two sheets first.".into();
            return;
        };
        self.overlaying = Some(Overlaying::new(doc.path.clone(), doc.page, doc.pages.len())); // WHAT: the window, for the drawing and sheet in front. WHY: kept even if the tab changes.
    }

    pub fn overlay_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut overlaying) = self.overlaying.take() else {
            return;
        };
        let theme = self.chrome.theme;

        let mut keep = true;
        let mut go = false;
        let mut save = false; // WHAT: Save as PDF was pressed. WHY: handled after the window is drawn.
        let mut pick_older = false;
        let mut want_looks = false; // WHAT: the two sheets are wanted for picking points. WHY: asked of the engine after the window is drawn.
        let newer_name = overlaying.newer.file_name().unwrap_or_default().to_string_lossy().to_string(); // WHAT: the newer file's name. WHY: shown.

        egui::Window::new("Overlay Pages")
            .collapsible(false)
            .resizable(true)
            .default_width(760.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "Both issues printed over each other: red is the older, blue is \
                         the newer, dark grey is both.",
                    )
                    .color(theme.faint)
                    .size(11.0),
                );
                ui.add_space(10.0);

                ui.horizontal(|ui| { // WHAT: a row for the newer issue. WHY: name and sheet side by side.
                    ui.label(RichText::new("This issue").color(theme.faint).size(11.0)); // WHAT: the newer issue's label. WHY: so it is plain which is which.
                    ui.label(RichText::new(&newer_name).size(11.0)); // WHAT: its file. WHY: the same.
                    ui.add_space(10.0); // WHAT: a gap. WHY: the sheet number reads apart from the name.
                    ui.label(RichText::new("Sheet").color(theme.faint).size(11.0)); // WHAT: its sheet number. WHY: chosen here, not by whatever page is showing.
                    let mut n = overlaying.newer_page + 1; // WHAT: counted from one. WHY: as people count sheets.
                    if ui.add(egui::DragValue::new(&mut n).range(1..=overlaying.newer_sheets.max(1))).changed() { // WHAT: the newer sheet number changed. WHY: that sheet is laid on instead.
                        overlaying.newer_page = n.saturating_sub(1); // WHAT: the sheet chosen. WHY: laid over.
                        overlaying.sheets_changed(); // WHAT: forgets the old sheet's points and pictures. WHY: they belonged to it.
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("The other issue").color(theme.faint).size(11.0));
                    match &overlaying.older {
                        Some(path) => {
                            ui.label(
                                RichText::new(
                                    path.file_name().unwrap_or_default().to_string_lossy(),
                                )
                                .size(11.0),
                            );
                        }
                        None => {
                            ui.label(RichText::new("none yet").color(theme.faint).size(11.0));
                        }
                    }
                    if ui.small_button("Choose…").clicked() {
                        pick_older = true;
                    }
                    ui.add_space(10.0);
                    ui.label(RichText::new("Sheet").color(theme.faint).size(11.0));
                    let mut n = overlaying.older_page + 1;
                    if ui
                        .add(egui::DragValue::new(&mut n).range(1..=9999))
                        .changed()
                    {
                        overlaying.older_page = n.saturating_sub(1);
                        overlaying.sheets_changed(); // WHAT: forgets what was picked on the old sheet. WHY: it was on another sheet.
                    }
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Line them up").color(theme.faint).size(11.0)); // WHAT: the three ways. WHY: one for every pair of sheets.
                    ui.radio_value(&mut overlaying.line_up, LineUp::ByDrawing, "By the drawing").on_hover_text("Finds the shift, a quarter turn, a skewed scan or a half-size print from the line-work."); // WHAT: automatic. WHY: nearly always right.
                    ui.radio_value(&mut overlaying.line_up, LineUp::ByPoints, "By two points").on_hover_text("Click the same two places on each sheet: two grid crossings far apart work best."); // WHAT: by hand. WHY: when the drawing alone cannot do it.
                    ui.radio_value(&mut overlaying.line_up, LineUp::AsTheyAre, "As they are").on_hover_text("Lays them where the two files put them, without correcting anything."); // WHAT: uncorrected. WHY: to see them as issued.
                    ui.add_space(12.0);
                    ui.label(RichText::new("Line darkness").color(theme.faint).size(11.0)); // WHAT: the darkness setting. WHY: called what it does: how dark a line has to be.
                    ui.add(
                        egui::Slider::new(&mut overlaying.darkness, 0.3..=0.9).show_value(false),
                    );
                });

                if overlaying.line_up == LineUp::ByPoints { // WHAT: lining up by points. WHY: then the sheets are shown to click on.
                    ui.add_space(8.0); // WHAT: a gap. WHY: the sheets sit apart from the settings.
                    match overlaying.looks.clone() { // WHAT: whether the sheets are drawn yet. WHY: ask for them, or show them.
                        None => { // WHAT: not drawn yet. WHY: offer to draw them.
                            ui.horizontal(|ui| { // WHAT: a row for the button and spinner. WHY: side by side.
                                let ready = overlaying.older.is_some() && !overlaying.looking; // WHAT: both sheets known and not already asked. WHY: nothing to show otherwise.
                                if ui.add_enabled(ready, egui::Button::new("Show both sheets to pick on")).clicked() { // WHAT: asked to draw them. WHY: to pick on.
                                    want_looks = true; // WHAT: asks for them. WHY: points are picked on them.
                                }
                                if overlaying.looking { // WHAT: still drawing. WHY: a spinner says so.
                                    ui.spinner(); // WHAT: a spinner. WHY: they are being drawn.
                                }
                            });
                        }
                        Some(looks) => { // WHAT: drawn. WHY: shown side by side to click on.
                            ui.label(RichText::new(format!("Click the same two places on each sheet, in the same order. Picked: {} of 2 on the older, {} of 2 on the newer. A third click starts that sheet again.", overlaying.older_points.len(), overlaying.newer_points.len())).color(theme.faint).size(10.0)); // WHAT: what to do and how far along. WHY: nobody guesses.
                            let half = ((ui.available_width() - 12.0) / 2.0).max(150.0); // WHAT: half the window each. WHY: side by side.
                            ui.horizontal(|ui| { // WHAT: the two sheets side by side. WHY: the same places clicked on each.
                                ui.vertical(|ui| { // WHAT: the older sheet's column. WHY: its label above it.
                                    ui.label(RichText::new("Older (red)").size(10.0)); // WHAT: which sheet. WHY: the same.
                                    pick_points(ui, &looks.older, looks.older_size, &mut overlaying.older_points, half, Color32::from_rgb(0xC0, 0x39, 0x2B)); // WHAT: the older sheet to click on. WHY: its two points.
                                });
                                ui.vertical(|ui| { // WHAT: the newer sheet's column. WHY: the same.
                                    ui.label(RichText::new("This issue (blue)").size(10.0)); // WHAT: which sheet. WHY: the same.
                                    pick_points(ui, &looks.newer, looks.newer_size, &mut overlaying.newer_points, half, Color32::from_rgb(0x1F, 0x6F, 0xB2)); // WHAT: the newer sheet. WHY: its two points.
                                });
                            });
                        }
                    }
                }

                if let Some(said) = &overlaying.said {
                    ui.add_space(8.0);
                    ui.label(RichText::new(said).color(theme.faint).size(11.0));
                }
                if let Some(error) = &overlaying.error {
                    ui.add_space(8.0);
                    ui.colored_label(Color32::from_rgb(235, 120, 120), error);
                }

                if let Some(picture) = &overlaying.picture {
                    ui.add_space(8.0);
                    let width = ui.available_width().max(200.0);
                    let size = picture.size_vec2();
                    let scale = width / size.x;
                    ui.add(
                        egui::Image::new(picture)
                            .fit_to_exact_size(egui::vec2(width, size.y * scale)),
                    );
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let ready = overlaying.older.is_some() && !overlaying.running && overlaying.lining().is_some(); // WHAT: both sheets and a way to line them up. WHY: two points on each are needed for that way.
                    if ui.add_enabled(ready, egui::Button::new("Overlay")).clicked() {
                        go = true;
                    }
                    if ui.add_enabled(ready, egui::Button::new("Save as PDF…")).on_hover_text("Writes the overlay as a one-sheet drawing at full detail and opens it, to zoom, print or send.").clicked() { // WHAT: the Save as PDF button. WHY: the overlay at full size, to open, zoom and print.
                        save = true; // WHAT: Save as PDF pressed. WHY: handled below.
                    }
                    if ui.button("Close").clicked() {
                        keep = false;
                    }
                    if overlaying.running {
                        ui.add_space(8.0);
                        ui.spinner();
                    }
                });
            });

        if pick_older {
            self.filing.one(
                crate::files::Choose::open()
                    .filter("Drawing sets (PDF)", &["pdf"])
                    .title("The other issue"),
                |app, path| {
                    if let Some(overlaying) = app.overlaying.as_mut() {
                        overlaying.older = Some(path);
                        overlaying.sheets_changed(); // WHAT: forgets the last file's points and pictures. WHY: a new file.
                    }
                },
            );
        }
        if want_looks { // WHAT: the sheets were asked for. WHY: sent to the engine.
            if let Some(older) = overlaying.older.clone() {
                overlaying.looking = true; // WHAT: waiting. WHY: no second request.
                overlaying.error = None; // WHAT: clears an old error. WHY: a new try.
                self.compare_task += 1; // WHAT: a new request number. WHY: an older answer is dropped.
                self.svc.send(crate::render::ToWorker::OverlayLooks { job: self.compare_task, older, older_page: overlaying.older_page, newer: overlaying.newer.clone(), newer_page: overlaying.newer_page }); // WHAT: asks for both sheets drawn. WHY: to pick points on.
            }
        }
        if go { // WHAT: Overlay was pressed. WHY: sent to the engine.
            if let (Some(older), Some(lining)) = (overlaying.older.clone(), overlaying.lining()) { // WHAT: the other file and a line-up. WHY: both needed to start.
                overlaying.running = true;
                overlaying.error = None;
                self.compare_task += 1;
                let job = self.compare_task;
                self.svc.send(crate::render::ToWorker::Overlay {
                    job,
                    older,
                    older_page: overlaying.older_page,
                    newer: overlaying.newer.clone(), // WHAT: the newer issue chosen when the window opened. WHY: never whichever tab is in front.
                    newer_page: overlaying.newer_page,
                    darkness: overlaying.darkness,
                    lining, // WHAT: how to line them up. WHY: chosen in the window.
                });
            }
        }
        if save { // WHAT: Save as PDF was pressed. WHY: asks where, then writes.
            if let (Some(older), Some(lining)) = (overlaying.older.clone(), overlaying.lining()) { // WHAT: the other file and a line-up. WHY: both needed to write it.
                let suggested = crate::docops::beside(&overlaying.newer, "overlay"); // WHAT: a name beside the newer issue. WHY: where people look for it.
                let (newer, newer_page, older_page, darkness) = (overlaying.newer.clone(), overlaying.newer_page, overlaying.older_page, overlaying.darkness); // WHAT: what to lay over what. WHY: moved into the save step.
                self.filing.one(
                    crate::files::Choose::save(suggested.file_name().unwrap_or_default().to_string_lossy()).filter("Drawing sets (PDF)", &["pdf"]).title("Save the overlay"), // WHAT: asks where. WHY: a new file, never over either issue.
                    move |app, to| { // WHAT: once a place is chosen. WHY: the job is sent with it.
                        app.doc_task += 1; // WHAT: a new job number. WHY: its answer opens the file.
                        let op = crate::docops::Operation::OverlaySheet { newer, newer_page, older, older_page, darkness, lining, to }; // WHAT: the job. WHY: written at full detail by the engine.
                        app.status = "Writing the overlay…".into(); // WHAT: says so. WHY: a big sheet takes a moment.
                        app.svc.send(crate::render::ToWorker::Document { job: app.doc_task, op: Box::new(op) }); // WHAT: sends it. WHY: the same road as every Document menu job, which opens what it wrote.
                    },
                );
            }
        }
        if keep {
            self.overlaying = Some(overlaying);
        }
    }

    pub fn overlay_done(&mut self, job: u64, image: egui::ColorImage, said: String, ctx: &egui::Context) {
        if job != self.compare_task {
            return;
        }
        let picture = ctx.load_texture("hyperview-overlay", image, egui::TextureOptions::LINEAR);
        if let Some(overlaying) = self.overlaying.as_mut() {
            overlaying.running = false;
            overlaying.picture = Some(picture);
            overlaying.said = Some(said.clone());
        }
        self.status = said;
    }

    /// The two sheets arrived, drawn small, for picking points on.
    pub fn overlay_looks_done(&mut self, job: u64, older: (egui::ColorImage, [f64; 2]), newer: (egui::ColorImage, [f64; 2]), ctx: &egui::Context) { // WHAT: the two sheets arrived. WHY: shown to pick on.
        if job != self.compare_task { // WHAT: an answer to an older request. WHY: dropped.
            return; // WHAT: an old answer. WHY: the sheets have changed since.
        }
        let Some(overlaying) = self.overlaying.as_mut() else { return }; // WHAT: the window, if still open. WHY: closed means nobody wants them.
        overlaying.looking = false; // WHAT: no longer waiting. WHY: they are here.
        overlaying.looks = Some(Looks { // WHAT: keeps both sheets as pictures. WHY: shown to click on.
            older: ctx.load_texture("hyperview-overlay-older", older.0, egui::TextureOptions::LINEAR), // WHAT: the older sheet. WHY: shown to click on.
            older_size: older.1, // WHAT: its size. WHY: clicks become points.
            newer: ctx.load_texture("hyperview-overlay-newer", newer.0, egui::TextureOptions::LINEAR), // WHAT: the newer sheet. WHY: the same.
            newer_size: newer.1, // WHAT: its size. WHY: the same.
        });
    }
}

#[cfg(test)]
mod overlay_window_tests { // WHAT: the overlay window's tests. WHY: driven headless, as the program is.
    use super::*; // WHAT: the window's parts. WHY: tested headless.

    /// One pass of a window with nothing but a sheet to click on.
    fn a_pass(ctx: &egui::Context, events: Vec<egui::Event>, texture: &egui::TextureHandle, picks: &mut Vec<[f64; 2]>) -> egui::Rect { // WHAT: one frame of a window holding a sheet to click. WHY: a walkthrough with no screen.
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), events, ..Default::default() }; // WHAT: an 800 by 600 window and what the mouse did. WHY: as the program is driven.
        let mut at = egui::Rect::NOTHING; // WHAT: where the sheet was drawn. WHY: clicked on next.
        let _ = ctx.run(input, |ctx| { // WHAT: one frame. WHY: as the program draws one.
            egui::CentralPanel::default().show(ctx, |ui| { // WHAT: the window's middle. WHY: where the sheet is drawn.
                at = pick_points(ui, texture, [2592.0, 1728.0], picks, 400.0, Color32::RED).rect; // WHAT: an ARCH D sheet, 400 wide. WHY: the shape of the real thing.
            });
        });
        at // WHAT: where the sheet was drawn. WHY: the next click aims at it.
    }

    fn click(ctx: &egui::Context, at: egui::Pos2, texture: &egui::TextureHandle, picks: &mut Vec<[f64; 2]>) { // WHAT: a whole click at a place. WHY: press and let go, over two frames.
        let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }; // WHAT: the left button. WHY: a click.
        a_pass(ctx, vec![egui::Event::PointerMoved(at), button(true)], texture, picks); // WHAT: moves there and presses. WHY: half a click.
        a_pass(ctx, vec![button(false)], texture, picks); // WHAT: lets go. WHY: a click is counted on letting go.
    }

    #[test]
    fn two_clicks_on_a_sheet_become_two_points_on_it_and_a_third_starts_again() { // WHAT: the test. WHY: clicks must land on the sheet where they were made.
        let ctx = egui::Context::default(); // WHAT: a window with no screen. WHY: driven headless.
        let image = egui::ColorImage::new([300, 200], vec![Color32::WHITE; 300 * 200]); // WHAT: a stand-in sheet picture. WHY: its proportions are an ARCH D's.
        let texture = ctx.load_texture("sheet", image, egui::TextureOptions::LINEAR); // WHAT: as the window loads one. WHY: the same.
        let mut picks = Vec::new(); // WHAT: nothing picked. WHY: the start.
        let rect = a_pass(&ctx, vec![], &texture, &mut picks); // WHAT: lays it out. WHY: to know where to click.
        assert!((rect.width() - 400.0).abs() < 0.5 && (rect.height() - 400.0 * 200.0 / 300.0).abs() < 0.5, "never stretched: {rect:?}"); // WHAT: its own proportions. WHY: a stretched sheet puts points in the wrong place.
        click(&ctx, rect.min + egui::vec2(100.0, 50.0), &texture, &mut picks); // WHAT: a quarter across, three-eighths down. WHY: the first point.
        assert_eq!(picks.len(), 1, "one click, one point"); // WHAT: one. WHY: the test.
        assert!((picks[0][0] - 648.0).abs() < 3.0 && (picks[0][1] - 324.0).abs() < 3.0, "in the sheet's points: {:?}", picks[0]); // WHAT: a quarter of 2592 and three-eighths of 864's double. WHY: the click became a point on the sheet.
        click(&ctx, rect.min + egui::vec2(300.0, 200.0), &texture, &mut picks); // WHAT: the second point. WHY: a pair.
        assert_eq!(picks.len(), 2, "two clicks, two points"); // WHAT: two. WHY: the pair.
        click(&ctx, rect.min + egui::vec2(10.0, 10.0), &texture, &mut picks); // WHAT: a third. WHY: starts again.
        assert_eq!(picks.len(), 1, "a third click starts the pair again"); // WHAT: one. WHY: the first two were wrong.
        click(&ctx, egui::pos2(rect.max.x + 50.0, rect.max.y + 50.0), &texture, &mut picks); // WHAT: a click off the sheet. WHY: the edge case.
        assert_eq!(picks.len(), 1, "a click off the sheet picks nothing"); // WHAT: unchanged. WHY: not on the sheet.
    }

    #[test]
    fn the_window_waits_for_all_four_points_and_forgets_them_when_a_sheet_changes() { // WHAT: the test. WHY: Overlay needs all four points, and a new sheet clears them.
        let mut o = Overlaying::new(PathBuf::from("Rev 2.pdf"), 3, 10); // WHAT: a window on sheet 4 of ten. WHY: the starting state.
        assert_eq!((o.newer_page, o.older_page, o.newer_sheets), (3, 3, 10), "starts on the sheet in front"); // WHAT: the same sheet both sides. WHY: the sheet being looked at.
        assert_eq!(o.lining(), Some(crate::overlay::Lining::Auto), "lined up by the drawing to begin with"); // WHAT: automatic. WHY: nearly always right.
        o.line_up = LineUp::ByPoints; // WHAT: by points. WHY: the test.
        o.older_points = vec![[1.0, 2.0], [30.0, 40.0]]; // WHAT: two on the older. WHY: half of it.
        o.newer_points = vec![[5.0, 6.0]]; // WHAT: one on the newer. WHY: not yet enough.
        assert_eq!(o.lining(), None, "three points are not a line-up"); // WHAT: nothing yet. WHY: Overlay stays greyed.
        o.newer_points.push([50.0, 60.0]); // WHAT: the fourth. WHY: now complete.
        assert_eq!(o.lining(), Some(crate::overlay::Lining::Points { older: [[1.0, 2.0], [30.0, 40.0]], newer: [[5.0, 6.0], [50.0, 60.0]] })); // WHAT: the line-up. WHY: two on each.
        o.sheets_changed(); // WHAT: a sheet changed. WHY: the points were on the old one.
        assert!(o.older_points.is_empty() && o.newer_points.is_empty() && o.looks.is_none(), "forgotten"); // WHAT: all gone. WHY: they belonged to another sheet.
        o.line_up = LineUp::AsTheyAre; // WHAT: as they are. WHY: the third way.
        assert_eq!(o.lining(), Some(crate::overlay::Lining::AsTheyAre)); // WHAT: uncorrected. WHY: asked for.
    }
}
