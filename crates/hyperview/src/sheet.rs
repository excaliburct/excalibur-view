//! What the program holds while a drawing set is open.
//!
//! Markups here are **the annotations that will be in the file** — the same
//! dictionaries, in PDF space, carrying whatever the Tool Chest tool carried.
//! There is no separate internal format that has to be converted on the way
//! out, because that conversion is where fidelity goes to die.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use annot::measure::Measure;
use annot::{Frame, Kind, Markup};
use egui::TextureHandle;
use pdf::Ref;

use crate::render::{PageSize, SheetLabel, TileKey};
use crate::view::{Tiles, View};

/// One markup, and what has happened to it since the file was opened.
#[derive(Clone)]
pub struct Mark {
    pub page: u32,
    /// Where it lives in the file. `None` means it has never been saved.
    pub reference: Option<Ref>,
    pub markup: Markup,
    pub gone: bool,
    pub changed: bool,
}

impl Mark {
    pub fn new(page: u32, markup: Markup) -> Mark {
        Mark {
            page,
            reference: None,
            markup,
            gone: false,
            changed: true,
        }
    }

    pub fn from_file(page: u32, reference: Ref, markup: Markup) -> Mark {
        Mark {
            page,
            reference: Some(reference),
            markup,
            gone: false,
            changed: false,
        }
    }

    pub fn kind(&self) -> Kind {
        self.markup.kind()
    }

    pub fn subject(&self) -> String {
        self.markup.subject()
    }

    /// The geometry, in the sheet space the screen works in.
    pub fn on_sheet(&self, frame: &Frame) -> Vec<[f64; 2]> {
        frame.points_to_sheet(&self.markup.points())
    }
}

/// A markup being drawn. Its points are kept in sheet space while the user is
/// putting them down, because that is the space the snapping, the preview and
/// the running total all work in. It becomes a real annotation on release.
#[derive(Clone, Debug)]
pub struct Draft {
    pub tool: crate::app::Tool,
    pub points: Vec<[f64; 2]>,
    pub strokes: Vec<Vec<[f64; 2]>>,
    pub subject: String,
    pub colour: [u8; 4],
    /// The Tool Chest tool this came from, carrying its weights and styling.
    pub template: Option<pdf::Dict>,
    /// What the markup says: the words in a text box, the address behind a
    /// hyperlink, the name of a space. Empty for a shape that only has a shape.
    pub says: String,
    /// A depth, for the one measurement that needs one. `None` means nobody
    /// has typed one, and a volume with no depth is reported rather than
    /// guessed at.
    pub depth: Option<f64>,
    /// The picture this markup shows, for an image or a stamp.
    pub picture: Option<annot::markup::Picture>,
    /// What the toolbar says the next markup should look like. A Tool Chest
    /// tool brings its own look and keeps it.
    pub pen: Option<crate::pen::Pen>,
}

impl Default for Draft {
    fn default() -> Draft {
        Draft {
            tool: crate::app::Tool::Select,
            points: Vec::new(),
            strokes: Vec::new(),
            subject: String::new(),
            colour: [220, 40, 40, 255],
            template: None,
            says: String::new(),
            depth: None,
            picture: None,
            pen: None,
        }
    }
}

impl Draft {
    /// A draft of one tool, with everything else left as it comes.
    pub fn of(tool: crate::app::Tool) -> Draft {
        Draft {
            tool,
            ..Draft::default()
        }
    }

    pub fn last(&self) -> Option<[f64; 2]> {
        self.points.last().copied()
    }

    /// Turns the draft into the annotation that will go in the file.
    pub fn into_markup(mut self, frame: &Frame, scale: Option<&Measure>) -> Option<Markup> {
        use annot::Subtype;
        use crate::app::Tool;
        let (mut subtype, mut intent) = self.tool.shape()?;

        // An arc is clicked as two ends and a point on the curve, and written
        // as the curve itself. A PDF has no arc annotation, so the curve is
        // the points — which is also what makes it measure right and what
        // lets Revu open it.
        if self.tool == Tool::Arc && self.points.len() >= 3 {
            self.points = arc_through(self.points[0], self.points[2], self.points[1]);
        }
        // Three points on a circle are enough to say which circle it is. The
        // markup carries the circle, not the three clicks, so a reader that
        // knows nothing about how it was drawn still shows the right thing.
        if matches!(self.tool, Tool::Diameter | Tool::Radius) && self.points.len() >= 3 {
            if let Some((centre, radius)) =
                circle_through(self.points[0], self.points[1], self.points[2])
            {
                self.points = vec![
                    [centre[0] - radius, centre[1] - radius],
                    [centre[0] + radius, centre[1] + radius],
                ];
            }
        }

        // A Tool Chest tool knows what it is better than the toolbar does. His
        // W12x26 is a length tool, his slab is an area tool, and picking one
        // out of the chest should not quietly turn it into something else.
        if let Some(dict) = &self.template {
            let from_tool = Subtype::read(dict);
            let its_intent = dict
                .get("IT")
                .and_then(|o| o.as_name())
                .map(|n| n.as_str().to_string());
            if let Some(name) = its_intent {
                if let Some(known) = known_intent(&name) {
                    subtype = from_tool;
                    intent = Some(known);
                }
            }
        }

        // A length is a Line when it has two ends and a PolyLine when it has
        // more. Revu draws the same distinction, and without it a run clicked
        // around four corners comes back as a straight line between the first
        // and the last — measuring short, silently.
        if matches!(intent, Some("LineDimension") | Some("PolyLineDimension")) {
            if self.points.len() > 2 {
                subtype = Subtype::PolyLine;
                intent = Some("PolyLineDimension");
            } else {
                subtype = Subtype::Line;
                intent = Some("LineDimension");
            }
        }

        let mut markup = match &self.template {
            Some(dict) => Markup::from_tool(dict),
            None => Markup::new(subtype),
        };
        markup.set("Subtype", pdf::Object::name(subtype.as_str()));
        match intent {
            Some(intent) => {
                markup.set("IT", pdf::Object::name(intent));
            }
            None => {
                // A Tool Chest markup keeps the intent it came with, like
                // Revu's PolygonCloud, so it goes back to Revu as the same
                // tool. A measuring intent it has no business carrying goes.
                let its_own = self
                    .template
                    .as_ref()
                    .and_then(|d| d.get("IT"))
                    .and_then(|o| o.as_name())
                    .is_some_and(|n| known_intent(n.as_str()).is_none());
                if !its_own {
                    markup.dict.remove("IT");
                }
                markup.dict.remove("MeasurementTypes");
            }
        }
        if self.template.is_none() {
            match &self.pen {
                // The properties toolbar decides what a markup drawn by hand
                // looks like: its colour, its weight, its line style, its
                // arrowheads, how see-through it is, and how its words are set.
                Some(pen) => pen.onto(&mut markup),
                None => {
                    markup.set_colour([
                        self.colour[0] as f32 / 255.0,
                        self.colour[1] as f32 / 255.0,
                        self.colour[2] as f32 / 255.0,
                    ]);
                    markup.set_width(2.0);
                }
            }
        }
        if !self.subject.is_empty() {
            markup.set_subject(&self.subject);
        }

        let points = frame.points_to_pdf(&self.points);

        // A form field is not a shape somebody drew: it is a box with a name
        // and a kind, and it has to end up in the document's own list of
        // fields as well as on the page.
        if let Some(field) = crate::forms::Field::of_tool(self.tool) {
            if points.len() < 2 {
                return None;
            }
            let (a, b) = (points[0], points[points.len() - 1]);
            let area = [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[0].max(b[0]),
                a[1].max(b[1]),
            ];
            let pen = self.pen.clone().unwrap_or_default();
            let name = if self.says.is_empty() {
                field.prefix().to_string()
            } else {
                self.says.clone()
            };
            return Some(crate::forms::make(field, &name, area, &pen));
        }

        match subtype {
            Subtype::Line => {
                if points.len() < 2 {
                    return None;
                }
                markup.set_line(points[0], points[points.len() - 1]);
            }
            Subtype::Ink => {
                let strokes: Vec<Vec<[f64; 2]>> = self
                    .strokes
                    .iter()
                    .map(|s| frame.points_to_pdf(s))
                    .filter(|s| !s.is_empty())
                    .collect();
                if strokes.is_empty() {
                    return None;
                }
                markup.set_ink(&strokes);
            }
            Subtype::Square | Subtype::Circle | Subtype::FreeText | Subtype::Stamp
            | Subtype::Link => {
                if points.len() < 2 {
                    return None;
                }
                let (a, b) = (points[0], points[points.len() - 1]);
                markup.set_box([
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[0].max(b[0]),
                    a[1].max(b[1]),
                ]);
            }
            Subtype::Text => {
                // A folded note is a marker of a fixed size wherever it is put,
                // because it is a place to hang a comment rather than a shape.
                let at = *points.first()?;
                markup.set_box([at[0], at[1] - 20.0, at[0] + 20.0, at[1]]);
                markup.set(
                    "Name",
                    pdf::Object::name(if self.tool == Tool::Flag {
                        "Paragraph"
                    } else {
                        "Comment"
                    }),
                );
                markup.set("Open", pdf::Object::Bool(false));
            }
            Subtype::Highlight | Subtype::Underline | Subtype::StrikeOut | Subtype::Squiggly => {
                if points.len() < 2 {
                    return None;
                }
                let (a, b) = (points[0], points[points.len() - 1]);
                let area = [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[0].max(b[0]),
                    a[1].max(b[1]),
                ];
                markup.set_box(area);
                // The quad points are what a reader marks the words by. One
                // quad for the whole run is right for a drawing, where the
                // words are a title block rather than a paragraph.
                markup.set(
                    "QuadPoints",
                    pdf::Object::Array(
                        [
                            area[0], area[3], area[2], area[3], area[0], area[1], area[2],
                            area[1],
                        ]
                        .iter()
                        .map(|v| pdf::Object::real(*v))
                        .collect(),
                    ),
                );
            }
            _ => {
                if points.len() < 2 {
                    return None;
                }
                markup.set_vertices(&points);
            }
        }

        // A cloud is an ordinary shape carrying a border effect that says to
        // draw its edge cloudy. Revu writes it that way and so does Acrobat,
        // so a cloud drawn here is a cloud wherever the set goes.
        if self.tool.is_cloudy() && !markup.dict.has("BE") {
            let mut effect = pdf::Dict::new();
            effect.set("S", pdf::Object::name("C"));
            effect.set("I", pdf::Object::Real(2.0));
            markup.dict.set("BE", pdf::Object::Dict(effect));
        }

        // A callout points at what it is about. The first click is the thing,
        // the last is where the words go, and the box is only the last part.
        //
        // The box goes out from the second click, away from what it points
        // at, with the leader meeting the middle of its near side. It starts
        // one line high and grows as the words are typed.
        if self.tool == Tool::Callout && points.len() >= 2 {
            let tip = points[0];
            let box_at = points[points.len() - 1];
            let (w, h) = (96.0, 20.0);
            let (x0, x1) = if box_at[0] >= tip[0] {
                (box_at[0], box_at[0] + w)
            } else {
                (box_at[0] - w, box_at[0])
            };
            markup.set_box([x0, box_at[1] - h * 0.5, x1, box_at[1] + h * 0.5]);
            let leader: Vec<pdf::Object> = points[..points.len() - 1]
                .iter()
                .chain(std::iter::once(&box_at))
                .flat_map(|p| [pdf::Object::real(p[0]), pdf::Object::real(p[1])])
                .collect();
            markup.set("CL", pdf::Object::Array(leader));
            markup.set("IT", pdf::Object::name("FreeTextCallout"));
        }

        // Typewriter is a text box with nothing drawn round it — which in a
        // PDF means no border and no fill, not a different kind of annotation.
        if self.tool == Tool::Typewriter {
            markup.set_width(0.0);
            markup.dict.remove("IC");
        }

        // A dimension line carries an arrow at each end, so it reads as a
        // dimension rather than as a line somebody happened to draw.
        if self.tool == Tool::Dimension {
            markup.set(
                "LE",
                pdf::Object::Array(vec![
                    pdf::Object::name("OpenArrow"),
                    pdf::Object::name("OpenArrow"),
                ]),
            );
            markup.set("IT", pdf::Object::name("LineDimension"));
        }

        // A redaction is a box that will be blacked out. Until it is applied
        // it is only a mark, and it says so.
        if self.tool == Tool::Redaction {
            markup.set_subject("Redaction");
            markup.set("IC", pdf::Object::Array(vec![
                pdf::Object::real(0.0),
                pdf::Object::real(0.0),
                pdf::Object::real(0.0),
            ]));
        }

        // A space is a named region rather than a drawing. It is filled
        // faintly so it can be seen without hiding what is under it.
        if self.tool == Tool::Space {
            markup.set("IT", pdf::Object::name("Space"));
            markup.set("CA", pdf::Object::real(0.25));
        }

        if let Some(picture) = self.picture.take() {
            markup.picture = Some(picture);
        }

        if !self.says.is_empty() {
            match self.tool {
                Tool::Hyperlink => {
                    let mut action = pdf::Dict::new();
                    action.set("S", pdf::Object::name("URI"));
                    action.set("URI", pdf::Object::text(&self.says));
                    markup.set("A", pdf::Object::Dict(action));
                    markup.set_contents(&self.says);
                }
                _ => {
                    markup.set_contents(&self.says);
                }
            }
        }

        let kind = markup.kind();
        if kind.measures() && kind != Kind::Count {
            match scale {
                Some(scale) => {
                    markup.set("Measure", pdf::Object::Dict(scale.write()));
                    let caption = caption_for(&markup, kind, scale);
                    markup.set_contents(&caption);
                }
                None => {
                    // No scale means no number. Saying nothing beats saying
                    // something wrong.
                    markup.dict.remove("Measure");
                    markup.set_contents("");
                }
            }
        } else if kind == Kind::Count {
            markup.set_contents("1");
        }
        Some(markup)
    }
}

/// One step in the Undo History: what was about to happen, and where the
/// markups stood before it did.
#[derive(Clone)]
pub struct Step {
    pub what: String,
    pub marks: Vec<Mark>,
    /// A change to a sheet's own drawing — its words, changed with Edit
    /// Text — that stepping back or forward puts back or does again.
    pub file: Option<PageSwap>,
    /// A change to which sheets are in the set and in what order, which
    /// stepping back or forward writes back or does again.
    pub sheets: Option<crate::reorder::Sheets>,
}

/// A sheet's drawing before and after a change to it. The page points at its
/// drawing through `/Contents` and at its fonts through `/Resources`; the old
/// ones are still in the file, because every save only adds, so going back is
/// pointing the page at them again.
#[derive(Clone, Debug)]
pub struct PageSwap {
    pub page: Ref,
    pub before: Drawing,
    pub after: Drawing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drawing {
    pub contents: Option<pdf::Object>,
    pub resources: Option<pdf::Object>,
    /// Everything else a change wrote over under its own number, as it was
    /// in this state: the content stream it rewrote, a font list it added to.
    /// Pointing the sheet back at the old numbers isn't enough when the old
    /// numbers now hold the new drawing.
    pub objects: std::rc::Rc<Vec<(pdf::Ref, pdf::Object)>>,
}

impl Drawing {
    pub fn of(page: &pdf::Dict) -> Drawing {
        Drawing {
            contents: page.get("Contents").cloned(),
            resources: page.get("Resources").cloned(),
            objects: Default::default(),
        }
    }

    /// A sheet's drawing before and after a change that was saved on top of
    /// `old` to make `new`.
    pub fn either_side(old: &pdf::Document, new: &pdf::Document, page: pdf::Ref) -> Option<(Drawing, Drawing)> {
        use pdf::xref::Slot;
        let mut before = Drawing::of(old.get(page).as_dict()?);
        let mut after = Drawing::of(new.get(page).as_dict()?);
        let mut numbers: Vec<(u32, u16)> = new
            .xref
            .slots
            .iter()
            .filter(|(n, _)| **n != page.number)
            .filter_map(|(n, slot)| match (old.xref.slots.get(n), slot) {
                (_, Slot::Free) | (None, _) | (Some(Slot::Free), _) => None,
                (Some(was), now) if was == now => None,
                (Some(Slot::InFile { generation, .. }), _) => Some((*n, *generation)),
                (Some(_), _) => Some((*n, 0)),
            })
            .collect();
        numbers.sort();
        let mut was = Vec::new();
        let mut is = Vec::new();
        for (number, generation) in numbers {
            let r = pdf::Ref::new(number, generation);
            was.push((r, (*old.get(r)).clone()));
            is.push((r, (*new.get(r)).clone()));
        }
        before.objects = std::rc::Rc::new(was);
        after.objects = std::rc::Rc::new(is);
        Some((before, after))
    }
}

/// How far back the history goes. Deep enough for a morning's work, shallow
/// enough that a set with thousands of markups does not fill the machine.
pub const REMEMBERED_STEPS: usize = 64;

pub struct Doc {
    /// Which tab this is, and how the render worker knows which drawing a
    /// tile belongs to.
    pub id: u64,
    pub path: PathBuf,
    /// The file at object level. Saving appends to this.
    pub file: pdf::Document,
    pub pages: Vec<PageSize>,
    pub frames: Vec<Frame>,
    pub labels: Vec<SheetLabel>,
    pub page: u32,
    pub view: View,
    pub tiles: Tiles,
    pub previews: HashMap<u32, (f32, TextureHandle)>,
    /// Each sheet's line-work, for snapping onto, once it has been read.
    pub geometry: HashMap<u32, std::sync::Arc<takeoff::snap::Geometry>>,
    /// Sheets whose line-work has been asked for.
    pub geometry_asked: HashSet<u32>,
    pub thumbs: HashMap<u32, TextureHandle>,
    pub asked: HashSet<u32>,
    /// Sheets picked in the Thumbnails panel, to print or save just those.
    pub picks: crate::picks::Picks,
    /// Sheets whose markups were saved since the drawing was last read for
    /// drawing, so the picture of them is out of date.
    pub redraw: std::collections::BTreeSet<u32>,
    /// Sheets being read again for that, and the reading tiles have to come
    /// from to be up to date.
    pub redrawing: std::collections::BTreeSet<u32>,
    pub fresh_from: u64,
    /// The paragraphs of words on each sheet, once read, for Edit Text.
    pub paragraphs: HashMap<u32, Vec<crate::textedit::Found>>,
    pub paragraphs_asked: HashSet<u32>,
    pub marks: Vec<Mark>,
    /// Scales the user has set but not yet saved. `None` means cleared.
    pub scales: BTreeMap<u32, Option<Measure>>,
    /// Scales the user has set on a drawing from the office server, not yet
    /// sent to it. `None` means cleared.
    pub scales_to_send: BTreeMap<u32, Option<Measure>>, // WHAT: each sheet's scale waiting to go up. WHY: a scale kept only in this seat's downloaded copy was lost on the next open, and the takeoff on the server never had it (Mesa, 2026-11-24)
    pub draft: Option<Draft>,
    pub selected: Option<usize>,
    /// The rest of the selection, when more than one markup is held at once.
    /// `selected` stays the one whose properties are shown, because a
    /// properties panel has to be about something in particular.
    pub also: Vec<usize>,
    pub undo: Vec<Step>,
    /// Steps taken back, ready to be put forward again. Doing anything new
    /// clears it, because there is no longer a forward to go to.
    pub redo: Vec<Step>,
    pub dirty: bool,
    pub filter: String,
    pub sent: Vec<TileKey>,
    pub open_millis: u128,
    pub labelled: u32,
    /// How much of each markup gets drawn into the file when it is saved.
    pub drawing: annot::appearance::Draw,
    /// The file cannot be written. Known at open time, not at save time.
    pub read_only: bool,
    /// The lock on the file, when it has one and a password opened it.
    ///
    /// A locked set is worked on like any other: markups saved into one are
    /// locked on the way in with the same key, so the file stays openable by
    /// the same password and by nothing else.
    pub locked: Option<pdf::opening::Lock>,
    /// The file on disk as this copy last read or wrote it. Anything else at
    /// save time means somebody else saved it in the meantime.
    pub on_disk: Option<Stamp>,
    /// Where this came from on a server, when it came from one.
    pub attached: Option<crate::sync::Attached>,
    /// How the window is split, if it is.
    pub split: Split,
    /// How the sheets are arranged in the window.
    pub layout: crate::layout::Layout,
    /// The other pane, when split. The pane being worked in is always `page`
    /// and `view` above, and switching panes swaps this with those — which
    /// means every tool, every measurement and every bit of painting goes on
    /// working on "the current pane" without knowing a split exists.
    pub other: Option<Pane>,
    /// Which side the pane being worked in is on: 0 is left or top.
    pub side: u8,
    /// Move both panes together. Off by default: the usual reason to split is
    /// to look at two different places at once.
    pub sync_panes: bool,
    /// The sheets were moved, added or taken out, and the renderer has not
    /// read the new order yet: what it sends back meanwhile is for the old
    /// order and is turned away.
    pub reshuffled: bool,
    /// The renderer still has to be told the sheets changed.
    pub sheets_changed: bool,
    /// Sheet numbers have to be read again once the renderer has the new order.
    pub relabel: bool,
    /// Who the last save was made as, for the saves Undo makes on its own.
    pub saved_by: String,
    /// Times the renderer has been asked to read the file again and has not
    /// yet said it has.
    pub refreshes_owed: u32,
    /// The password the file was opened with, empty for most. Kept in memory
    /// only, so the file can be read again after a save.
    pub password: String,
}

/// How the sheet area is divided.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Split {
    #[default]
    None,
    /// Side by side.
    Vertical,
    /// One above the other.
    Horizontal,
}

impl Split {
    pub fn on(self) -> bool {
        self != Split::None
    }
}

/// One half of a split: which sheet it is showing, and where it is looking.
#[derive(Clone, Debug)]
pub struct Pane {
    pub page: u32,
    pub view: View,
}

/// Why a drawing would not open.
pub enum Shut {
    /// It is locked, and the password typed was not the right one — or none
    /// was typed yet. Carries the file's name, for the box that asks.
    WantsPassword(String),
    /// Anything else, in words somebody can act on.
    Trouble(String),
}

impl Doc {
    /// Opens a drawing set at object level: the pages, their frames and every
    /// markup already in the file. No screen state, so this is the path a test
    /// or a batch job takes as well as the viewer.
    /// Why this drawing cannot be written to, in a sentence somebody can act
    /// on. Only meaningful when `read_only`.
    pub fn why_read_only(&self) -> String {
        "This file cannot be written to. Save a copy somewhere you can write \
         and change that."
            .into()
    }

    pub fn open(path: PathBuf) -> Result<Doc, String> {
        Doc::open_with(path, "").map_err(|shut| match shut {
            Shut::WantsPassword(name) => format!("{name} is locked. It needs its password."),
            Shut::Trouble(why) => why,
        })
    }

    /// The same, with a password for a locked set. `""` for the ordinary case
    /// — which also opens a file whose only password is the owner's, since
    /// that is what an empty user password means.
    pub fn open_with(path: PathBuf, password: &str) -> Result<Doc, Shut> {
        let on_disk = stamp(&path);
        let mut file = pdf::Document::open(&path)
            .map_err(|e| Shut::Trouble(format!("{}: {e}", path.display())))?;
        if file.encrypted && !file.unlock(password) {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            return Err(Shut::WantsPassword(name));
        }
        let locked = file.lock();
        let read_only = std::fs::metadata(&path)
            .map(|m| m.permissions().readonly())
            .unwrap_or(false);
        let count = file.page_count();
        let (pages, frames) = sizes(&file);
        let mut doc = Doc {
            id: 0,
            path,
            file,
            labels: vec![SheetLabel::default(); count],
            frames,
            pages,
            page: 0,
            view: View::new(),
            tiles: Tiles::default(),
            previews: HashMap::new(),
            geometry: HashMap::new(),
            geometry_asked: HashSet::new(),
            thumbs: HashMap::new(),
            asked: HashSet::new(),
            marks: Vec::new(),
            scales: Default::default(),
            scales_to_send: BTreeMap::default(), // WHAT: nothing waiting. WHY: a drawing just opened has sent nothing
            draft: None,
            selected: None,
            also: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: false,
            filter: String::new(),
            sent: Vec::new(),
            open_millis: 0,
            labelled: 0,
            drawing: annot::appearance::Draw::default(),
            read_only,
            locked,
            on_disk,
            attached: None,
            split: Split::None,
            layout: crate::layout::Layout::default(),
            other: None,
            side: 0,
            sync_panes: false,
            picks: Default::default(),
            redraw: Default::default(),
            redrawing: Default::default(),
            fresh_from: 0,
            paragraphs: HashMap::new(),
            paragraphs_asked: HashSet::new(),
            reshuffled: false,
            sheets_changed: false,
            relabel: false,
            saved_by: String::new(),
            refreshes_owed: 0,
            password: password.to_string(),
        };
        doc.reload_marks();
        Ok(doc)
    }

    /// Puts a finished draft on the sheet it was drawn on.
    pub fn place(&mut self, page: u32, draft: Draft) -> Option<usize> {
        let frame = self.frame_of(page);
        let scale = self.scale_of(page);
        let markup = draft.into_markup(&frame, scale.as_ref())?;
        self.checkpoint();
        self.marks.push(Mark::new(page, markup));
        Some(self.marks.len() - 1)
    }

    /// The same, as part of something bigger that has already taken its
    /// own undo step — one click that places ten markups is one step back.
    pub fn place_quietly(&mut self, page: u32, draft: Draft) -> Option<usize> {
        let frame = self.frame_of(page);
        let scale = self.scale_of(page);
        let mut markup = draft.into_markup(&frame, scale.as_ref())?;
        if let Some(scale) = &scale {
            let kind = markup.kind();
            if kind.measures() && kind != Kind::Count {
                markup.set_contents(&caption_for(&markup, kind, scale));
            }
        }
        self.marks.push(Mark::new(page, markup));
        self.dirty = true;
        Some(self.marks.len() - 1)
    }

    pub fn size(&self) -> PageSize {
        self.pages.get(self.page as usize).copied().unwrap_or(PageSize {
            width: 612.0,
            height: 792.0,
        })
    }

    /// Splits the window, or puts it back.
    ///
    /// Both panes start on the same sheet, each fitted to its own half. Fitted
    /// rather than sharing an offset: two panes of different widths sharing one
    /// offset would show different parts of the sheet while claiming to be the
    /// same view, which is exactly the confusion a split is meant to remove.
    pub fn set_split(&mut self, split: Split) {
        if split == self.split {
            return;
        }
        self.split = split;
        if split.on() {
            if self.other.is_none() {
                self.other = Some(Pane {
                    page: self.page,
                    view: self.view.clone(),
                });
            }
            self.view.fit_requested = true;
            if let Some(other) = self.other.as_mut() {
                other.view.fit_requested = true;
            }
        } else {
            // Closing the split keeps the pane being worked in, wherever it
            // happens to be on screen, and gives it the whole window.
            self.other = None;
            self.side = 0;
            self.view.fit_requested = true;
        }
    }

    /// Moves the work to the other pane.
    pub fn switch_pane(&mut self) {
        let Some(other) = self.other.as_mut() else {
            return;
        };
        std::mem::swap(&mut self.page, &mut other.page);
        std::mem::swap(&mut self.view, &mut other.view);
        self.side = 1 - self.side;
        self.draft = None;
        self.selected = None;
        self.sent.clear();
    }

    /// The sheet the other pane is showing, if there is one.
    pub fn other_page(&self) -> Option<u32> {
        self.other.as_ref().map(|p| p.page)
    }

    pub fn frame(&self) -> Frame {
        self.frame_of(self.page)
    }

    pub fn frame_of(&self, page: u32) -> Frame {
        self.frames
            .get(page as usize)
            .copied()
            .unwrap_or_else(|| Frame::new([0.0, 0.0, 612.0, 792.0], 0))
    }

    /// The scale in force on a sheet: what the user has just set, else what the
    /// file says, else nothing at all.
    pub fn scale_of(&self, page: u32) -> Option<Measure> {
        if let Some(pending) = self.scales.get(&page) {
            return pending.clone();
        }
        let sheet = self.file.page(page as usize)?;
        annot::viewport::scale_of(&self.file, &sheet)
    }

    pub fn scale(&self) -> Option<Measure> {
        self.scale_of(self.page)
    }

    pub fn set_scale(&mut self, page: u32, measure: Option<Measure>) {
        if self.attached.is_some() {
            self.scales_to_send.insert(page, measure.clone()); // WHAT: kept to send to the office server. WHY: the scale is the office's, read by every seat and by the takeoff
        }
        self.scales.insert(page, measure);
        self.dirty = true;
        self.recaption(page);
    }

    /// Takes the scale the office server keeps for `page` (a `/Measure`
    /// written as PDF, base64), unless this seat has one of its own still to
    /// send, or the sheet already reads the same. True when it changed.
    pub fn take_server_scale(&mut self, page: u32, measure: &str) -> bool {
        use base64::Engine;
        if self.scales_to_send.contains_key(&page) || page as usize >= self.pages.len() {
            return false; // WHAT: this seat's own scale wins, and a page it hasn't got is no page. WHY: a scale just set here is sent, never overwritten by the one it replaces
        }
        let Some(theirs) = base64::engine::general_purpose::STANDARD
            .decode(measure.as_bytes())
            .ok()
            .and_then(|bytes| pdf::Reader::new(&bytes).object().ok())
            .and_then(|o| o.as_dict().and_then(Measure::read))
        else {
            return false; // WHAT: one that doesn't read. WHY: nothing to apply
        };
        let written = |m: &Measure| {
            let mut out = Vec::new();
            pdf::write::write_object(&pdf::Object::Dict(m.write()), &mut out);
            out
        }; // WHAT: a scale as bytes. WHY: two scales compared exactly
        if self.scale_of(page).is_some_and(|ours| written(&ours) == written(&theirs)) {
            return false; // WHAT: the same scale already. WHY: nothing changes
        }
        self.scales.insert(page, Some(theirs)); // WHAT: taken, as if set here. WHY: measuring and the next save use it; it came from the server, so it isn't sent back
        true
    }

    /// Any sheet with a scale at all, which is what a takeoff can total.
    pub fn scaled_pages(&self) -> usize {
        (0..self.pages.len() as u32)
            .filter(|p| self.scale_of(*p).is_some())
            .count()
    }

    pub fn sheet_name(&self, page: u32) -> String {
        match self.labels.get(page as usize) {
            Some(l) if !l.number.is_empty() => l.number.clone(),
            Some(l) if !l.title.is_empty() => l.title.clone(),
            _ => format!("Sheet {}", page + 1),
        }
    }

    pub fn live(&self) -> impl Iterator<Item = (usize, &Mark)> {
        self.marks.iter().enumerate().filter(|(_, m)| !m.gone)
    }

    pub fn on_page(&self, page: u32) -> impl Iterator<Item = (usize, &Mark)> {
        self.live().filter(move |(_, m)| m.page == page)
    }

    pub fn checkpoint(&mut self) {
        self.checkpoint_named("Change");
    }

    /// Remembers where the markups were, under a name somebody would use for
    /// what is about to happen. The name is what the Undo History window
    /// shows, so "Delete 3 markups" beats "Change".
    pub fn checkpoint_named(&mut self, what: &str) {
        self.undo.push(Step {
            what: what.to_string(),
            marks: self.marks.clone(),
            file: None,
            sheets: None,
        });
        if self.undo.len() > REMEMBERED_STEPS {
            self.undo.remove(0);
        }
        // Once something new is done there is no forward to go to.
        self.redo.clear();
        self.dirty = true;
    }

    /// Steps back one checkpoint.
    ///
    /// A markup that was already written into the file cannot simply vanish
    /// from the list: the file would still hold it. Anything in the file but
    /// not in the restored state is carried back as removed, so the next save
    /// takes it out.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        if let Some(sheets) = previous.sheets.clone() {
            if let Err(why) = self.swap_sheets(&sheets, false) {
                log::warn!("could not put the sheets back: {why}");
                self.undo.push(previous);
                return false;
            }
            self.redo.push(Step {
                what: previous.what,
                marks: self.marks.clone(),
                file: None,
                sheets: Some(sheets),
            });
            return true;
        }
        if let Some(swap) = &previous.file {
            if let Err(why) = self.swap_page(swap, false) {
                log::warn!("could not step back a change to the drawing: {why}");
                self.undo.push(previous);
                return false;
            }
        }
        let forward = Step {
            what: previous.what.clone(),
            marks: self.marks.clone(),
            file: previous.file.clone(),
            sheets: None,
        };
        self.restore(previous.marks);
        self.redo.push(forward);
        true
    }

    /// Puts back what the last undo took away.
    pub fn redo(&mut self) -> bool {
        let Some(forward) = self.redo.pop() else {
            return false;
        };
        if let Some(sheets) = forward.sheets.clone() {
            if let Err(why) = self.swap_sheets(&sheets, true) {
                log::warn!("could not change the sheets again: {why}");
                self.redo.push(forward);
                return false;
            }
            self.undo.push(Step {
                what: forward.what,
                marks: self.marks.clone(),
                file: None,
                sheets: Some(sheets),
            });
            return true;
        }
        if let Some(swap) = &forward.file {
            if let Err(why) = self.swap_page(swap, true) {
                log::warn!("could not put back a change to the drawing: {why}");
                self.redo.push(forward);
                return false;
            }
        }
        let back = Step {
            what: forward.what.clone(),
            marks: self.marks.clone(),
            file: forward.file.clone(),
            sheets: None,
        };
        self.restore(forward.marks);
        self.undo.push(back);
        true
    }

    /// The file as it now is, read again, and opened with the same password
    /// when it has one.
    ///
    /// A locked set has to be opened again after every save. Read without
    /// its password, every word in it is ciphertext: markups came back with
    /// scrambled names, and the next save added plain objects to a locked
    /// file, which no reader can make sense of.
    pub fn read_again(&self, bytes: Vec<u8>) -> pdf::Document {
        let mut file = pdf::Document::from_bytes(bytes);
        if file.encrypted && !file.unlock(&self.password) {
            log::warn!("{} could not be opened again with its password", self.path.display());
        }
        file
    }

    /// Points a sheet at its drawing from before a change, or after it, as
    /// its own small save. The markups on it are left as they are.
    pub fn swap_page(&mut self, swap: &PageSwap, forward: bool) -> Result<(), String> {
        if self.read_only {
            return Err("the drawing is read-only".into());
        }
        let wanted = if forward { &swap.after } else { &swap.before };
        let current = self.file.get(swap.page);
        let mut page = current.as_dict().cloned().ok_or("that sheet is not in the file any more")?;
        match &wanted.contents {
            Some(c) => page.set(pdf::Name::new("Contents"), c.clone()),
            None => {
                page.remove("Contents");
            }
        }
        match &wanted.resources {
            Some(r) => page.set(pdf::Name::new("Resources"), r.clone()),
            None => {
                page.remove("Resources");
            }
        }
        let mut update = pdf::Update::new(&self.file);
        for (reference, object) in wanted.objects.iter() {
            update.replace(*reference, object.clone());
        }
        update.replace(swap.page, pdf::Object::Dict(page));
        let bytes = update.apply(&self.file);
        write_into_place(&self.path, &bytes)?;
        self.file = self.read_again(bytes);
        self.on_disk = stamp(&self.path);
        if let Some(index) = self.file.pages().iter().position(|r| *r == swap.page) {
            let index = index as u32;
            self.redraw.insert(index);
            self.paragraphs.remove(&index);
            self.paragraphs_asked.remove(&index);
            self.geometry.remove(&index);
            self.geometry_asked.remove(&index);
        }
        Ok(())
    }

    /// Goes back to a particular step, taking every step after it with it.
    pub fn back_to(&mut self, step: usize) -> bool {
        if step >= self.undo.len() {
            return false;
        }
        let mut went = false;
        while self.undo.len() > step {
            if !self.undo_one() {
                break;
            }
            went = true;
        }
        went
    }

    fn undo_one(&mut self) -> bool {
        self.undo()
    }

    /// Puts a remembered list of markups back, carrying anything already in
    /// the file that is not in it back as removed.
    ///
    /// A markup that was already written into the file cannot simply vanish
    /// from the list: the file would still hold it. So anything in the file
    /// but not in the restored state comes back marked gone, and the next
    /// save takes it out.
    fn restore(&mut self, wanted: Vec<Mark>) {
        let kept: HashSet<Ref> = wanted.iter().filter_map(|m| m.reference).collect();
        let mut restored = wanted;
        for mark in &self.marks {
            let Some(reference) = mark.reference else { continue };
            if kept.contains(&reference) {
                continue;
            }
            let mut back = mark.clone();
            back.gone = true;
            restored.push(back);
        }
        self.marks = restored;
        self.selected = None;
        self.also.clear();
        self.dirty = true;
    }

    /// Everything held at once, the one whose properties are shown first.
    pub fn selection(&self) -> Vec<usize> {
        let mut out = Vec::new();
        if let Some(one) = self.selected {
            out.push(one);
        }
        for more in &self.also {
            if Some(*more) != self.selected && !out.contains(more) {
                out.push(*more);
            }
        }
        out.retain(|i| self.marks.get(*i).map(|m| !m.gone).unwrap_or(false));
        out
    }

    /// Takes hold of one markup, letting go of everything else. A markup that
    /// is part of a group — a Cloud+'s cloud or its words — brings the rest
    /// of its group with it.
    pub fn choose(&mut self, index: Option<usize>) {
        self.selected = index;
        self.also.clear();
        if let Some(i) = index {
            self.also = crate::groups::group_of(&self.marks, i)
                .into_iter()
                .filter(|j| *j != i)
                .collect();
        }
    }

    /// Adds one markup to what is already held, or lets go of it if it is,
    /// with the rest of its group.
    pub fn choose_also(&mut self, index: usize) {
        let group = crate::groups::group_of(&self.marks, index);
        if group.len() > 1 {
            let held = self.selection().contains(&index);
            for member in group {
                if self.selection().contains(&member) == held {
                    self.choose_one_also(member);
                }
            }
            return;
        }
        self.choose_one_also(index);
    }

    fn choose_one_also(&mut self, index: usize) {
        if self.selected == Some(index) {
            // Letting go of the one whose properties are shown promotes the
            // next one, so a selection never loses its head while it still
            // has a body.
            self.selected = self.also.first().copied();
            self.also.retain(|i| Some(*i) != self.selected);
            return;
        }
        if let Some(at) = self.also.iter().position(|i| *i == index) {
            self.also.remove(at);
        } else if self.selected.is_none() {
            self.selected = Some(index);
        } else {
            self.also.push(index);
        }
    }

    /// Measures a markup against its sheet's scale.
    pub fn row(&self, index: usize) -> Option<takeoff::Row> {
        let mark = self.marks.get(index)?;
        let scale = self.scale_of(mark.page);
        let mut row = takeoff::measure(
            mark.page as usize,
            mark.reference.unwrap_or(Ref::new(0, 0)),
            &mark.markup,
            scale.as_ref(),
        );
        row.grid = self.grid_location(index);
        Some(row)
    }

    /// Where a markup sits on its sheet's grid: "B-4", "B/C-4", or nothing.
    ///
    /// Nothing is a real answer — a sheet with no grid, or a detail drawn out
    /// in the margin past the end of one. It is never the nearest label as a
    /// consolation, because "near line E" and "at line E" are different
    /// places to send a fitter.
    pub fn grid_location(&self, index: usize) -> String {
        let Some(mark) = self.marks.get(index) else {
            return String::new();
        };
        let Some(label) = self.labels.get(mark.page as usize) else {
            return String::new();
        };
        if label.grid.is_empty() {
            return String::new();
        }
        // The grid is read in sheet space — measured down from the top left
        // of the page box — so the markup has to be put in the same space
        // before the two can be compared.
        let frame = self
            .frames
            .get(mark.page as usize)
            .cloned()
            .unwrap_or_else(|| Frame::new([0.0, 0.0, 612.0, 792.0], 0));
        let points = mark.on_sheet(&frame);
        let Some(area) = takeoff::geometry::bounds(&points) else {
            return String::new();
        };
        label
            .grid
            .where_is_box(area, takeoff::grid::HowClose::default())
            .say()
    }

    pub fn rows(&self) -> Vec<takeoff::Row> {
        self.live().filter_map(|(i, _)| self.row(i)).collect()
    }

    /// Rewrites the words on every measurement on a sheet, which is what has to
    /// happen when its scale changes.
    pub fn recaption(&mut self, page: u32) {
        let scale = self.scale_of(page);
        let count = self.marks.len();
        for index in 0..count {
            if self.marks[index].page != page || self.marks[index].gone {
                continue;
            }
            let kind = self.marks[index].kind();
            if !kind.measures() || kind == Kind::Count {
                continue;
            }
            let caption = match &scale {
                Some(scale) => caption_for(&self.marks[index].markup, kind, scale),
                None => String::new(),
            };
            self.marks[index].markup.set_contents(&caption);
            match &scale {
                Some(scale) => {
                    self.marks[index]
                        .markup
                        .set("Measure", pdf::Object::Dict(scale.write()));
                }
                None => {
                    self.marks[index].markup.dict.remove("Measure");
                }
            }
            self.marks[index].changed = true;
        }
    }

    /// Re-reads the measurement on everything selected.
    ///
    /// Moving, flipping or resizing a markup changes what it measures. A
    /// program that kept the old number would be reporting a length that is
    /// no longer on the drawing, which is exactly the kind of quiet wrongness
    /// a takeoff cannot have.
    pub fn remeasure_selection(&mut self) {
        for index in self.selection() {
            let Some(mark) = self.marks.get(index) else { continue };
            let page = mark.page;
            let kind = mark.kind();
            if !kind.measures() || kind == Kind::Count {
                continue;
            }
            let scale = self.scale_of(page);
            let caption = match &scale {
                Some(scale) => caption_for(&self.marks[index].markup, kind, scale),
                None => String::new(),
            };
            self.marks[index].markup.set_contents(&caption);
            self.marks[index].changed = true;
        }
    }

    /// Writes everything into the file, by incremental update.
    pub fn save(&mut self, author: &str) -> Result<usize, String> {
        if !author.is_empty() {
            self.saved_by = author.to_string();
        }
        if !self.dirty {
            return Ok(0);
        }
        // Somebody else saved this drawing while it was open here — another
        // estimator on the share, or Bluebeam. Their markups are brought in
        // first and ours put on top, so neither of us loses anything. Writing
        // our copy over theirs is what a program that does not check does.
        let mut theirs_came_in = false;
        if stamp(&self.path).is_some() && stamp(&self.path) != self.on_disk {
            let theirs = self.catch_up()?;
            theirs_came_in = theirs > 0;
            if theirs > 0 {
                log::info!(
                    "{} was saved elsewhere since it was opened; kept {theirs} change(s) from there",
                    self.path.display()
                );
            }
        }
        let mut placer = annot::place::Placer::new(&self.file)
            .by(author)
            .drawing(self.drawing);
        for (page, measure) in &self.scales {
            placer.set_scale(*page as usize, measure.clone());
        }
        let mut written = 0usize;
        // The sheets whose drawing changes with this save.
        let touched: std::collections::BTreeSet<u32> = if theirs_came_in {
            (0..self.pages.len() as u32).collect()
        } else {
            self.marks
                .iter()
                .filter(|m| m.changed || m.gone)
                .map(|m| m.page)
                .collect()
        };
        // The order the markups are in here is the order they go in the file,
        // and that order is what decides which is in front of which. Kept as
        // they are written so "bring to front" survives a save.
        let mut order: std::collections::BTreeMap<usize, Vec<Ref>> = Default::default();
        // Where each named markup is in the file, so a group's parts can say
        // whose they are the way other programs read it.
        let mut placed: std::collections::HashMap<String, Ref> = self
            .marks
            .iter()
            .filter_map(|m| m.reference.map(|r| (m.markup.name(), r)))
            .filter(|(n, _)| !n.is_empty())
            .collect();
        for mark in self.marks.iter_mut() {
            if !mark.gone && mark.markup.dict.has(crate::groups::PART_OF) {
                let had = mark.markup.dict.get("IRT").cloned();
                crate::groups::point_at_head(mark, &placed);
                if mark.markup.dict.get("IRT").cloned() != had {
                    mark.changed = true;
                }
            }
            match (mark.reference, mark.gone, mark.changed) {
                (Some(reference), true, _) => {
                    placer.remove(mark.page as usize, reference);
                    written += 1;
                }
                (Some(reference), false, true) => {
                    if placer.replace(reference, &mut mark.markup) {
                        written += 1;
                    }
                    order.entry(mark.page as usize).or_default().push(reference);
                }
                (Some(reference), false, false) => {
                    order.entry(mark.page as usize).or_default().push(reference);
                }
                (None, false, _) => {
                    if let Some(reference) = placer.add(mark.page as usize, &mut mark.markup) {
                        written += 1;
                        order.entry(mark.page as usize).or_default().push(reference);
                        let name = mark.markup.name();
                        if !name.is_empty() {
                            placed.insert(name, reference);
                        }
                    }
                }
                _ => {}
            }
        }
        for (page, refs) in order {
            placer.order(page, refs);
        }
        let bytes = placer.finish().apply(&self.file);

        // A drawing copied off a read-only share is common, and finding out at
        // save time that the markups cannot land is worth a plain sentence
        // rather than an errno.
        let read_only = std::fs::metadata(&self.path)
            .map(|m| m.permissions().readonly())
            .unwrap_or(false);
        if read_only {
            return Err(format!(
                "{} is read-only. Use Save As to put it somewhere you can write, \
                 or clear the read-only flag on the file. Nothing has been lost.",
                self.path.display()
            ));
        }

        if let Err(why) = write_into_place(&self.path, &bytes) {
            log::warn!("could not save {}: {why}", self.path.display());
            return Err(why);
        }

        // Reopen so every markup now has a place in the file.
        self.file = self.read_again(bytes);
        self.marks.retain(|m| !m.gone);
        self.reload_marks();
        self.scales.clear();
        self.dirty = false;
        self.on_disk = stamp(&self.path);
        self.redraw.extend(touched);
        Ok(written)
    }

    /// Brings in what somebody else saved to this file since it was opened
    /// here, and puts this copy's own unsaved changes back on top of it.
    /// Returns how many of their markups are new or changed.
    ///
    /// Markups are matched by name, which Revu and Hyperview both give every
    /// markup and which survives any number of saves by either program.
    fn catch_up(&mut self) -> Result<usize, String> {
        let bytes = std::fs::read(&self.path)
            .map_err(|e| format!("{} changed on disk and could not be read again: {e}", self.path.display()))?;
        let fresh = self.read_again(bytes);
        // The same sheets in the same order, or markups would land on the
        // wrong ones: somebody else may have moved sheets about.
        if *fresh.pages() != *self.file.pages() {
            return Err(format!(
                "{} was changed somewhere else while it was open here, and its sheets are \
                 not the same any more. Nothing has been lost: use Save As to keep your \
                 markups in a copy, then open the new one.",
                self.path.display()
            ));
        }
        let before: HashMap<String, String> = self
            .marks
            .iter()
            .filter(|m| m.reference.is_some() && !m.changed && !m.gone)
            .map(|m| (m.markup.name(), format!("{:?}", m.markup.dict)))
            .collect();
        let mine: Vec<Mark> = std::mem::take(&mut self.marks)
            .into_iter()
            .filter(|m| m.changed || m.gone || m.reference.is_none())
            .collect();
        self.file = fresh;
        self.reload_marks();
        let theirs = self
            .marks
            .iter()
            .filter(|m| {
                before.get(&m.markup.name()).map(|was| *was != format!("{:?}", m.markup.dict))
                    .unwrap_or(true)
            })
            .count();
        for mut mark in mine {
            let name = mark.markup.name();
            let at = if name.is_empty() {
                None
            } else {
                self.marks.iter().position(|m| m.markup.name() == name)
            }
            .or_else(|| {
                mark.reference
                    .and_then(|r| self.marks.iter().position(|m| m.reference == Some(r)))
            });
            match (mark.gone, at) {
                // Deleted here: deleted there too.
                (true, Some(at)) => self.marks[at].gone = true,
                (true, None) => {}
                // Changed here: this copy's version wins over the one on disk.
                (false, Some(at)) => {
                    self.marks[at].markup = mark.markup;
                    self.marks[at].page = mark.page;
                    self.marks[at].changed = true;
                }
                // New here, or deleted there while it was being changed
                // here: it goes in as new. Something somebody was working on
                // is never lost to somebody else's delete.
                (false, None) => {
                    mark.reference = None;
                    mark.changed = true;
                    self.marks.push(mark);
                }
            }
        }
        self.on_disk = stamp(&self.path);
        Ok(theirs)
    }

    /// Every markup's name, which is how one is recognised wherever it has
    /// been.
    pub fn names(&self) -> std::collections::HashSet<String> {
        self.marks
            .iter()
            .filter(|m| !m.gone)
            .map(|m| m.markup.name())
            .filter(|n| !n.is_empty())
            .collect()
    }

    /// Puts markups that came from somebody else into this file, and takes out
    /// the ones they removed. They are marked changed, so the next save writes
    /// them into the PDF — which is still the file of record.
    pub fn merge(
        &mut self,
        add: Vec<(u32, Markup)>,
        remove: Vec<String>,
    ) -> crate::sync::Merged {
        let mut merged = crate::sync::Merged::default();
        for (page, markup) in add {
            self.marks.push(Mark::new(page, markup));
            merged.taken += 1;
        }
        if !remove.is_empty() {
            let gone: std::collections::HashSet<String> = remove.into_iter().collect();
            for mark in self.marks.iter_mut() {
                if mark.gone {
                    continue;
                }
                if gone.contains(&mark.markup.name()) {
                    mark.gone = true;
                    merged.removed += 1;
                }
            }
        }
        if !merged.nothing() {
            self.dirty = true;
            self.selected = None;
        }
        merged
    }

    /// Reads the markups back out of the file.
    pub fn reload_marks(&mut self) {
        // What was held stays held, by name: saving every few seconds must
        // not drop whatever somebody was in the middle of moving or editing.
        // A markup from a program that gave it no name is known by where it
        // is in the file instead, which a save keeps.
        #[derive(PartialEq)]
        enum Known {
            Name(String),
            At(Ref),
        }
        let known = |marks: &Vec<Mark>, i: usize| {
            let m = marks.get(i)?;
            let name = m.markup.name();
            if !name.is_empty() {
                Some(Known::Name(name))
            } else {
                m.reference.map(Known::At)
            }
        };
        let held = self.selected.and_then(|i| known(&self.marks, i));
        let also: Vec<Known> = self.also.iter().filter_map(|i| known(&self.marks, *i)).collect();
        let mut marks = Vec::new();
        for page in 0..self.pages.len() {
            for (reference, markup) in annot::place::read_page(&self.file, page) {
                marks.push(Mark::from_file(page as u32, reference, markup));
            }
        }
        self.marks = marks;
        let find = |marks: &Vec<Mark>, wanted: &Known| {
            marks.iter().position(|m| match wanted {
                Known::Name(name) => m.markup.name() == *name,
                Known::At(at) => m.reference == Some(*at),
            })
        };
        self.selected = held.as_ref().and_then(|k| find(&self.marks, k));
        self.also = also.iter().filter_map(|k| find(&self.marks, k)).collect();
    }
}

/// Every sheet's size as it is seen, and how its page space sits on it.
pub fn sizes(file: &pdf::Document) -> (Vec<PageSize>, Vec<Frame>) {
    let count = file.page_count();
    let mut pages = Vec::with_capacity(count);
    let mut frames = Vec::with_capacity(count);
    for i in 0..count {
        match file.page(i) {
            Some(page) => {
                let area = file.page_box(&page);
                let rotation = file.page_rotation(&page);
                let frame = Frame::new(area, rotation);
                let (width, height) = frame.size();
                pages.push(PageSize {
                    width: width as f32,
                    height: height as f32,
                });
                frames.push(frame);
            }
            None => {
                pages.push(PageSize {
                    width: 612.0,
                    height: 792.0,
                });
                frames.push(Frame::new([0.0, 0.0, 612.0, 792.0], 0));
            }
        }
    }
    (pages, frames)
}

/// A file as it stands on disk: its size and when it was last written.
pub type Stamp = (u64, Option<std::time::SystemTime>);

pub fn stamp(path: &std::path::Path) -> Option<Stamp> {
    std::fs::metadata(path).ok().map(|m| (m.len(), m.modified().ok()))
}

/// Puts `bytes` where `path` is, as safely as the folder allows.
///
/// Written beside the original first and moved into place, so a failure half
/// way cannot leave anybody with half a drawing set. When something else has
/// the file open for a moment — a virus scanner, the search indexer, Explorer's
/// preview — the move is tried again. When the folder lets people change files
/// but not make new ones, as some office shares are set up, the file is
/// written over where it is. And when none of that works, the answer says why
/// in words somebody can act on.
pub fn write_into_place(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    let temp = path.with_extension("saving");
    match std::fs::write(&temp, bytes) {
        Ok(()) => {
            let mut last: Option<std::io::Error> = None;
            for attempt in 0..4u64 {
                match std::fs::rename(&temp, path) {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        let again = in_use(&e);
                        last = Some(e);
                        if !again {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(120 * (attempt + 1)));
                    }
                }
            }
            let refused = last.expect("the loop only ends early with an error");
            // Something has the file open in a way that stops it being
            // replaced but may still let it be written to.
            match write_over(path, bytes) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&temp);
                    Ok(())
                }
                Err(Over::NotOpened(_)) => {
                    let _ = std::fs::remove_file(&temp);
                    Err(explain(path, &refused))
                }
                Err(Over::PartWritten(e)) => Err(format!(
                    "{} Your markups were written in full to {} — rename it to .pdf to use it.",
                    explain(path, &e),
                    temp.display()
                )),
            }
        }
        // No new files allowed in this folder: write over the file itself.
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => match write_over(path, bytes) {
            Ok(()) => Ok(()),
            Err(Over::NotOpened(e2)) | Err(Over::PartWritten(e2)) => Err(explain(path, &e2)),
        },
        Err(e) => Err(explain(path, &e)),
    }
}

enum Over {
    NotOpened(std::io::Error),
    PartWritten(std::io::Error),
}

fn write_over(path: &std::path::Path, bytes: &[u8]) -> Result<(), Over> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(Over::NotOpened)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(Over::PartWritten)
}

/// Whether an error is somebody else holding the file for a moment.
fn in_use(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(32) | Some(33) | Some(5))
}

/// A save that failed, said so somebody can do something about it.
pub fn explain(path: &std::path::Path, e: &std::io::Error) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let folder = path
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let safe = "Nothing has been lost: your markups are still here, and Excalibur View tries again \
                by itself. Save As puts them somewhere else now.";
    match (cfg!(windows), e.raw_os_error()) {
        (true, Some(32)) | (true, Some(33)) => format!(
            "{name} is open somewhere else — on another computer, in Bluebeam, or in a \
             preview pane — so it cannot be written just now. {safe}"
        ),
        (true, Some(5)) => format!(
            "Windows would not let this computer change {name} in {folder}. If somebody has it \
             open in Bluebeam, that is why. Otherwise whoever looks after the server needs to \
             give you Modify permission on that folder. {safe}"
        ),
        (true, Some(53)) | (true, Some(59)) | (true, Some(64)) | (true, Some(67))
        | (true, Some(121)) | (true, Some(1231)) | (true, Some(1232)) => format!(
            "{folder} could not be reached — the network or the server dropped. {safe}"
        ),
        (true, Some(39)) | (true, Some(112)) => format!("The disk {name} is on is full. {safe}"),
        _ if e.kind() == std::io::ErrorKind::PermissionDenied => format!(
            "This computer is not allowed to change {name} in {folder}. {safe}"
        ),
        _ => format!("{}: {e}. {safe}", path.display()),
    }
}

/// Bluebeam's measurement intents, as the ones this program knows how to draw.
/// Anything else out of a tool is left alone rather than guessed at.
fn known_intent(name: &str) -> Option<&'static str> {
    Some(match name {
        "LineDimension" => "LineDimension",
        "PolyLineDimension" => "PolyLineDimension",
        "Polylength" => "Polylength",
        "PolygonDimension" => "PolygonDimension",
        "PolygonVolume" => "PolygonVolume",
        "PolygonCount" => "PolygonCount",
        "CircleDimension" => "CircleDimension",
        "PolygonRadius" => "PolygonRadius",
        "PolyLineAngle" => "PolyLineAngle",
        _ => return None,
    })
}

/// The words that go on a measurement, worked out from its own geometry.
/// The circle three points sit on, as a centre and a radius.
///
/// Three clicks on the edge of a hole is how somebody measures a hole they can
/// see but whose middle is not drawn. Three points in a line sit on no circle
/// at all, and that comes back as `None` rather than as a circle the size of
/// the sheet.
pub fn circle_through(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> Option<([f64; 2], f64)> {
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() < 1e-9 {
        return None;
    }
    let (aa, bb, cc) = (
        a[0] * a[0] + a[1] * a[1],
        b[0] * b[0] + b[1] * b[1],
        c[0] * c[0] + c[1] * c[1],
    );
    let x = (aa * (b[1] - c[1]) + bb * (c[1] - a[1]) + cc * (a[1] - b[1])) / d;
    let y = (aa * (c[0] - b[0]) + bb * (a[0] - c[0]) + cc * (b[0] - a[0])) / d;
    let radius = ((a[0] - x).powi(2) + (a[1] - y).powi(2)).sqrt();
    if !radius.is_finite() || radius <= 0.0 {
        return None;
    }
    Some(([x, y], radius))
}

/// The arc from one end to the other passing through a point between them.
///
/// Given back as a run of points, because a PDF annotation has no arc: what
/// goes in the file is the curve itself. Three points in a line give back the
/// straight run between the ends, which is what they describe.
pub fn arc_through(from: [f64; 2], to: [f64; 2], through: [f64; 2]) -> Vec<[f64; 2]> {
    let Some((centre, radius)) = circle_through(from, through, to) else {
        return vec![from, to];
    };
    let angle = |p: [f64; 2]| (p[1] - centre[1]).atan2(p[0] - centre[0]);
    let (a0, a1, am) = (angle(from), angle(to), angle(through));

    // Go round the way that passes through the middle click, which is the
    // whole reason for asking for it.
    let normal = |mut t: f64| {
        while t < 0.0 {
            t += std::f64::consts::TAU;
        }
        while t >= std::f64::consts::TAU {
            t -= std::f64::consts::TAU;
        }
        t
    };
    let forward = normal(a1 - a0);
    let to_middle = normal(am - a0);
    let sweep = if to_middle <= forward {
        forward
    } else {
        forward - std::f64::consts::TAU
    };

    // Enough segments that the curve reads as a curve at any zoom somebody
    // will look at it under, and few enough that the file stays small.
    let steps = ((sweep.abs() * radius / 4.0).ceil() as usize).clamp(8, 96);
    (0..=steps)
        .map(|i| {
            let t = a0 + sweep * (i as f64 / steps as f64);
            [centre[0] + radius * t.cos(), centre[1] + radius * t.sin()]
        })
        .collect()
}

pub fn caption_for(markup: &Markup, kind: Kind, scale: &Measure) -> String {
    let points = markup.points();
    match kind {
        // A run with a pitch on it is measured up the slope, the same as the
        // totals measure it, so the caption and the list never disagree.
        Kind::Length => scale.length(takeoff::row::up_the_slope(
            markup,
            takeoff::geometry::length(&points),
        )),
        // A perimeter goes round: the last side is the one back to the start.
        Kind::Polylength => scale.length(takeoff::row::up_the_slope(
            markup,
            if markup.subtype() == annot::Subtype::Polygon {
                takeoff::geometry::perimeter(&points)
            } else {
                takeoff::geometry::length(&points)
            },
        )),
        Kind::Area => scale.area(takeoff::geometry::area(&points)),
        // A volume without a depth is not a volume. Reporting the area
        // instead would be a number in the wrong units that looks right.
        Kind::Volume => match markup.dict.get("Depth").and_then(|o| o.as_f64()) {
            Some(depth) if depth > 0.0 => {
                scale.volume(takeoff::geometry::area(&points) * depth)
            }
            _ => String::new(),
        },
        Kind::Diameter => {
            let r = takeoff::geometry::radius_through(&points)
                .or_else(|| takeoff::geometry::bounds(&points).map(takeoff::geometry::radius_of_box))
                .unwrap_or(0.0);
            scale.length(r * 2.0)
        }
        Kind::Radius => {
            let r = takeoff::geometry::radius_through(&points)
                .or_else(|| takeoff::geometry::bounds(&points).map(takeoff::geometry::radius_of_box))
                .unwrap_or(0.0);
            scale.length(r)
        }
        Kind::Angle => scale.angle(takeoff::geometry::angle(&points)),
        Kind::Count => "1".into(),
        Kind::Markup => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use annot::measure::imperial;
    use annot::Subtype;

    #[test]
    fn a_change_that_rewrote_the_sheets_own_stream_keeps_the_old_one_to_go_back_to() {
        // A page whose drawing is object 4, then a change that writes a new
        // object 4 in place, the way PDFium saves an edited page.
        let bodies = [
            "<</Type/Catalog/Pages 2 0 R>>",
            "<</Type/Pages/Kids[3 0 R]/Count 1>>",
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R>>",
            "<</Length 12>>\nstream\nBT (old) ET\nendstream",
        ];
        let mut file = b"%PDF-1.5\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in bodies.iter().enumerate() {
            offsets.push(file.len());
            file.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let table = file.len();
        file.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for offset in &offsets {
            file.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        file.extend_from_slice(format!("trailer\n<</Size 5/Root 1 0 R>>\nstartxref\n{table}\n%%EOF\n").as_bytes());
        let old = pdf::Document::from_bytes(file);
        let page = old.pages()[0];
        let mut update = pdf::Update::new(&old);
        update.replace(
            pdf::Ref::new(4, 0),
            pdf::Object::Stream(Box::new(pdf::Stream { dict: pdf::Dict::new(), data: b"BT (new) ET".to_vec() })),
        );
        update.replace(page, (*old.get(page)).clone());
        let new = pdf::Document::from_bytes(update.apply(&old));

        let (before, after) = Drawing::either_side(&old, &new, page).unwrap();
        assert_eq!(before.objects.len(), 1);
        assert_eq!(before.objects[0].0, pdf::Ref::new(4, 0));
        assert_eq!(before.objects[0].1.as_stream().unwrap().data, b"BT (old) ET\n".to_vec());
        assert_eq!(after.objects[0].1.as_stream().unwrap().data, b"BT (new) ET".to_vec());
    }

    fn quarter() -> Measure {
        imperial(48.0, "1/4\" = 1'-0\"", 16)
    }

    /// A one-page drawing on disk, opened as a tab is.
    fn a_tab(name: &str) -> Doc {
        let folder = std::env::temp_dir().join(format!("hv-scale-{name}-{}", std::process::id())); // WHAT: a folder of its own. WHY: tests run side by side
        std::fs::create_dir_all(&folder).unwrap(); // WHAT: made. WHY: the drawing goes in it
        let bodies = ["<</Type/Catalog/Pages 2 0 R>>", "<</Type/Pages/Kids[3 0 R]/Count 1>>", "<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]>>"]; // WHAT: one sheet, naming no scale. WHY: as p.25
        let mut file = b"%PDF-1.5\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in bodies.iter().enumerate() {
            offsets.push(file.len());
            file.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let table = file.len();
        file.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
        for offset in &offsets {
            file.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        file.extend_from_slice(format!("trailer\n<</Size 4/Root 1 0 R>>\nstartxref\n{table}\n%%EOF\n").as_bytes());
        let path = folder.join("S-525.pdf"); // WHAT: the drawing's place. WHY: opened from there
        std::fs::write(&path, file).unwrap(); // WHAT: written. WHY: as a download is
        Doc::open(path).unwrap() // WHAT: opened. WHY: as a tab
    }

    fn written(m: &Measure) -> String {
        use base64::Engine;
        let mut bytes = Vec::new();
        pdf::write::write_object(&pdf::Object::Dict(m.write()), &mut bytes);
        base64::engine::general_purpose::STANDARD.encode(bytes) // WHAT: the scale as the server keeps it. WHY: what a sync hands back
    }

    #[test]
    fn a_scale_set_on_an_office_drawing_waits_to_go_to_the_office_and_one_set_on_another_seat_is_taken() {
        // Mesa, 2026-11-24: a scale set on p.25 lived only in this seat's
        // copy, and was gone the next time the drawing opened.
        let mut ours = a_tab("ours"); // WHAT: a drawing from the office server. WHY: the case that lost its scale
        ours.attached = Some(crate::sync::Attached { set: "set_1".into(), revision: 0, sent: HashSet::default() }); // WHAT: attached to its set. WHY: as a download is
        ours.set_scale(0, Some(quarter())); // WHAT: the scale set, as Set Scale... does. WHY: the step that was lost
        assert!(ours.scales_to_send.get(&0).is_some_and(Option::is_some), "waiting to go to the office"); // WHAT: queued. WHY: the next sync sends it
        let other = imperial(96.0, "1/8\" = 1'-0\"", 16); // WHAT: another seat's scale. WHY: must not overwrite one set here
        assert!(!ours.take_server_scale(0, &written(&other)), "this seat's own scale wins until it is sent"); // WHAT: not taken. WHY: it would undo what was just set

        let mut loose = a_tab("loose"); // WHAT: a drawing on this computer only. WHY: nothing to send
        loose.set_scale(0, Some(quarter())); // WHAT: a scale set. WHY: kept in the file at the next save, as always
        assert!(loose.scales_to_send.is_empty(), "a drawing not from the office sends nothing"); // WHAT: nothing queued. WHY: there is no office copy

        let mut theirs = a_tab("theirs"); // WHAT: the same drawing opened again, or on a second seat. WHY: the scale must be there
        theirs.attached = Some(crate::sync::Attached { set: "set_1".into(), revision: 0, sent: HashSet::default() }); // WHAT: attached. WHY: as above
        assert!(theirs.scale_of(0).is_none(), "the downloaded copy names no scale"); // WHAT: none in the file. WHY: the server's PDF is as uploaded
        assert!(theirs.take_server_scale(0, &written(&quarter())), "the office's scale taken"); // WHAT: taken. WHY: the fix
        assert_eq!(theirs.scale_of(0).map(|m| m.ratio), Some("1/4\" = 1'-0\"".to_string()), "and measured with"); // WHAT: in force. WHY: measuring uses it
        assert!(!theirs.take_server_scale(0, &written(&quarter())), "the same scale again changes nothing"); // WHAT: not again. WHY: nothing to redraw
        assert!(theirs.scales_to_send.is_empty(), "the office's own scale isn't sent back"); // WHAT: not queued. WHY: it came from there
        assert!(!theirs.take_server_scale(0, "not base64!"), "nothing that doesn't read"); // WHAT: refused. WHY: never a broken scale
        assert!(!theirs.take_server_scale(9, &written(&quarter())), "no page it hasn't got"); // WHAT: refused. WHY: no such sheet
    }

    #[test]
    fn a_length_caption_reads_the_way_the_sheet_does() {
        let mut m = Markup::new(Subtype::Line);
        m.set("IT", pdf::Object::name("LineDimension"));
        m.set_line([0.0, 0.0], [720.0, 0.0]);
        assert_eq!(caption_for(&m, Kind::Length, &quarter()), "40'-0\"");
    }

    #[test]
    fn an_area_caption_is_in_square_feet() {
        let mut m = Markup::new(Subtype::Polygon);
        m.set("IT", pdf::Object::name("PolygonDimension"));
        m.set_vertices(&[[0.0, 0.0], [720.0, 0.0], [720.0, 360.0], [0.0, 360.0]]);
        assert_eq!(caption_for(&m, Kind::Area, &quarter()), "800 sf");
    }

    #[test]
    fn a_perimeter_goes_all_the_way_round() {
        // A closed shape's last side is the one back to where it started.
        // Measuring it as an open run leaves every shape one side short —
        // quietly, and always low.
        let mut m = Markup::new(Subtype::Polygon);
        m.set("IT", pdf::Object::name("Polylength"));
        // 40 feet by 20 feet at a quarter inch to the foot: 120 feet round.
        // Measured as an open run it would read 100 feet — one side short.
        m.set_vertices(&[
            [0.0, 0.0],
            [720.0, 0.0],
            [720.0, 360.0],
            [0.0, 360.0],
        ]);
        assert_eq!(caption_for(&m, Kind::Polylength, &quarter()), "120'-0\"");
    }

    #[test]
    fn a_length_along_an_open_run_does_not_close_itself() {
        let mut m = Markup::new(Subtype::PolyLine);
        m.set("IT", pdf::Object::name("PolyLineDimension"));
        m.set_vertices(&[[0.0, 0.0], [720.0, 0.0], [720.0, 360.0]]);
        assert_eq!(caption_for(&m, Kind::Polylength, &quarter()), "60'-0\"");
    }

    #[test]
    fn a_volume_with_no_depth_reads_as_nothing_rather_than_as_an_area() {
        // An area shown where a volume belongs is a number in the wrong units
        // that looks right, which is the worst kind of wrong.
        let mut m = Markup::new(Subtype::Polygon);
        m.set("IT", pdf::Object::name("PolygonVolume"));
        m.set_vertices(&[[0.0, 0.0], [720.0, 0.0], [720.0, 360.0], [0.0, 360.0]]);
        assert_eq!(caption_for(&m, Kind::Volume, &quarter()), "");
    }

    #[test]
    fn a_volume_with_a_depth_reads_as_a_volume() {
        let mut m = Markup::new(Subtype::Polygon);
        m.set("IT", pdf::Object::name("PolygonVolume"));
        m.set_vertices(&[[0.0, 0.0], [720.0, 0.0], [720.0, 360.0], [0.0, 360.0]]);
        // Six inches deep, in the sheet's own points at a quarter inch scale:
        // 48 inches of building to the inch of paper, so half a foot is
        // 6/48 of an inch, which is 9 points.
        m.set("Depth", pdf::Object::real(720.0 * 0.5 / 20.0));
        let said = caption_for(&m, Kind::Volume, &quarter());
        assert!(!said.is_empty(), "it should read as something");
    }

    #[test]
    fn a_circle_from_three_clicks_is_the_circle_those_three_points_sit_on() {
        // Three clicks on the edge of a hole is how somebody measures a hole
        // whose middle is not drawn.
        let found = circle_through([0.0, 10.0], [10.0, 0.0], [0.0, -10.0]);
        let (centre, radius) = found.expect("three points on a circle");
        assert!(centre[0].abs() < 1e-6 && centre[1].abs() < 1e-6, "{centre:?}");
        assert!((radius - 10.0).abs() < 1e-6, "{radius}");
    }

    #[test]
    fn three_points_in_a_line_sit_on_no_circle_and_say_so() {
        assert!(circle_through([0.0, 0.0], [10.0, 0.0], [20.0, 0.0]).is_none());
    }

    #[test]
    fn an_arc_passes_through_the_point_it_was_told_to() {
        let curve = arc_through([0.0, 0.0], [100.0, 0.0], [50.0, 50.0]);
        assert!(curve.len() > 8);
        assert!((curve[0][0] - 0.0).abs() < 0.01 && (curve[0][1]).abs() < 0.01);
        let last = curve[curve.len() - 1];
        assert!((last[0] - 100.0).abs() < 0.01 && last[1].abs() < 0.01);
        // And it really goes over the top rather than under.
        assert!(curve.iter().any(|p| p[1] > 45.0), "it should bulge up");
        assert!(!curve.iter().any(|p| p[1] < -1.0), "and not down");
    }

    #[test]
    fn an_arc_through_three_points_in_a_line_is_the_straight_run() {
        let curve = arc_through([0.0, 0.0], [100.0, 0.0], [50.0, 0.0]);
        assert_eq!(curve, vec![[0.0, 0.0], [100.0, 0.0]]);
    }

    #[test]
    fn a_markup_that_measures_nothing_gets_no_caption() {
        let mut m = Markup::new(Subtype::Square);
        m.set_box([0.0, 0.0, 10.0, 10.0]);
        assert_eq!(caption_for(&m, Kind::Markup, &quarter()), "");
    }

    #[test]
    fn a_mark_reports_its_geometry_in_the_space_the_screen_uses() {
        let frame = Frame::new([-1512.0, -1080.0, 1512.0, 1080.0], 0);
        let mut m = Markup::new(Subtype::Line);
        // Two points in PDF space, around the origin.
        m.set_line([-1512.0, 1080.0], [-1152.0, 1080.0]);
        let mark = Mark::new(0, m);
        let on_sheet = mark.on_sheet(&frame);
        assert_eq!(on_sheet[0], [0.0, 0.0]);
        assert_eq!(on_sheet[1], [360.0, 0.0]);
    }

    /// A tool as a Revu chest carries it: the annotation dictionary.
    fn a_chest_tool(text: &str) -> (crate::app::Tool, pdf::Dict) {
        let dict = pdf::Reader::new(text.as_bytes())
            .object()
            .unwrap()
            .as_dict()
            .unwrap()
            .clone();
        let (tool, _) = crate::app::Tool::for_chest(annot::Kind::read(&dict), &dict);
        (tool, dict)
    }

    fn drawn_with(tool: crate::app::Tool, template: pdf::Dict, points: &[[f64; 2]]) -> Markup {
        let draft = Draft {
            tool,
            points: points.to_vec(),
            subject: "Revision".into(),
            template: Some(template),
            ..Draft::default()
        };
        draft
            .into_markup(&Frame::new([0.0, 0.0, 612.0, 792.0], 0), None)
            .expect("a markup")
    }

    #[test]
    fn a_cloud_from_a_chest_is_drawn_as_a_cloud() {
        let (tool, template) = a_chest_tool(
            "<< /Type /Annot /Subtype /Polygon /IT /PolygonCloud /Subj (Revision) \
             /BE << /S /C /I 1 >> /C [1 0 0] >>",
        );
        assert_eq!(tool, crate::app::Tool::CloudPolygon);
        let m = drawn_with(tool, template, &[[10.0, 10.0], [90.0, 10.0], [90.0, 60.0], [10.0, 60.0]]);
        assert_eq!(m.subtype(), Subtype::Polygon, "not a rectangle");
        assert_eq!(m.kind(), Kind::Markup);
        let be = m.dict.get("BE").and_then(|o| o.as_dict()).expect("a cloudy border");
        assert_eq!(be.get("I").and_then(|o| o.as_f64()), Some(1.0), "his intensity, not ours");
        assert_eq!(
            m.dict.get("IT").and_then(|o| o.as_name()).map(|n| n.as_str().to_string()),
            Some("PolygonCloud".into()),
            "it goes back to Revu as the same tool"
        );
    }

    #[test]
    fn a_callout_from_a_chest_is_drawn_as_a_callout() {
        let (tool, template) = a_chest_tool(
            "<< /Type /Annot /Subtype /FreeText /IT /FreeTextCallout /Subj (RFI) >>",
        );
        assert_eq!(tool, crate::app::Tool::Callout);
        let m = drawn_with(tool, template, &[[10.0, 10.0], [120.0, 200.0]]);
        assert_eq!(m.subtype(), Subtype::FreeText);
        assert!(m.dict.get("CL").is_some(), "it has its leader");
    }

    #[test]
    fn every_markup_in_a_chest_is_armed_as_its_own_kind() {
        use crate::app::Tool;
        for (text, expected) in [
            ("<< /Subtype /Square /BE << /S /C >> >>", Tool::Cloud),
            ("<< /Subtype /Square >>", Tool::Rect),
            ("<< /Subtype /Circle >>", Tool::Ellipse),
            ("<< /Subtype /Polygon >>", Tool::Polygon),
            ("<< /Subtype /PolyLine >>", Tool::Polyline),
            ("<< /Subtype /Line /LE [/None /OpenArrow] >>", Tool::Arrow),
            ("<< /Subtype /Ink >>", Tool::Ink),
            ("<< /Subtype /FreeText >>", Tool::Text),
            ("<< /Subtype /FreeText /IT /FreeTextTypeWriter >>", Tool::Typewriter),
            ("<< /Subtype /Text >>", Tool::Note),
            ("<< /Subtype /Highlight >>", Tool::Highlight),
            ("<< /Subtype /Line /IT /LineDimension >>", Tool::Length),
            ("<< /Subtype /Polygon /IT /PolygonVolume >>", Tool::Volume),
            ("<< /Subtype /Circle /IT /CircleDimension >>", Tool::Diameter),
            ("<< /Subtype /PolyLine /IT /PolyLineAngle >>", Tool::Angle),
        ] {
            assert_eq!(a_chest_tool(text).0, expected, "{text}");
        }
    }
}
