//! Cut, copy and paste, and the history of what has been done.
//!
//! Markups are carried between sheets and between drawings as whole
//! annotations rather than as pictures of themselves, so a beam copied from
//! one sheet to another is still a beam: the same tool, the same weights, the
//! same subject. What changes on the way is the coordinates, which are put
//! through the sheet's own frame at both ends — a sheet turned 90 degrees and
//! a sheet whose page box does not start at zero are both ordinary, and a
//! paste that ignored either would land the markup off the paper.

use crate::app::App;

/// Markups taken off a sheet and waiting to go somewhere.
///
/// They are held in sheet space — top left origin, y down, already turned —
/// because that is the space both ends of a paste agree on whatever each
/// sheet's own page box and rotation happen to be.
#[derive(Clone, Default)]
pub struct Clipboard {
    pub marks: Vec<Carried>,
    /// Where the copy came from, so Paste in Place knows there is a place to
    /// put it back.
    pub from: Option<Where>,
    /// The words put on the operating system's clipboard with the copy, so a
    /// paste that brings them back is known to be this copy coming back.
    pub said: Option<String>,
}

#[derive(Clone)]
pub struct Carried {
    pub markup: annot::Markup,
    /// The markup's geometry in the sheet space of the sheet it came from.
    pub points: Vec<[f64; 2]>,
    pub strokes: Vec<Vec<[f64; 2]>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Where {
    pub doc: u64,
    pub page: u32,
}

/// What goes on the operating system's clipboard when a snapshot is taken.
const SNAPSHOT_SAID: &str = "A snapshot copied in Excalibur View";

/// The words to put on the operating system's clipboard for markups copied:
/// their own words, one markup a line, or a count when they have none.
pub fn said_for(marks: &[Carried]) -> String {
    let words: Vec<String> = marks.iter().map(|c| c.markup.contents()).filter(|w| !w.trim().is_empty()).collect(); // WHAT: what the markups say. WHY: pasted into an email, a copied note brings its words.
    if words.is_empty() {
        format!("{} markup{} copied in Excalibur View", marks.len(), if marks.len() == 1 { "" } else { "s" }) // WHAT: a count. WHY: shapes have no words, and the clipboard still needs something only this copy put there.
    } else {
        words.join("\n")
    }
}

impl Clipboard {
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// The box everything on the clipboard sits in, in sheet space.
    pub fn bounds(&self) -> Option<[f64; 4]> {
        let mut out: Option<[f64; 4]> = None;
        for carried in &self.marks {
            for p in carried
                .points
                .iter()
                .chain(carried.strokes.iter().flatten())
            {
                out = Some(match out {
                    None => [p[0], p[1], p[0], p[1]],
                    Some(b) => [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])],
                });
            }
        }
        out
    }
}

impl App {
    /// Puts the selection on the clipboard.
    pub fn copy_selection(&mut self) -> usize {
        let Some(doc) = self.doc() else { return 0 };
        let picked = doc.selection();
        if picked.is_empty() {
            self.status = "Nothing is selected.".into();
            return 0;
        }
        let frame = doc.frame();
        let mut carried = Vec::new();
        for index in &picked {
            let Some(mark) = doc.marks.get(*index) else { continue };
            let mut markup = mark.markup.clone();
            // A part copied with its group says whose it is by name, so the
            // paste can put the group back together, even for a group read
            // from a file that only said so by reference.
            let head = crate::groups::head_of(&doc.marks, *index);
            if head != *index && picked.contains(&head) {
                let name = doc.marks[head].markup.name();
                if !name.is_empty() {
                    markup.set(crate::groups::PART_OF, pdf::Object::text(&name));
                }
            }
            carried.push(Carried {
                markup,
                points: frame.points_to_sheet(&mark.markup.points()),
                strokes: mark
                    .markup
                    .ink()
                    .iter()
                    .map(|s| frame.points_to_sheet(s))
                    .collect(),
            });
        }
        let count = carried.len();
        let said = said_for(&carried); // WHAT: the words that go on the operating system's clipboard with the copy. WHY: Ctrl+V reads that clipboard, and seeing them come back is how it knows to put these markups down.
        self.clipboard = Clipboard {
            marks: carried,
            from: Some(Where {
                doc: doc.id,
                page: doc.page,
            }),
            said: Some(said.clone()),
        };
        self.system_copy = Some(said); // WHAT: put there at the end of the frame. WHY: as above; the markups' own words are also what another program gets if they are pasted there.
        self.status = format!(
            "{count} markup{} copied.",
            if count == 1 { "" } else { "s" }
        );
        count
    }

    /// Copies the selection and then takes it off the sheet.
    pub fn cut_selection(&mut self) {
        let count = self.copy_selection();
        if count == 0 {
            return;
        }
        let Some(doc) = self.doc_mut() else { return };
        let picked = doc.selection();
        doc.checkpoint_named(&format!(
            "Cut {} markup{}",
            picked.len(),
            if picked.len() == 1 { "" } else { "s" }
        ));
        for index in picked {
            if let Some(mark) = doc.marks.get_mut(index) {
                mark.gone = true;
            }
        }
        doc.choose(None);
        self.status = format!("{count} markup{} cut.", if count == 1 { "" } else { "s" });
    }

    /// Puts the clipboard down on this sheet.
    ///
    /// `in_place` puts it at the coordinates it was copied from, which is what
    /// somebody wants when they are carrying a title block note from sheet to
    /// sheet. Otherwise it lands in the middle of what they are looking at,
    /// which is what they want when they are carrying it across one drawing.
    pub fn paste(&mut self, in_place: bool) {
        if self.clipboard.is_empty() {
            self.status = "There is nothing to paste.".into();
            return;
        }
        let carried = self.clipboard.marks.clone();
        let bounds = self.clipboard.bounds();
        let canvas = self.last_canvas;
        let Some(doc) = self.doc_mut() else {
            self.status = "No drawing is open.".into();
            return;
        };
        let page = doc.page;
        let frame = doc.frame_of(page);

        // Where it goes, as a shift in sheet space.
        let shift = if in_place {
            [0.0, 0.0]
        } else {
            match bounds {
                Some(b) => {
                    let middle = doc.view.to_sheet(canvas.center());
                    [
                        middle[0] as f64 - (b[0] + b[2]) * 0.5,
                        middle[1] as f64 - (b[1] + b[3]) * 0.5,
                    ]
                }
                None => [0.0, 0.0],
            }
        };

        doc.checkpoint_named(&format!(
            "Paste {} markup{}",
            carried.len(),
            if carried.len() == 1 { "" } else { "s" }
        ));
        let first = doc.marks.len();
        // Every pasted markup gets a new name now rather than at the save, so
        // a group pasted whole points at its own new head and not the one it
        // was copied from.
        let renamed: std::collections::HashMap<String, String> = carried
            .iter()
            .map(|c| c.markup.name())
            .filter(|n| !n.is_empty())
            .map(|n| (n, annot::name::fresh()))
            .collect();
        for one in &carried {
            let mut markup = one.markup.clone();
            // A pasted markup is a new markup, not the same one in two places:
            // it gets no reference into the file until it is saved, and it
            // gets a name of its own so Revu does not treat the two as one.
            match renamed.get(&markup.name()) {
                Some(name) => {
                    markup.set_name(name);
                }
                None => {
                    markup.dict.remove("NM");
                }
            }
            let part_of = markup.dict.get(crate::groups::PART_OF).and_then(|o| o.as_text());
            if let Some(head) = part_of {
                match renamed.get(&head) {
                    Some(name) => {
                        markup.set(crate::groups::PART_OF, pdf::Object::text(name));
                    }
                    None => {
                        markup.dict.remove(crate::groups::PART_OF);
                    }
                }
            }
            let was = markup.drawn_box();
            markup.dict.remove("AP");
            markup.dict.remove("Popup");
            markup.dict.remove("IRT");
            markup.dict.remove("RT");

            let moved: Vec<[f64; 2]> = one
                .points
                .iter()
                .map(|p| [p[0] + shift[0], p[1] + shift[1]])
                .collect();
            let pdf_points = frame.points_to_pdf(&moved);
            match markup.subtype() {
                annot::Subtype::Ink => {
                    let strokes: Vec<Vec<[f64; 2]>> = one
                        .strokes
                        .iter()
                        .map(|s| {
                            frame.points_to_pdf(
                                &s.iter()
                                    .map(|p| [p[0] + shift[0], p[1] + shift[1]])
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .filter(|s| !s.is_empty())
                        .collect();
                    if strokes.is_empty() {
                        continue;
                    }
                    markup.set_ink(&strokes);
                }
                annot::Subtype::Line => {
                    if pdf_points.len() < 2 {
                        continue;
                    }
                    markup.set_line(pdf_points[0], pdf_points[pdf_points.len() - 1]);
                }
                annot::Subtype::Square
                | annot::Subtype::Circle
                | annot::Subtype::FreeText
                | annot::Subtype::Stamp
                | annot::Subtype::Highlight
                | annot::Subtype::Underline
                | annot::Subtype::StrikeOut
                | annot::Subtype::Squiggly
                | annot::Subtype::Text
                | annot::Subtype::Link => {
                    if pdf_points.len() < 2 {
                        continue;
                    }
                    // Round every corner, not the first and last: a box
                    // carries four, and its first and last share a side.
                    let placed = pdf_points.iter().fold(
                        [f64::MAX, f64::MAX, f64::MIN, f64::MIN],
                        |r, p| [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])],
                    );
                    markup.set_box(placed);
                    // A callout's leader goes with its box.
                    if let (Some(was), Some(cl)) = (was, markup.dict.get("CL").map(|o| o.numbers())) {
                        let d = [(placed[0] + placed[2] - was[0] - was[2]) / 2.0, (placed[1] + placed[3] - was[1] - was[3]) / 2.0];
                        let moved: Vec<pdf::Object> =
                            cl.iter().enumerate().map(|(i, v)| pdf::Object::real(v + d[i % 2])).collect();
                        markup.set("CL", pdf::Object::Array(moved));
                    }
                }
                _ => {
                    if pdf_points.len() < 2 {
                        continue;
                    }
                    markup.set_vertices(&pdf_points);
                }
            }

            // A measurement measures the sheet it is on, not the one it came
            // from. Pasting onto a sheet at a different scale re-reads it;
            // pasting onto one with no scale leaves it with no number, which
            // the Markups list then reports as left out.
            let kind = markup.kind();
            if kind.measures() && kind != annot::Kind::Count {
                match doc.scale_of(page) {
                    Some(scale) => {
                        markup.set("Measure", pdf::Object::Dict(scale.write()));
                        let caption = crate::sheet::caption_for(&markup, kind, &scale);
                        markup.set_contents(&caption);
                    }
                    None => {
                        markup.dict.remove("Measure");
                        markup.set_contents("");
                    }
                }
            }
            doc.marks.push(crate::sheet::Mark::new(page, markup));
        }

        let made = doc.marks.len() - first;
        doc.selected = (made > 0).then_some(first);
        doc.also = (first + 1..first + made).collect();
        doc.dirty = true;
        self.status = if made == 0 {
            "Nothing on the clipboard could be put on this sheet.".into()
        } else if in_place {
            format!(
                "{made} markup{} pasted at the same place on this sheet.",
                if made == 1 { "" } else { "s" }
            )
        } else {
            format!("{made} markup{} pasted.", if made == 1 { "" } else { "s" })
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_clipboard_has_no_bounds() {
        assert!(Clipboard::default().bounds().is_none());
        assert!(Clipboard::default().is_empty());
    }

    #[test]
    fn the_bounds_hold_everything_carried() {
        let mut markup = annot::Markup::new(annot::Subtype::Polygon);
        markup.set_vertices(&[[0.0, 0.0], [10.0, 10.0]]);
        let board = Clipboard {
            marks: vec![
                Carried {
                    markup: markup.clone(),
                    points: vec![[10.0, 20.0], [30.0, 40.0]],
                    strokes: Vec::new(),
                },
                Carried {
                    markup,
                    points: vec![[5.0, 60.0]],
                    strokes: vec![vec![[100.0, 8.0]]],
                },
            ],
            from: None,
            said: None,
        };
        assert_eq!(board.bounds(), Some([5.0, 8.0, 100.0, 60.0]));
    }
}

// ---- the list of what has been done ----------------------------------------

impl App {
    /// Everything done to this drawing since it was opened, and how far back
    /// it is still possible to go.
    ///
    /// Revu has this window and people use it to answer one question: "what
    /// did I just do?" So the list reads as a list of doings, newest first,
    /// and clicking one goes back to just before it.
    pub fn undo_history(&mut self, ctx: &egui::Context) {
        if !self.history_open {
            return;
        }
        let theme = self.chrome.theme;
        let mut keep = true;
        let mut go_back_to: Option<usize> = None;
        let mut forward = false;

        let (steps, redo, remembered) = match self.doc() {
            Some(doc) => (
                doc.undo.iter().map(|s| s.what.clone()).collect::<Vec<_>>(),
                doc.redo.len(),
                crate::sheet::REMEMBERED_STEPS,
            ),
            None => (Vec::new(), 0, crate::sheet::REMEMBERED_STEPS),
        };

        egui::Window::new("Undo History")
            .open(&mut keep)
            .collapsible(false)
            .resizable(true)
            .default_width(360.0)
            .default_height(400.0)
            .show(ctx, |ui| {
                if steps.is_empty() && redo == 0 {
                    ui.label(
                        egui::RichText::new(
                            "Nothing has been done to this drawing yet.",
                        )
                        .color(theme.faint),
                    );
                    return;
                }
                ui.label(
                    egui::RichText::new(format!(
                        "The last {remembered} things you do to a drawing are kept. \
                         Click one to go back to just before it."
                    ))
                    .color(theme.faint)
                    .size(11.0),
                );
                ui.add_space(8.0);

                if redo > 0 {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{redo} step{} taken back",
                                if redo == 1 { "" } else { "s" }
                            ))
                            .color(theme.faint)
                            .size(11.0),
                        );
                        if ui.small_button("Put one back").clicked() {
                            forward = true;
                        }
                    });
                    ui.add_space(6.0);
                }

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (at, what) in steps.iter().enumerate().rev() {
                            let row = ui.selectable_label(
                                false,
                                egui::RichText::new(format!("{}. {what}", at + 1)).size(12.0),
                            );
                            if row.clicked() {
                                go_back_to = Some(at);
                            }
                        }
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("0. Opened")
                                .color(theme.faint)
                                .size(12.0),
                        );
                    });
            });

        if forward {
            if let Some(doc) = self.doc_mut() {
                doc.redo();
            }
        }
        if let Some(step) = go_back_to {
            let went = self.doc_mut().map(|d| d.back_to(step)).unwrap_or(false);
            self.status = if went {
                format!("Gone back to step {step}.")
            } else {
                "That is where the drawing already is.".into()
            };
        }
        self.history_open = keep;
    }
}

// ---- carrying a look from one markup to another ----------------------------

/// Everything about how a markup looks, without its shape.
///
/// What the Format Painter carries. Not the geometry, not the subject, not
/// the measurement — only the look, because somebody using it wants this beam
/// to be the same colour and weight as that one, not to be that one.
#[derive(Clone)]
pub struct Style {
    pub colour: [f32; 3],
    pub interior: Option<[f32; 3]>,
    pub width: f64,
    pub opacity: f32,
    pub fill_opacity: f32,
    pub border: Option<pdf::Object>,
    pub effect: Option<pdf::Object>,
    pub endings: Option<pdf::Object>,
    pub font: Option<pdf::Object>,
}

impl Style {
    pub fn of(markup: &annot::Markup) -> Style {
        Style {
            colour: markup.colour(),
            interior: markup.interior(),
            width: markup.width(),
            opacity: markup.opacity(),
            fill_opacity: markup.fill_opacity(),
            border: markup.dict.get("BS").cloned(),
            effect: markup.dict.get("BE").cloned(),
            endings: markup.dict.get("LE").cloned(),
            font: markup.dict.get("DA").cloned(),
        }
    }

    /// Puts this look onto another markup, leaving its shape alone.
    pub fn onto(&self, markup: &mut annot::Markup) {
        markup.set_colour(self.colour);
        match self.interior {
            Some(fill) => {
                markup.dict.set(
                    "IC",
                    pdf::Object::Array(
                        fill.iter().map(|v| pdf::Object::real(*v as f64)).collect(),
                    ),
                );
            }
            None => {
                markup.dict.remove("IC");
            }
        }
        markup.set_width(self.width);
        markup.dict.set("CA", pdf::Object::real(self.opacity as f64));
        for (key, value) in [
            ("BS", &self.border),
            ("BE", &self.effect),
            ("LE", &self.endings),
            ("DA", &self.font),
        ] {
            match value {
                Some(v) => markup.dict.set(key, v.clone()),
                None => {
                    markup.dict.remove(key);
                }
            }
        }
        // The appearance was drawn for the old look, so it has to go and be
        // drawn again, or the markup keeps looking exactly as it did — unless
        // it is a picture that can only be kept.
        if !markup.keeps_its_appearance() {
            markup.dict.remove("AP");
        }
    }
}

impl App {
    /// Picks up the look of the selection, ready to brush onto the next markup
    /// clicked.
    pub fn pick_up_format(&mut self) {
        let style = self
            .doc()
            .and_then(|d| d.selected.and_then(|i| d.marks.get(i)))
            .map(|m| Style::of(&m.markup));
        match style {
            Some(style) => {
                self.painting_format = Some(style);
                self.status = "Look picked up. Click a markup to put it on, or press Escape."
                    .into();
            }
            None => {
                self.painting_format = None;
                self.status = "Select the markup whose look you want first.".into();
            }
        }
    }

    /// Puts the carried look onto one markup.
    pub fn brush_format_onto(&mut self, index: usize) -> bool {
        let Some(style) = self.painting_format.clone() else {
            return false;
        };
        let Some(doc) = self.doc_mut() else { return false };
        doc.checkpoint_named("Paint format");
        let Some(mark) = doc.marks.get_mut(index) else {
            return false;
        };
        style.onto(&mut mark.markup);
        mark.changed = true;
        doc.dirty = true;
        self.status = "The look was put on. Escape puts the brush down.".into();
        true
    }
}

// ---- a hyperlink's address -------------------------------------------------

impl App {
    /// Asks where the link goes, once the area has been dragged out.
    pub fn address_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut typed) = self.asking_address.take() else {
            return;
        };
        let theme = self.chrome.theme;
        let mut keep = true;
        let mut put_down = false;
        let mut give_up = false;

        egui::Window::new("Where does this go?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .max_width(660.0)
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                let box_ = ui.add(
                    egui::TextEdit::singleline(&mut typed)
                        .hint_text("https://…")
                        .desired_width(f32::INFINITY),
                );
                box_.request_focus();
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "A web address, or a file on the server everyone can reach. \
                         Whoever opens the drawing clicks the area you dragged out \
                         and it opens.",
                    )
                    .color(theme.faint)
                    .size(11.0),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let ready = !typed.trim().is_empty();
                    if ui
                        .add_enabled(ready, egui::Button::new("Put it down"))
                        .clicked()
                        || (ready && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        put_down = true;
                    }
                    if ui.button("Cancel").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        give_up = true;
                    }
                });
            });

        if give_up {
            if let Some(doc) = self.doc_mut() {
                doc.draft = None;
            }
            return;
        }
        if put_down {
            let address = typed.trim().to_string();
            if let Some(draft) = self.doc_mut().and_then(|d| d.draft.as_mut()) {
                draft.says = address.clone();
            }
            self.finish_draft();
            self.status = format!("That area now opens {address}.");
            return;
        }
        if keep {
            self.asking_address = Some(typed);
        } else if let Some(doc) = self.doc_mut() {
            doc.draft = None;
        }
        let _ = &mut keep;
    }
}

// ---- taking a picture of part of a sheet -----------------------------------

impl App {
    /// Takes a picture of the box just dragged out.
    ///
    /// The picture goes on the clipboard as a markup, so it can be pasted onto
    /// another sheet or another drawing straight away, and is written out
    /// beside the drawing so it can go into an email or a report. Taken from
    /// the file at print resolution rather than grabbed off the screen: a
    /// screen grab of a detail is unreadable the moment anybody prints it.
    pub fn take_snapshot(&mut self) {
        const DPI: u32 = 300;
        let Some(doc) = self.doc() else {
            return;
        };
        let Some(draft) = doc.draft.as_ref() else { return };
        if draft.points.len() < 2 {
            return;
        }
        let (a, b) = (draft.points[0], draft.points[draft.points.len() - 1]);
        let area = [
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[0].max(b[0]),
            a[1].max(b[1]),
        ];
        if area[2] - area[0] < 2.0 || area[3] - area[1] < 2.0 {
            if let Some(doc) = self.doc_mut() {
                doc.draft = None;
            }
            return;
        }
        let path = doc.path.clone();
        let page = doc.page;
        let library = self.svc.library.clone();
        if let Some(doc) = self.doc_mut() {
            doc.draft = None;
        }

        let image = match crate::render::region_of(library, &path, page, area, DPI) {
            Ok(image) => image,
            Err(why) => {
                self.error = Some(why);
                return;
            }
        };
        let picture = crate::picture::from_canvas(&image);

        // On the clipboard as a markup, ready to go down on any sheet at the
        // size it was taken at.
        let mut markup = annot::Markup::new(annot::Subtype::Stamp);
        markup.set_subject("Snapshot");
        markup.picture = Some(picture);
        markup.set_box([area[0], area[1], area[2], area[3]]);
        self.clipboard = Clipboard {
            marks: vec![Carried {
                markup,
                points: vec![[area[0], area[1]], [area[2], area[3]]],
                strokes: Vec::new(),
            }],
            from: Some(Where { doc: doc_id(self), page }),
            said: Some(SNAPSHOT_SAID.into()),
        };
        self.system_copy = Some(SNAPSHOT_SAID.into()); // WHAT: the snapshot's words on the operating system's clipboard. WHY: so Ctrl+V knows the snapshot is the last thing copied.

        // And beside the drawing, so it can go into an email.
        let beside = crate::docops::beside(&path, "snapshot");
        let beside = beside.with_extension("png");
        let beside = crate::docops::free_name(&beside);
        match write_png(&image, &beside) {
            Ok(()) => {
                self.status = format!(
                    "Snapshot taken. Paste puts it on any sheet, and it is saved as {}.",
                    beside
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                );
            }
            Err(why) => {
                // The clipboard copy still worked, and saying so beats
                // reporting a failure for the half that did not.
                self.status =
                    format!("Snapshot taken and ready to paste. It could not be saved: {why}");
            }
        }
    }
}

fn doc_id(app: &App) -> u64 {
    app.doc().map(|d| d.id).unwrap_or(0)
}

/// Writes a picture out as a PNG.
fn write_png(image: &egui::ColorImage, to: &std::path::Path) -> Result<(), String> {
    let (width, height) = (image.width() as u32, image.height() as u32);
    let mut flat: Vec<u8> = Vec::with_capacity((width * height * 4) as usize);
    for pixel in image.pixels.iter() {
        flat.extend_from_slice(&pixel.to_array());
    }
    let buffer: image::RgbaImage = image::ImageBuffer::from_raw(width, height, flat)
        .ok_or_else(|| "the picture came out the wrong size".to_string())?;
    buffer.save(to).map_err(|e| e.to_string())
}

// ---- saving everything, and sending it on ----------------------------------

impl App {
    /// Saves every drawing that has anything unsaved in it.
    pub fn save_all(&mut self) {
        let author = self.prefs.author.clone();
        let mut saved = 0usize;
        let mut written = 0usize;
        let mut trouble: Vec<String> = Vec::new();
        for at in 0..self.docs.len() {
            let dirty = self.docs[at].dirty;
            if !dirty {
                continue;
            }
            let name = self.docs[at]
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            match self.docs[at].save(&author) {
                Ok(count) => {
                    saved += 1;
                    written += count;
                }
                // One that cannot be saved must not stop the rest, and it has
                // to be named — a "some of them failed" is a message nobody
                // can act on.
                Err(why) => trouble.push(format!("{name}: {why}")),
            }
        }
        self.status = match (saved, trouble.len()) {
            (0, 0) => "Everything is already saved.".into(),
            (n, 0) => format!(
                "{n} drawing{} saved, {written} markup{} written.",
                if n == 1 { "" } else { "s" },
                if written == 1 { "" } else { "s" }
            ),
            (n, _) => format!(
                "{n} saved. {} could not be: {}",
                trouble.len(),
                trouble.join("; ")
            ),
        };
        if !trouble.is_empty() {
            self.error = Some(self.status.clone());
        }
    }

    /// Hands the drawing to whatever this machine sends mail with.
    ///
    /// A `mailto:` with the file named in the body rather than attached,
    /// because no mail program on any platform accepts an attachment from a
    /// link — and a program that claimed to have attached it, and had not,
    /// would be worse than one that says where the file is.
    pub fn email_this_set(&mut self) {
        let Some(doc) = self.doc() else {
            self.status = "Open a drawing first.".into();
            return;
        };
        if doc.dirty {
            self.status =
                "Save it first — what would go out is the file on disk, not what is on \
                 the screen."
                    .into();
            return;
        }
        let path = doc.path.clone();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // A tablet can attach it for real: the share sheet hands the file to
        // Mail, or to anything else that sends files.
        if crate::platform::can_share() {
            crate::platform::share(&path);
            self.status = format!("Sending {name}…");
            return;
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        let subject = urlencode(&name);
        let body = urlencode(&format!(
            "{name} is attached.\n\n{}\n\nSent from Excalibur View.",
            path.display()
        ));
        let link = format!("mailto:?subject={subject}&body={body}");
        match open_it(&link) {
            Ok(()) => {
                self.status = format!(
                    "A message is open with the file named in it. Attach {name} \
                     ({}) from {} — no mail program accepts an attachment from a link, \
                     so this program does not pretend to have added one.",
                    megabytes(size),
                    path.parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                );
            }
            Err(why) => {
                self.error = Some(format!(
                    "This machine has nothing set up to send mail with: {why}. The file \
                     is at {}.",
                    path.display()
                ));
            }
        }
    }
}

fn megabytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{} KB", (bytes / 1024).max(1))
    }
}

/// Everything a `mailto:` link cannot carry as it stands.
fn urlencode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Hands something to whatever the machine opens that kind of thing with.
fn open_it(what: &str) -> Result<(), String> {
    if crate::platform::open_url(what) {
        return Ok(());
    }
    #[cfg(windows)]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", what]);
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg(what);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(what);
        c
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod mail_tests {
    use super::*;

    #[test]
    fn a_file_name_survives_being_put_in_a_link() {
        // Spaces, ampersands and quotes in a drawing's name are ordinary, and
        // every one of them would cut a mailto link short.
        assert_eq!(urlencode("S-201 Rev & Final.pdf"), "S-201%20Rev%20%26%20Final.pdf");
        assert_eq!(urlencode("plain"), "plain");
        assert_eq!(urlencode("a\nb"), "a%0Ab");
    }
}
