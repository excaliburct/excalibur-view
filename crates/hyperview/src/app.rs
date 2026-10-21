//! The application: window, panels, tools and document state.

use std::path::{Path, PathBuf};



use crate::render;
use crate::render::{FromWorker, ToWorker};
use crate::sheet::Doc;
use crate::units::Units;

/// A drag that is moving what is selected.
#[derive(Clone, Copy, Debug)]
pub struct Moving {
    /// Where the pointer was last frame, in sheet space.
    pub last: [f64; 2],
    /// Whether anything has moved yet — the undo step is taken then, so a
    /// click that wobbles is not an undo step.
    pub moved: bool,
    /// Moving one part of a group on its own: a Cloud+'s box of words, whose
    /// leader keeps pointing at the cloud.
    pub only: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Pan,
    Select,
    /// Drag a loop round several markups at once.
    Lasso,
    Calibrate,
    Length,
    Area,
    Count,
    Rect,
    Ellipse,
    Arrow,
    Ink,
    Text,
    Highlight,
    /// An area taken out of another: a column in a slab, a shaft in a floor.
    AreaCutout,
    /// An elliptical opening taken out of an area.
    EllipseCutout,
    /// Click inside a region and let the drawing's own lines close it in.
    Fill,
    /// Draw across a doorway so a fill stops there.
    FillBoundary,

    // ---- the rest of the measurements ----
    /// The way round a closed shape, rather than the area inside it.
    Perimeter,
    /// Across a circle, from three points on it.
    Diameter,
    /// From the middle of a circle out, from three points on it.
    Radius,
    /// Between two lines struck from a corner.
    Angle,
    /// An area with a depth: footings, walls, slabs by the yard.
    Volume,

    // ---- the rest of the markups ----
    /// An open run of straight segments.
    Polyline,
    /// A closed shape of straight segments.
    Polygon,
    /// A box with a cloudy border: the revision mark.
    Cloud,
    /// A cloudy border round a shape clicked out corner by corner.
    CloudPolygon,
    /// A cloud with a leader to a box of words: Revu's Cloud+.
    CloudPlus,
    /// Your own signature, put down with a click.
    Signature,
    /// Change the words already on the sheet.
    EditText,
    /// Three clicks: the two ends and a point the curve passes through.
    Arc,
    /// A dimension line with arrows at both ends and the length on it.
    Dimension,
    /// A note in a box with a leader pointing at what it is about.
    Callout,
    /// A comment that folds away into a marker.
    Note,
    /// A marker that stands out on the sheet, for coming back to.
    Flag,
    /// Text on the sheet with nothing drawn round it.
    Typewriter,
    /// Rubs out freehand strokes, and nothing else.
    Eraser,
    /// A line under words on the sheet.
    Underline,
    /// A line through words on the sheet.
    Strikethrough,
    /// A wavy line under words on the sheet.
    Squiggly,
    /// A picture placed on the sheet.
    Image,
    /// A stamp from the chest.
    Stamp,
    /// A box that will be blacked out, and its content taken out with it.
    Redaction,
    /// A box that opens something when it is clicked.
    Hyperlink,
    /// A file carried along with the drawing.
    FileAttachment,
    /// A named region a takeoff can be broken down by.
    Space,
    /// Drag a box round part of the sheet and take a picture of it.
    Snapshot,

    // ---- form fields ----
    /// A box somebody types in.
    FormText,
    FormCheckBox,
    FormRadio,
    FormList,
    FormCombo,
    FormButton,
    FormSignature,

    // ---- sketch to scale ----
    /// A shape drawn by typing its sides rather than clicking them.
    SketchPolygon,
    SketchRect,
    SketchEllipse,
    SketchPolyline,
}

/// How a tool collects what it needs from the sheet.
///
/// Tools differ in almost nothing except this, so saying it once here keeps
/// the canvas from growing a branch per tool.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shaping {
    /// Not drawn at all: pan, select, and the sketch tools that ask for
    /// numbers in a window instead.
    None,
    /// One click puts it down.
    Click,
    /// Dragged out between two corners.
    Drag,
    /// Drawn freehand while the button is down.
    Freehand,
    /// A click per corner until a double click, Enter or a right click.
    MultiClick,
    /// Exactly three clicks, then it finishes itself.
    ThreePoint,
}

impl Tool {
    /// Whether a point put down with this tool is pulled onto the drawing.
    /// Everything that measures, draws or places does; picking things up and
    /// moving round the sheet do not, and neither does a freehand pen.
    pub fn snaps(self) -> bool {
        !matches!(
            self,
            Tool::Pan
                | Tool::Lasso
                | Tool::Ink
                | Tool::Highlight
                | Tool::Eraser
                | Tool::Underline
                | Tool::Strikethrough
                | Tool::Squiggly
                | Tool::Snapshot
                // Words go where they are put, not onto the nearest line end.
                | Tool::Text
                | Tool::Typewriter
        )
    }

    /// The annotation this tool draws: its subtype, and Bluebeam's intent
    /// where the markup is a measurement rather than a drawing.
    pub fn shape(self) -> Option<(annot::Subtype, Option<&'static str>)> {
        use annot::Subtype as S;
        Some(match self {
            Tool::Length => (S::Line, Some("LineDimension")),
            Tool::Area => (S::Polygon, Some("PolygonDimension")),
            Tool::AreaCutout => (S::Polygon, Some("PolygonCutout")),
            Tool::EllipseCutout => (S::Circle, Some("PolygonCutout")),
            Tool::Count => (S::Polygon, Some("PolygonCount")),
            Tool::Perimeter => (S::Polygon, Some("Polylength")),
            Tool::Diameter => (S::Circle, Some("CircleDimension")),
            Tool::Radius => (S::Circle, Some("PolygonRadius")),
            Tool::Angle => (S::PolyLine, Some("PolyLineAngle")),
            Tool::Volume => (S::Polygon, Some("PolygonVolume")),
            Tool::Rect | Tool::Redaction | Tool::Cloud | Tool::CloudPlus => (S::Square, None),
            Tool::Ellipse => (S::Circle, None),
            Tool::Arrow | Tool::Dimension => (S::Line, None),
            Tool::Ink => (S::Ink, None),
            Tool::Text | Tool::Callout | Tool::Typewriter => (S::FreeText, None),
            Tool::Note | Tool::Flag => (S::Text, None),
            Tool::Highlight => (S::Highlight, None),
            Tool::Underline => (S::Underline, None),
            Tool::Strikethrough => (S::StrikeOut, None),
            Tool::Squiggly => (S::Squiggly, None),
            Tool::Polyline | Tool::Arc => (S::PolyLine, None),
            Tool::Polygon | Tool::CloudPolygon | Tool::Space => (S::Polygon, None),
            Tool::Image | Tool::Stamp | Tool::Snapshot | Tool::Signature => (S::Stamp, None),
            Tool::Hyperlink => (S::Link, None),
            Tool::FileAttachment => (S::Other, None),
            Tool::FormText
            | Tool::FormCheckBox
            | Tool::FormRadio
            | Tool::FormList
            | Tool::FormCombo
            | Tool::FormButton
            | Tool::FormSignature => (S::Other, None),
            Tool::SketchPolygon => (S::Polygon, Some("PolygonDimension")),
            Tool::SketchRect => (S::Polygon, Some("PolygonDimension")),
            Tool::SketchEllipse => (S::Circle, Some("PolygonDimension")),
            Tool::SketchPolyline => (S::PolyLine, Some("PolyLineDimension")),
            // A fill is not a shape somebody drew; it becomes a Polygon area
            // when they apply it, and until then it is a proposal.
            Tool::Fill | Tool::FillBoundary => return None,
            Tool::Eraser => return None,
            Tool::Pan | Tool::Select | Tool::Lasso | Tool::Calibrate | Tool::EditText => return None,
        })
    }

    /// How the canvas collects this tool's geometry.
    pub fn shaping(self) -> Shaping {
        match self {
            Tool::Pan
            | Tool::Select
            | Tool::EditText
            | Tool::SketchPolygon
            | Tool::SketchRect
            | Tool::SketchEllipse
            | Tool::SketchPolyline => Shaping::None,

            Tool::Count
            | Tool::Text
            | Tool::Typewriter
            | Tool::Note
            | Tool::Flag
            | Tool::Stamp
            | Tool::Signature
            | Tool::FileAttachment => Shaping::Click,

            Tool::Rect
            | Tool::Ellipse
            | Tool::Arrow
            | Tool::Dimension
            | Tool::Highlight
            | Tool::Underline
            | Tool::Strikethrough
            | Tool::Squiggly
            | Tool::Cloud
            | Tool::CloudPlus
            | Tool::Image
            | Tool::Redaction
            | Tool::Hyperlink
            | Tool::Snapshot
            | Tool::FormText
            | Tool::FormCheckBox
            | Tool::FormRadio
            | Tool::FormList
            | Tool::FormCombo
            | Tool::FormButton
            | Tool::FormSignature
            | Tool::Lasso => Shaping::Drag,

            Tool::Ink | Tool::Eraser => Shaping::Freehand,

            Tool::Length
            | Tool::Area
            | Tool::Calibrate
            | Tool::AreaCutout
            | Tool::Perimeter
            | Tool::Volume
            | Tool::Polyline
            | Tool::Polygon
            | Tool::CloudPolygon
            | Tool::Callout
            | Tool::Space => Shaping::MultiClick,

            Tool::Arc | Tool::Diameter | Tool::Radius | Tool::Angle => Shaping::ThreePoint,

            // Dynamic Fill runs its own input.
            Tool::Fill | Tool::FillBoundary => Shaping::None,
            Tool::EllipseCutout => Shaping::Drag,
        }
    }

    /// Tools that collect points click by click until the user says they are
    /// finished, rather than being dragged out in one motion.
    pub fn is_multi_click(self) -> bool {
        self.shaping() == Shaping::MultiClick
    }

    /// Tools that are part of Dynamic Fill rather than ordinary drawing.
    pub fn is_filling(self) -> bool {
        matches!(self, Tool::Fill | Tool::FillBoundary)
    }

    /// Tools whose shape closes back on itself.
    pub fn is_closed(self) -> bool {
        matches!(
            self,
            Tool::Area
                | Tool::AreaCutout
                | Tool::EllipseCutout
                | Tool::Perimeter
                | Tool::Volume
                | Tool::Polygon
                | Tool::CloudPolygon
                | Tool::Space
                | Tool::Count
                | Tool::SketchPolygon
                | Tool::SketchRect
                | Tool::SketchEllipse
        )
    }

    /// Tools that draw a cloudy border rather than a plain one.
    pub fn is_cloudy(self) -> bool {
        matches!(self, Tool::Cloud | Tool::CloudPolygon | Tool::CloudPlus)
    }

    /// The tool that draws a Tool Chest tool, and the toolbar button that
    /// lights up while it is armed.
    ///
    /// A measuring tool goes by what it measures. A tool that only marks up
    /// goes by the annotation it stamps: a revision cloud is drawn as a cloud
    /// and a callout as a callout, not as a box with the right colour.
    pub fn for_chest(kind: annot::Kind, template: &pdf::Dict) -> (Tool, &'static str) {
        use annot::{Kind, Subtype as S};
        match kind {
            Kind::Length | Kind::Polylength => (Tool::Length, "Measure.Length"),
            Kind::Area => (Tool::Area, "Measure.Area"),
            Kind::Volume => (Tool::Volume, "Measure.Volume"),
            Kind::Count => (Tool::Count, "Measure.Count"),
            Kind::Angle => (Tool::Angle, "Measure.Angle"),
            Kind::Diameter => (Tool::Diameter, "Measure.Diameter"),
            Kind::Radius => (Tool::Radius, "Measure.Radius"),
            Kind::Markup => {
                let name = |key: &str| {
                    template
                        .get(key)
                        .and_then(|o| o.as_name())
                        .map(|n| n.as_str().to_string())
                        .unwrap_or_default()
                };
                let intent = name("IT");
                let cloudy = intent.ends_with("Cloud")
                    || template
                        .get("BE")
                        .and_then(|o| o.as_dict())
                        .and_then(|be| be.get("S"))
                        .and_then(|o| o.as_name())
                        .is_some_and(|s| s.as_str() == "C");
                match S::read(template) {
                    S::Square if cloudy => (Tool::Cloud, "Markup.Cloud"),
                    S::Square => (Tool::Rect, "Markup.Rectangle"),
                    S::Circle => (Tool::Ellipse, "Markup.Ellipse"),
                    S::Polygon if cloudy => (Tool::CloudPolygon, "Markup.Cloud9"),
                    S::Polygon => (Tool::Polygon, "Markup.Polygon"),
                    S::PolyLine => (Tool::Polyline, "Markup.Polyline"),
                    S::Line => (Tool::Arrow, "Markup.Line"),
                    S::Ink => (Tool::Ink, "Markup.Pen"),
                    S::FreeText if intent == "FreeTextCallout" => (Tool::Callout, "Markup.Callout"),
                    S::FreeText if intent == "FreeTextTypeWriter" => {
                        (Tool::Typewriter, "Markup.Typewriter")
                    }
                    S::FreeText => (Tool::Text, "Markup.TextBox"),
                    S::Text => (Tool::Note, "Markup.Note"),
                    S::Highlight => (Tool::Highlight, "Markup.Highlight"),
                    S::Underline => (Tool::Underline, "Markup.Underline"),
                    S::StrikeOut => (Tool::Strikethrough, "Markup.Strikethrough"),
                    S::Squiggly => (Tool::Squiggly, "Markup.Squiggly"),
                    // A stamp, a link or something newer: a box is the
                    // nearest thing that can still be drawn and moved.
                    _ => (Tool::Rect, "Markup.Rectangle"),
                }
            }
        }
    }

    /// Tools that mark words on the sheet rather than draw on it.
    pub fn marks_text(self) -> bool {
        matches!(
            self,
            Tool::Highlight | Tool::Underline | Tool::Strikethrough | Tool::Squiggly
        )
    }

    /// Tools that lay out a form rather than mark up a drawing.
    pub fn is_form(self) -> bool {
        matches!(
            self,
            Tool::FormText
                | Tool::FormCheckBox
                | Tool::FormRadio
                | Tool::FormList
                | Tool::FormCombo
                | Tool::FormButton
                | Tool::FormSignature
        )
    }

    /// Tools that are drawn by typing dimensions rather than clicking them.
    pub fn is_sketch(self) -> bool {
        matches!(
            self,
            Tool::SketchPolygon | Tool::SketchRect | Tool::SketchEllipse | Tool::SketchPolyline
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::Pan => "Pan",
            Tool::Select => "Select",
            Tool::Lasso => "Lasso",
            Tool::Calibrate => "Calibrate",
            Tool::Length => "Length",
            Tool::Area => "Area",
            Tool::Count => "Count",
            Tool::Rect => "Box",
            Tool::Ellipse => "Ellipse",
            Tool::Arrow => "Arrow",
            Tool::Ink => "Pen",
            Tool::Text => "Text",
            Tool::Highlight => "Highlight",
            Tool::AreaCutout => "Area Cutout",
            Tool::EllipseCutout => "Ellipse Cutout",
            Tool::Fill => "Dynamic Fill",
            Tool::FillBoundary => "Fill Boundary",
            Tool::Perimeter => "Perimeter",
            Tool::Diameter => "Diameter",
            Tool::Radius => "Radius",
            Tool::Angle => "Angle",
            Tool::Volume => "Volume",
            Tool::Polyline => "Polyline",
            Tool::Polygon => "Polygon",
            Tool::Cloud => "Cloud",
            Tool::CloudPolygon => "Polygon Cloud",
            Tool::CloudPlus => "Cloud+",
            Tool::Signature => "Signature",
            Tool::EditText => "Edit Text",
            Tool::Arc => "Arc",
            Tool::Dimension => "Dimension",
            Tool::Callout => "Callout",
            Tool::Note => "Note",
            Tool::Flag => "Flag",
            Tool::Typewriter => "Typewriter",
            Tool::Eraser => "Eraser",
            Tool::Underline => "Underline",
            Tool::Strikethrough => "Strikethrough",
            Tool::Squiggly => "Squiggly",
            Tool::Image => "Image",
            Tool::Stamp => "Stamp",
            Tool::Redaction => "Redaction",
            Tool::Hyperlink => "Hyperlink",
            Tool::FileAttachment => "File Attachment",
            Tool::Space => "Space",
            Tool::Snapshot => "Snapshot",
            Tool::FormText => "Text Field",
            Tool::FormCheckBox => "Check Box",
            Tool::FormRadio => "Radio Button",
            Tool::FormList => "List Box",
            Tool::FormCombo => "Combo Box",
            Tool::FormButton => "Button",
            Tool::FormSignature => "Signature Field",
            Tool::SketchPolygon => "Sketch Polygon",
            Tool::SketchRect => "Sketch Box",
            Tool::SketchEllipse => "Sketch Ellipse",
            Tool::SketchPolyline => "Sketch Polyline",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Tool::Pan => "Drag to move the sheet. Middle mouse or the space bar pans with any tool.",
            Tool::Select => "Click a markup to select it; drag it to move it. Delete removes it. Middle mouse or the space bar pans.",
            Tool::Lasso => "Drag a loop round several markups to take hold of them all at once.",
            Tool::Calibrate => "Click the two ends of a dimension you can read, then type what it says.",
            Tool::Length => "Click each corner. Double click, Enter or right click finishes. Shift holds the line square.",
            Tool::Area => "Drag out a rectangle, or click around an irregular shape. Double click, Enter or right click closes it.",
            Tool::Count => "Click each item. Every click is one more.",
            Tool::Rect => "Drag out a box.",
            Tool::Ellipse => "Drag out an ellipse.",
            Tool::Arrow => "Drag from the tail to the head.",
            Tool::Ink => "Draw freehand.",
            Tool::Text => "Click where the note goes, then type.",
            Tool::Highlight => "Drag over what you want highlighted.",
            Tool::AreaCutout => "Drag out a rectangle, or click round something, to take it out of the area it sits in.",
            Tool::EllipseCutout => "Drag out a round opening to take it out of the area it sits in.",
            Tool::Fill => "Click inside an area. Its own lines close it in. Nothing is measured until you apply it.",
            Tool::FillBoundary => "Draw across a doorway or a gap so the fill stops there.",
            Tool::Perimeter => "Drag out a rectangle, or click round the shape. The measurement is the way round it, not the area inside.",
            Tool::Diameter => "Click three points on the circle. The measurement is straight across it.",
            Tool::Radius => "Click three points on the circle. The measurement is from the middle out.",
            Tool::Angle => "Click one leg, then the corner, then the other leg.",
            Tool::Volume => "Drag out a rectangle, or click round the shape, then type the depth. Without a depth there is no volume.",
            Tool::Polyline => "Click each corner. Double click, Enter or right click finishes.",
            Tool::Polygon => "Drag out a rectangle, or click each corner. Double click, Enter or right click closes it.",
            Tool::Cloud => "Drag out a box. It is drawn as a revision cloud.",
            Tool::CloudPlus => "Drag out the cloud, then click where the words go, and type.",
            Tool::Signature => "Click the sheet where your signature goes.",
            Tool::EditText => "Click words on the sheet to change them.",
            Tool::CloudPolygon => "Click round what changed. Double click, Enter or right click closes it.",
            Tool::Arc => "Click the two ends, then a point the curve goes through.",
            Tool::Dimension => "Drag between the two points. The length is written on the line.",
            Tool::Callout => "Click what it is about, then where the box goes, then type.",
            Tool::Note => "Click where the comment goes. It folds away into a marker.",
            Tool::Flag => "Click what you want to come back to.",
            Tool::Typewriter => "Click where the words go, then type. Nothing is drawn round them.",
            Tool::Eraser => "Drag over freehand strokes to rub them out. It leaves everything else alone.",
            Tool::Underline => "Drag over the words you want underlined.",
            Tool::Strikethrough => "Drag over the words you want struck through.",
            Tool::Squiggly => "Drag over the words you want marked.",
            Tool::Image => "Drag out where the picture goes, then choose it.",
            Tool::Stamp => "Click where the stamp goes.",
            Tool::Redaction => "Drag over what has to come out. Nothing is taken out until you apply it.",
            Tool::Hyperlink => "Drag out the area that will be clickable, then say where it goes.",
            Tool::FileAttachment => "Click where the paperclip goes, then choose the file.",
            Tool::Space => "Click round an area and name it. Takeoffs can be broken down by it.",
            Tool::Snapshot => "Drag a box round part of the sheet. It is copied as a picture you can paste anywhere.",
            Tool::FormText => "Drag out a box somebody can type in.",
            Tool::FormCheckBox => "Drag out a box somebody can tick.",
            Tool::FormRadio => "Drag out one of a set of choices. Give them the same name and only one can be picked.",
            Tool::FormList => "Drag out a list to pick from.",
            Tool::FormCombo => "Drag out a list somebody can also type into.",
            Tool::FormButton => "Drag out a button.",
            Tool::FormSignature => "Drag out a place for somebody to sign.",
            Tool::SketchPolygon => "Type the sides one at a time and it draws itself to scale.",
            Tool::SketchRect => "Type the width and the height and it draws itself to scale.",
            Tool::SketchEllipse => "Type the two axes and it draws itself to scale.",
            Tool::SketchPolyline => "Type each run one at a time and it draws itself to scale.",
        }
    }
}

pub const TOOLBOX: &[&[Tool]] = &[
    &[Tool::Pan, Tool::Select],
    &[Tool::Calibrate, Tool::Length, Tool::Area, Tool::Count],
    &[
        Tool::Rect,
        Tool::Ellipse,
        Tool::Arrow,
        Tool::Ink,
        Tool::Text,
        Tool::Highlight,
    ],
];

pub const PALETTE: &[(&str, [u8; 4])] = &[
    ("Red", [220, 40, 40, 255]),
    ("Orange", [240, 140, 20, 255]),
    ("Yellow", [235, 200, 30, 255]),
    ("Green", [40, 170, 70, 255]),
    ("Blue", [40, 110, 230, 255]),
    ("Purple", [150, 70, 210, 255]),
    ("Black", [20, 20, 20, 255]),
];

/// A volume waiting for its depth.
pub struct AskingDepth {
    /// Which markup, by its place in the list.
    pub markup: usize,
    pub typed: String,
    pub error: Option<String>,
}

pub struct Calibrating {
    pub page: u32,
    pub points: f64,
    pub typed: String,
    pub everywhere: bool,
    pub error: Option<String>,
}

/// A locked drawing set, and the box asking for its password.
///
/// Nothing here is kept once the set opens: the typed password moves into
/// `App::passwords`, which itself lives only as long as the program does.
pub struct AskPassword {
    pub path: std::path::PathBuf,
    /// Just the file name, which is what somebody recognises.
    pub name: String,
    pub typed: String,
    /// True when a password has already been tried and refused, so the box
    /// can say so rather than looking as though nothing happened.
    pub wrong: bool,
}

pub struct App {
    pub svc: render::Service,
    pub library_error: Option<String>,
    pub error: Option<String>,
    pub status: String,
    /// Every open drawing set. Tabs, in the order they appear.
    pub docs: Vec<Doc>,
    /// Which one is in front.
    pub current: usize,
    /// The id given to the next drawing opened, so a tab and the render
    /// worker always mean the same file.
    pub next_doc_id: u64,
    pub tool: Tool,
    pub color: [u8; 4],
    pub subject: String,
    /// The Tool Chest tool in hand, if one was picked.
    pub template: Option<pdf::Dict>,
    pub units: Units,
    pub denominator: u32,
    pub calibrating: Option<Calibrating>,
    pub editing_text: Option<usize>,
    /// The text box whose size follows its words while they are typed: one
    /// put down with a click rather than dragged out, the way Revu's is.
    pub typing_fit: Option<usize>,
    /// Which drawing and box the words on the sheet were opened for. The first frame
    /// of a box is when the click that made it may still be in the input,
    /// and that click is not somebody clicking away.
    pub typing_opened: Option<(u64, usize)>,
    /// The click that finished typing on the sheet. It finishes the words and
    /// does nothing else: it does not start another box where it landed.
    pub finishing_click: bool,
    pub show_takeoff: bool,
    pub show_sheets: bool,
    pub last_tile_millis: u128,
    pub opening: bool,
    /// Whose markups these are, for the Author column.
    pub author: String,
    pub chrome: ui::Chrome,
    pub profile: Option<chest::Profile>,
    pub panel: String,
    pub show_markups: bool,
    /// How the markups list is grouped, scoped, filtered and sorted.
    pub list: crate::markuplist::State,
    pub chest_filter: String,
    pub chest_open: std::collections::BTreeSet<String>,
    /// Where a press started, while a box is being dragged out with one of
    /// the tools that otherwise collects corners a click at a time. `None`
    /// whenever no such drag is in progress.
    pub dragged_box: Option<[f64; 2]>,
    /// Passwords for locked drawing sets, typed this session.
    ///
    /// Held in memory and nowhere else: never written to the preferences,
    /// never to disk, gone when the program closes. Somebody who locks a set
    /// and then finds the password saved beside it has been given a lock with
    /// the key taped to it.
    pub passwords: std::collections::HashMap<std::path::PathBuf, String>,
    /// The drawing waiting on a password, when one is.
    pub asking_password: Option<AskPassword>,
    /// Which file the open now in flight is for, so a refusal knows what to
    /// ask about.
    pub opening_path: Option<std::path::PathBuf>,
    pub wheel_zooms: bool,
    pub last_save: std::time::Instant,
    /// Markups counted at the last autosave check, so a save happens once the
    /// drawing has settled rather than in the middle of a run.
    pub settled: Option<(std::time::Instant, usize)>,
    /// A failed autosave is said once, not every few seconds.
    pub autosave_complained: bool,
    pub prefs: crate::prefs::Prefs,
    /// The thread that talks to a server, when there is one to talk to.
    pub link: Option<crate::server::Link>,
    pub standing: crate::server::Standing,
    pub signing_in: Option<crate::serverui::SigningIn>,
    /// The server panel's own forms: a new project, the office settings an
    /// administrator looks after, and changing a password.
    pub office: crate::serverui::OfficePanel,
    /// The print window, when it is up.
    pub printing: Option<crate::print::Printing>,
    /// The Document menu's window, when one is up.
    pub doc_job: Option<crate::docui::Setting>,
    pub doc_task: u64,
    /// A document job started from the Thumbnails panel's picks. Its result
    /// is saved where it was asked to go and not opened, because the point
    /// was a file to send, not another tab.
    pub quiet_task: Option<u64>,
    /// Markups that could not be written into the drawing before the picks
    /// were saved, so the saved copy does not have them.
    pub quiet_left_out: bool,
    /// The Compare Documents window, when one is up.
    pub comparing: Option<crate::compareui::Comparing>,
    pub compare_task: u64,
    /// The Overlay Pages window, when one is up.
    pub overlaying: Option<crate::compareui::Overlaying>,
    /// The Batch window, when one is up.
    pub batching: Option<crate::batchui::Batching>,
    pub batch_task: u64,
    /// Files Explorer handed over to combine, and when the last one came.
    /// They arrive one start at a time; the Combine window opens once they
    /// stop coming.
    pub to_combine: Vec<PathBuf>,
    pub to_combine_at: Option<std::time::Instant>,
    /// The About box, and the list of shortcuts.
    pub about: bool,
    pub shortcuts_open: bool,
    pub recent_open: bool,
    /// How many unsaved markups a pending revert would throw away.
    pub reverting: Option<usize>,
    /// A file name and its fingerprint, while that window is up.
    pub fingerprint: Option<(String, String)>,
    /// The Offset or Multiply window, when one is up.
    pub repeating: Option<crate::repeat::Repeating>,
    /// Rises by one each time, so sheets got ready for a print somebody has
    /// since cancelled are thrown away rather than sent to a plotter.
    pub print_job: u64,
    /// What the Print dialog was last set to, so it opens on it again.
    pub last_print: crate::print::LastChoice,
    /// A server running inside this program, when this computer is the one
    /// hosting the office's drawings. Held here so it lives as long as the
    /// program does — dropping it stops serving.
    pub hosting: Option<crate::host::Hosting>,
    /// Markups cut or copied, waiting to go somewhere.
    pub clipboard: crate::clip::Clipboard,
    /// The Undo History window, when it is up.
    pub history_open: bool,
    /// The loop being dragged round some markups, while it is being dragged.
    pub lasso: Option<Vec<[f64; 2]>>,
    /// Markups being dragged to a new place with Select.
    pub moving: Option<Moving>,
    /// Fingers on the glass: what the sheet's draft was when the first one
    /// came down, so a second finger turning the touch into a pinch can put
    /// back whatever the first one started. See `view::touch`.
    pub touch: crate::view::Touch,
    /// A grip on the selected markup, while it is being dragged.
    pub gripping: Option<crate::grips::Gripping>,
    /// A Cloud+'s cloud has been dragged out and is waiting for a click on
    /// where its words go.
    pub cloud_waiting: bool,
    /// Your signatures: picking one, making one, and the one in hand.
    pub signing: crate::signature::Signing,
    /// Words on the sheet being changed, when they are.
    pub rewording: Option<crate::textedit::Rewording>,
    pub reword_task: u64,
    /// The paragraph under the pointer, with the Edit Text tool in hand.
    pub text_under: Option<usize>,
    /// The look the Format Painter is carrying, when it has one.
    pub painting_format: Option<crate::clip::Style>,
    /// The address being typed for a hyperlink, while that box is up.
    pub asking_address: Option<String>,
    /// Which sheets are on the screen this frame, and where.
    pub showing: Vec<crate::layout::Placed>,
    /// Where the view has been, so it can go back.
    pub been: Vec<crate::places::Place>,
    pub going_back: Vec<crate::places::Place>,
    pub full_screen: bool,
    /// The Document Properties window, when it is up.
    pub properties: Option<crate::properties::Properties>,
    /// The Spell Check window, when it is up.
    pub checking: Option<crate::spell::Checking>,
    /// A form field whose settings are being changed.
    pub editing_field: Option<usize>,
    /// Form editor mode: every field outlined and named.
    pub form_editor: bool,
    /// Everything but the markups drawn faintly.
    pub dimmed: bool,
    /// Adding, taking away or changing a markup's own points.
    pub points_mode: Option<crate::points::Mode>,
    /// Where to put the view back once a drawing that was reopened arrives.
    pub reopen_to: Option<(std::path::PathBuf, u32, crate::view::View)>,
    /// What the next markup will look like.
    pub pen: crate::pen::Pen,
    /// Which chooser on the properties toolbar is open.
    pub choosing: Option<crate::pen::Choosing>,
    /// The depth being typed for a volume, while that box is up.
    pub asking_depth: Option<AskingDepth>,
    /// The cut list window: what to cut, and what to buy to cut it from.
    pub shop: crate::shopwindow::Shop,
    /// Steel models open from File ▸ Open Model.
    pub models: crate::modelwindow::Models,
    /// The double-count check: what looks counted twice.
    pub doubles: crate::doublewindow::Checking,
    /// Revision cost: what a new issue did to the quantities.
    pub costing: crate::revisionwindow::Costing,
    /// What the pointer would snap to right now, shown on the sheet.
    pub snap_hit: Option<takeoff::snap::Hit>,
    /// When a sync was last asked for and has not been answered yet.
    pub sync_asked: Option<std::time::Instant>,
    /// When the drawing in front last synced by the clock.
    pub last_sync: std::time::Instant,
    /// Shared chests already asked for this session, so a slow download is
    /// not asked for twice.
    pub chests_fetched: std::collections::HashSet<String>,
    /// Hyperview's own toolbars, built once, for a seat whose profile has none.
    pub standard_bars: Option<Vec<chest::ToolBar>>,
    /// The version whose banner somebody pressed "Later" on. A newer one
    /// still gets its own.
    pub dismissed_update: Option<String>,
    /// When this seat last asked whether there is a newer version. Asked
    /// again every quarter of an hour, because a seat that is left open all
    /// week is the ordinary case in an office, not the exception.
    pub last_update_check: std::time::Instant,
    /// Whether this copy can put a new version in place by itself. A copy
    /// run from a download folder cannot, and says where to get one instead.
    pub updates_itself: bool,
    /// "Check for Updates" was pressed and has not been answered yet.
    pub checking_for_update: bool,
    /// The answer to "Check for Updates" when there was nothing to install,
    /// shown across the top until somebody presses OK.
    pub update_note: Option<String>,
    /// The last status message, and when it changed. With a drawing open the
    /// status strip is about the drawing, so a new message shows there for a
    /// few seconds and then gives way.
    pub status_seen: (String, std::time::Instant),
    /// The plugins this seat has, and what the menu lists from them.
    pub plugins: crate::plugins::Shelf,
    /// What each plugin's settings were last set to here.
    pub plugin_settings: crate::plugins::Settings,
    /// A plugin run under way.
    pub plugin_job: Option<crate::pluginui::Job>,
    /// Where the office server's answer goes, when it ran a plugin for this
    /// copy (`crate::edition`).
    pub plugin_answer: Option<std::sync::mpsc::Sender<Result<plugin_api::Output, String>>>,
    pub next_plugin_job: u64,
    /// What the last plugin run found, while its window is open.
    pub plugin_shown: Option<crate::pluginui::Shown>,
    /// Plugins ▸ Manage Plugins… is open.
    pub managing_plugins: bool,
    /// Office plugins already asked for this session, by digest.
    pub plugins_fetched: std::collections::HashSet<String>,
    /// `--benchmark`: the run being measured, when there is one.
    pub bench: Option<crate::bench::Bench>,
    pub bench_frame_at: Option<std::time::Instant>,
    /// Tiles the screen is still waiting for, as of the last frame.
    pub missing_tiles: usize,
    /// Drawings another start of the program handed to this window.
    pub inbox: crossbeam_channel::Receiver<crate::instance::Handed>,
    /// Files being chosen: the system's dialog on a desktop, the program's own
    /// list on a tablet. See [`crate::files`].
    pub filing: crate::files::Choosing,
    /// What the assistant asks of this window, through the connector (`crate::desk`).
    pub desk: crossbeam_channel::Receiver<crate::desk::Ask>,
    /// Questions from the assistant waiting on something: a drawing opening, a check.
    pub desk_waits: Vec<crate::deskui::Waiting>,
    /// The sheet to turn to once a drawing that is opening has opened.
    pub turn_to: Option<crate::deskui::TurnTo>,
    /// `hyperview://` links that arrived before the office session was back.
    pub links_waiting: Vec<(String, std::time::Instant)>,
    /// A license file opened by somebody who was not signed in yet. It goes
    /// on the moment an administrator is.
    pub waiting_license: Option<PathBuf>,
    /// Help → Test Assistant Connection: the report, once there is one, and the
    /// test while it runs.
    pub assistant_report: Option<String>,
    pub assistant_testing: Option<std::sync::mpsc::Receiver<String>>,
    /// Finding words across every sheet.
    pub search: crate::find::Search,
    /// A Dynamic Fill in progress.
    pub filling: crate::dynamicfill::Filling,
    /// A VisualSearch in progress.
    pub looking: crate::visual::Looking,
    /// The drawing being opened right now, if any.
    pub pending_open: Option<u64>,
    /// A drawing downloaded from a server and about to open, with the set it
    /// came from, so the tab can be attached to it once it exists.
    pub opened_from_server: Option<(PathBuf, String)>,
    /// Which company's server this copy was installed for, if any.
    pub joined: crate::joined::Joined,
    /// Where the sheet was drawn last frame, so going to a search answer can
    /// put it in the middle of the window.
    pub last_canvas: egui::Rect,
    /// The whole sheet area, before it was split.
    pub last_canvas_whole: egui::Rect,
    /// The Preferences window, and the copy being edited in it.
    pub editing_prefs: Option<crate::prefs::Prefs>,
    /// Sheets being dragged to a new place in the Thumbnails list.
    pub sheet_drag: Option<crate::reorder::Dragging>,
    /// Where the Thumbnails list's rows were last drawn.
    pub sheet_rows: Option<crate::reorder::Rows>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Vec<PathBuf>) -> App {
        ui::chrome::fonts(&cc.egui_ctx);
        ui::Theme::dark().apply(&cc.egui_ctx);
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.item_spacing = egui::vec2(6.0, 6.0);
        if crate::platform::tablet() {
            // Fingers, not a mouse: every control a little taller and wider
            // apart, and a touch that moves a few points still a tap.
            style.spacing.button_padding = egui::vec2(10.0, 7.0);
            style.spacing.item_spacing = egui::vec2(8.0, 8.0);
            style.spacing.interact_size.y = style.spacing.interact_size.y.max(30.0);
            style.interaction.resize_grab_radius_side = 10.0;
            style.interaction.resize_grab_radius_corner = 16.0;
        }
        cc.egui_ctx.set_style(style);
        crate::platform::wake_with(&cc.egui_ctx);

        let svc = render::Service::start(None, cc.egui_ctx.clone());
        let mut app = App {
            svc,
            library_error: None,
            error: None,
            status: "Open a drawing set to begin.".into(),
            docs: Vec::new(),
            current: 0,
            next_doc_id: 1,
            tool: Tool::Select,
            color: PALETTE[0].1,
            subject: String::new(),
            template: None,
            units: Units::FeetInches,
            denominator: 16,
            calibrating: None,
            editing_text: None,
            typing_fit: None,
            typing_opened: None,
            finishing_click: false,
            show_takeoff: false,
            show_sheets: true,
            last_tile_millis: 0,
            opening: false,
            author: whoami(),
            chrome: ui::Chrome::new(),
            profile: crate::profiles::load(),
            panel: "Thumbnails".into(),
            show_markups: false,
            list: Default::default(),
            chest_filter: String::new(),
            chest_open: Default::default(),
            dragged_box: None,
            passwords: Default::default(),
            asking_password: None,
            opening_path: None,
            wheel_zooms: true,
            last_save: std::time::Instant::now(),
            settled: None,
            autosave_complained: false,
            prefs: crate::prefs::Prefs::default(),
            link: Some(crate::server::start(cc.egui_ctx.clone())),
            standing: Default::default(),
            signing_in: None,
            office: Default::default(),
            printing: None,
            doc_job: None,
            doc_task: 0,
            quiet_task: None,
            quiet_left_out: false,
            comparing: None,
            compare_task: 0,
            overlaying: None,
            batching: None,
            to_combine: Vec::new(),
            to_combine_at: None,
            batch_task: 0,
            about: false,
            shortcuts_open: false,
            recent_open: false,
            reverting: None,
            fingerprint: None,
            repeating: None,
            print_job: 0,
            last_print: Default::default(),
            hosting: None,
            clipboard: crate::clip::Clipboard::default(),
            history_open: false,
            lasso: None,
            moving: None,
            gripping: None,
            cloud_waiting: false,
            signing: Default::default(),
            rewording: None,
            reword_task: 0,
            text_under: None,
            painting_format: None,
            asking_address: None,
            showing: Vec::new(),
            been: Vec::new(),
            going_back: Vec::new(),
            full_screen: false,
            properties: None,
            checking: None,
            editing_field: None,
            form_editor: false,
            dimmed: false,
            points_mode: None,
            reopen_to: None,
            pen: crate::pen::Pen::default(),
            choosing: None,
            touch: Default::default(),
            asking_depth: None,
            shop: Default::default(),
            models: Default::default(),
            doubles: Default::default(),
            costing: Default::default(),
            snap_hit: None,
            dismissed_update: None,
            standard_bars: None,
            chests_fetched: Default::default(),
            plugins: crate::plugins::Shelf::load(&crate::install::plugin_trusted()),
            plugin_settings: crate::plugins::load_settings(),
            plugin_job: None,
            plugin_answer: None,
            next_plugin_job: 0,
            plugin_shown: None,
            managing_plugins: false,
            plugins_fetched: Default::default(),
            sync_asked: None,
            last_sync: std::time::Instant::now(),
            last_update_check: std::time::Instant::now(),
            updates_itself: crate::install::updates_itself(),
            checking_for_update: false,
            update_note: None,
            status_seen: (String::new(), std::time::Instant::now()),
            inbox: crate::instance::listen(cc.egui_ctx.clone()),
            filing: Default::default(),
            desk: crate::desk::listen(cc.egui_ctx.clone()),
            desk_waits: Vec::new(),
            turn_to: None,
            links_waiting: Vec::new(),
            waiting_license: None,
            assistant_report: None,
            assistant_testing: None,
            bench: crate::bench::REQUESTED.get().cloned().map(crate::bench::Bench::new),
            bench_frame_at: None,
            missing_tiles: 0,
            search: Default::default(),
            filling: crate::dynamicfill::Filling::new(),
            looking: crate::visual::Looking::new(),
            pending_open: None,
            opened_from_server: None,
            joined: crate::joined::Joined::load(),
            last_canvas: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 700.0)),
            last_canvas_whole: egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1000.0, 700.0),
            ),
            editing_prefs: None,
            sheet_drag: None,
            sheet_rows: None,
        };
        app.chrome.plugins = app.plugins.menu();
        if crate::platform::tablet() {
            // What belongs to a desktop rather than a tablet: connecting a
            // desktop assistant app, and looking for updates, which the app
            // store brings.
            for id in ["Help.ConnectAssistant", "Help.TestAssistant", "Help.CheckForUpdates"] {
                app.chrome.hide(id);
            }
        }
        // Settings from the file win over the defaults filled in above.
        app.prefs = crate::prefs::Prefs::load();
        if app.prefs.classic {
            app.set_look(&cc.egui_ctx, true);
        }
        app.units = app.prefs.units;
        app.denominator = app.prefs.denominator;
        app.wheel_zooms = app.prefs.wheel_zooms;
        if crate::platform::tablet() {
            cc.egui_ctx.set_zoom_factor(app.prefs.control_scale());
        }
        if !app.prefs.author.trim().is_empty() {
            app.author = app.prefs.author.clone();
        }
        // A session kept from last time is picked up quietly. It is checked
        // before it is trusted, and a stale one simply leaves the program
        // signed out rather than interrupting anybody.
        // A seat installed for a company knows where its server is before
        // anybody has typed anything.
        if app.prefs.server.base.is_empty() && app.joined.set() {
            app.standing.base = app.joined.server.clone();
            if !app.joined.name.is_empty() {
                app.standing.name = crate::server::plain_name(&app.joined.name);
            }
        }
        let remembered = app.prefs.server.clone();
        if !remembered.base.is_empty() && !remembered.token.is_empty() {
            app.standing.base = remembered.base.clone();
            app.standing.name = crate::server::plain_name(&remembered.name);
            app.ask(crate::server::Ask::Resume {
                base: remembered.base,
                token: remembered.token,
                reachable_at: remembered.reachable_at,
            });
        } else {
            // A copy with no server behind it — a laptop, a demo zip, the first
            // machine in a shop. It still asks once at start-up whether there
            // is a newer build, because a seat that only learns about fixes
            // after somebody sets a server up is a seat that never learns about
            // them. Signing in later asks again, of the server, whose answer
            // wins from then on.
            app.ask(crate::server::Ask::CheckForUpdate {
                running: env!("CARGO_PKG_VERSION").to_string(),
                channel: app.prefs.channel,
                asked: false,
            });
        }
        for path in open {
            match path.to_str().filter(|p| crate::deskui::is_link(p)) {
                // A link needs the office server, and the session kept from
                // last time is still being checked: it waits for that.
                Some(link) => app.links_waiting.push((link.to_string(), std::time::Instant::now())),
                None => app.take_file(path),
            }
        }
        app
    }

    /// The View menu's ticks and the toolbar buttons follow the saved
    /// settings, so a setting that starts on (snapping does) shows as on,
    /// and clicking it turns it off rather than the tick and the setting
    /// going opposite ways.
    fn show_view_settings(&mut self) {
        let settings = [
            ("View.Rulers", self.prefs.rulers),
            ("View.Crosshair", self.prefs.crosshair),
            ("View.MarkupText", self.prefs.markup_text),
            ("View.ShowGrid", self.prefs.grid),
            ("View.SnapToGrid", self.prefs.snap_to_grid),
            ("View.SnapToContent", self.prefs.snap_to_content),
            ("View.SnapToMarkup", self.prefs.snap_to_markup),
        ];
        for (id, on) in settings {
            self.chrome.set(id, on);
        }
    }

    pub fn doc(&self) -> Option<&Doc> {
        self.docs.get(self.current)
    }

    pub fn doc_mut(&mut self) -> Option<&mut Doc> {
        self.docs.get_mut(self.current)
    }

    /// Opens a drawing set in a tab. A set already open is brought forward
    /// rather than opened twice — six people sending each other the same
    /// drawing all morning should not end up with six copies of it.
    pub fn open(&mut self, path: PathBuf) {
        if let Some(at) = self.docs.iter().position(|d| d.path == path) {
            self.current = at;
            self.status = format!("{} is already open.", name_of(&path));
            return;
        }
        let author = self.author.clone();
        if let Some(doc) = self.doc_mut() {
            let _ = doc.save(&author);
        }
        self.error = None;
        self.opening = true;
        self.status = format!("Opening {}…", name_of(&path));
        let doc = self.next_doc_id;
        self.next_doc_id += 1;
        self.pending_open = Some(doc);
        // Remembered before it opens rather than after, so a set that turns out
        // to be password-protected is still in the list — which is where
        // somebody will go to try it again once they have the password.
        self.prefs.opened(&path);
        let _ = self.prefs.save();
        let password = self.passwords.get(&path).cloned().unwrap_or_default();
        self.opening_path = Some(path.clone());
        self.svc.send(ToWorker::Open { doc, path, password });
    }


    /// Asks for the password on a locked drawing set.
    ///
    /// Modal on purpose: there is nothing useful to do with a set that has not
    /// opened, and a box somebody can click behind and lose is worse than one
    /// they have to answer.
    pub fn password_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut asking) = self.asking_password.take() else {
            return;
        };
        let mut open = true;
        let mut try_it = false;
        let mut give_up = false;
        egui::Window::new("This drawing set is locked")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(&asking.name).strong());
                ui.add_space(6.0);
                ui.label("It needs the password it was locked with. Either the one for opening it or the owner's will do.");
                ui.add_space(10.0);
                let box_ = ui.add(
                    egui::TextEdit::singleline(&mut asking.typed)
                        .password(true)
                        .hint_text("Password")
                        .desired_width(f32::INFINITY),
                );
                box_.request_focus();
                if box_.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    try_it = true;
                }
                if asking.wrong {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("That password was not accepted.")
                            .color(self.chrome.theme.warn)
                            .size(11.0),
                    );
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Open").clicked() {
                        try_it = true;
                    }
                    if ui.button("Cancel").clicked() {
                        give_up = true;
                    }
                });
                ui.add_space(4.0);
                // Said here rather than discovered later, because somebody
                // about to mark up a locked set should know before they start.
                ui.label(
                    egui::RichText::new(
                        "It opens like any other drawing set once it is unlocked, and \
                         anything you draw on it is locked the same way when you save.",
                    )
                    .weak()
                    .size(11.0),
                );
            });

        if give_up || !open {
            self.status = format!("{} was not opened.", asking.name);
            return;
        }
        if try_it {
            if asking.typed.is_empty() {
                asking.wrong = true;
                self.asking_password = Some(asking);
                return;
            }
            let path = asking.path.clone();
            self.passwords.insert(path.clone(), asking.typed.clone());
            self.open(path);
            return;
        }
        self.asking_password = Some(asking);
    }

    /// Reads a drawing again from disk, after something changed it underneath.
    ///
    /// Used where a change goes into the file rather than into the markups —
    /// turning a layer off, writing bookmarks — because pdfium reads those
    /// when it opens a document and nowhere else.
    pub fn reopen(&mut self, path: &std::path::Path) {
        let Some(at) = self.docs.iter().position(|d| d.path == path) else {
            return;
        };
        let was_on = self.docs[at].page;
        let view = self.docs[at].view.clone();
        self.close_tab(at);
        self.open(path.to_path_buf());
        self.reopen_to = Some((path.to_path_buf(), was_on, view));
    }

    /// Closes a tab, saving anything on it first.
    pub fn close_tab(&mut self, at: usize) {
        if at >= self.docs.len() {
            return;
        }
        let author = self.author.clone();
        let mut doc = self.docs.remove(at);
        // Markups are never lost to a closed tab, whatever else happens.
        if doc.dirty && !doc.read_only {
            match doc.save(&author) {
                Ok(n) if n > 0 => {
                    self.status = format!("Saved {n} markup(s) into {}.", name_of(&doc.path))
                }
                Err(e) => self.error = Some(format!("Could not save before closing: {e}")),
                _ => {}
            }
        }
        self.svc.send(ToWorker::Close(doc.id));
        if self.current >= self.docs.len() {
            self.current = self.docs.len().saturating_sub(1);
        }
        // Whatever the search found in a drawing nobody is looking at any more
        // is not an answer any more either.
        self.search.clear();
    }

    /// Puts the drawing in front on the window's title bar.
    pub fn retitle(&self, ctx: &egui::Context) {
        let version = env!("CARGO_PKG_VERSION");
        let title = match self.doc() {
            Some(doc) => format!("{} — Excalibur View {version}", name_of(&doc.path)),
            None => format!("Excalibur View {version}"),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    pub fn pick_and_open(&mut self) {
        self.filing.one(
            crate::files::Choose::open()
                .filter("Drawing sets (PDF)", &["pdf", "PDF"])
                .filter("Models (IFC)", &["ifc", "IFC"]),
            |app, path| app.take_file(path),
        );
    }

    /// Reads a Revu profile and takes its tools, columns and unit weights.
    ///
    /// A file picker rather than a folder somebody has to copy things into.
    /// Every company that buys this has its own chest, built over years, and
    /// "put it in a folder beside the program" is a sentence that loses people
    /// before they have seen anything the program does.
    ///
    /// The chosen file is copied next to the program so it loads again next
    /// time. The original is left where it was: somebody's chest lives in their
    /// own folders and this is not the program to start moving it about.
    pub fn pick_and_load_chest(&mut self) {
        self.filing.one(
            crate::files::Choose::open()
                .filter("Tool chests", &[chest::native::EXTENSION, "bpx", "btx", "BPX", "BTX"])
                .filter("Excalibur View tool chest", &[chest::native::EXTENSION])
                .filter("Import from Revu (profile or tool set)", &["bpx", "btx", "BPX", "BTX"])
                .title("Load a tool chest"),
            |app, path| app.load_chest_from(&path),
        );
    }

    /// Loads a tool chest file: chosen with Load Tool Chest, or dropped on
    /// the window.
    pub fn load_chest_from(&mut self, path: &std::path::Path) {
        let path = path.to_path_buf();
        let Ok(bytes) = std::fs::read(&path) else {
            self.error = Some(format!("{} could not be read.", path.display()));
            return;
        };
        let Some(profile) = chest::Profile::read(&bytes) else {
            self.error = Some(format!(
                "{} is not a tool chest this can read. It should be an Excalibur View tool chest \
                 (.evtools), or a Revu profile (.bpx) or tool set (.btx) to import.",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
            return;
        };

        let tools: usize = profile.sets.iter().map(|s| s.tools.len()).sum();
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "tool chest".into());
        // Whatever it came in as, it is kept as Excalibur View's own file: a
        // Revu file is read once, here, and never needed again.
        let native = chest::native::is_native(&bytes);
        let stem = path
            .file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| profile.name.clone());
        let name = format!("{stem}.{}", chest::native::EXTENSION);
        let kept_bytes = if native { bytes } else { chest::native::write(&profile, &format!("Revu: {file_name}")) };

        // Kept, so it is there next time. A failure here is worth saying out
        // loud but is not worth refusing the chest over — it works for this
        // session either way.
        let beside = crate::server::profiles_folder();
        let kept = std::fs::create_dir_all(&beside).is_ok()
            && std::fs::write(beside.join(&name), &kept_bytes).is_ok();

        // Put together with everything else this seat has.
        self.profile = if kept {
            crate::profiles::load().or(Some(profile))
        } else {
            Some(profile)
        };
        self.status = match (kept, native) {
            (true, true) => format!("{name}: {tools} tools loaded. It will load again next time."),
            (true, false) => format!(
                "{file_name}: {tools} tools imported from Revu and kept as {name}, Excalibur View's \
                 own tool chest. The Revu file is not needed again."
            ),
            (false, _) => format!(
                "{file_name}: {tools} tools loaded, but they could not be saved beside the program \
                 — you will have to load them again next time."
            ),
        };
    }

    /// Saves the tools this seat has as one Excalibur View tool chest: to
    /// hand to somebody, or to keep.
    pub fn save_chest_as(&mut self) {
        let Some(profile) = self.profile.clone() else {
            self.status = "There is no tool chest loaded to save.".into();
            return;
        };
        self.filing.one(
            crate::files::Choose::save(format!(
                "{}.{}",
                crate::server::safe(&profile.name),
                chest::native::EXTENSION
            ))
            .filter("Excalibur View tool chest", &[chest::native::EXTENSION])
            .title("Save the tool chest"),
            move |app, path| match std::fs::write(&path, chest::native::write(&profile, "")) {
                Ok(()) => {
                    app.status = format!("Saved {} tools to {}.", profile.tool_count(), path.display())
                }
                Err(e) => app.error = Some(format!("{}: {e}", path.display())),
            },
        );
    }

    pub fn go_to(&mut self, page: u32) {
        // Worth remembering only when this is really going somewhere: a jump
        // to the sheet already showing is not a place you can come back from.
        let worth_remembering = self
            .doc()
            .map(|d| (page as usize) < d.pages.len() && d.page != page)
            .unwrap_or(false);
        if worth_remembering {
            self.remember_place();
        }
        let Some(doc) = self.docs.get_mut(self.current) else {
            return;
        };
        if page as usize >= doc.pages.len() || page == doc.page {
            return;
        }
        let was = doc.size();
        let now = doc.pages[page as usize];
        doc.page = page;
        doc.draft = None;
        doc.selected = None;
        doc.sent.clear();
        let id = doc.id;
        let wanted = !doc.previews.contains_key(&page);
        // Flipping between sheets of the same size holds the zoom and position,
        // so the same corner of the building stays under the cursor.
        if (was.width - now.width).abs() > 0.5 || (was.height - now.height).abs() > 0.5 {
            doc.view.fit_requested = true;
        }
        if wanted {
            self.svc.send(ToWorker::Preview { doc: id, page });
        }
    }

    fn drain(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.svc.rx.try_recv() {
            match msg {
                FromWorker::Extracted { job, to, pages } => self.sheets_ready_to_print(job, to, pages),
                FromWorker::DocDone { job, done } => self.document_done(job, done),
                FromWorker::DocFailed { job, why } => self.document_failed(job, why),
                FromWorker::Compared { job, found } => self.comparison_done(job, *found),
                FromWorker::CompareFailed { job, why } => {
                    self.comparison_failed(job, why.clone());
                    if let Some(overlaying) = self.overlaying.as_mut() {
                        overlaying.running = false;
                        overlaying.error = Some(why);
                    }
                }
                FromWorker::BatchLine { job, line, of } => self.batch_line(job, *line, of),
                FromWorker::BatchDone { job } => self.batch_finished(job),
                FromWorker::Overlaid { job, image, said } => {
                    self.overlay_done(job, *image, said, ctx)
                }
                FromWorker::ExtractFailed { job, why } => self.printing_failed(job, why),
                FromWorker::PrintProgress { job, done, of } => self.print_progress(job, done, of),
                FromWorker::Read { job, page, reading } => self.plugin_read(job, page, *reading),
                FromWorker::Geometry { doc, page, geometry, millis } => {
                    if let Some(bench) = self.bench.as_mut() {
                        bench.lines_read(geometry.segments.len(), millis as f64);
                    }
                    log::debug!(
                        "sheet {page}: {} lines read for snapping in {millis} ms",
                        geometry.segments.len()
                    );
                    if let Some(open) = self.docs.iter_mut().find(|d| d.id == doc && !d.reshuffled) {
                        open.geometry.insert(page, geometry);
                        // The sheet in hand and a few either side; line-work
                        // for a hundred sheets is a lot of memory for nothing.
                        if open.geometry.len() > 4 {
                            let here = open.page as i64;
                            let far: Vec<u32> = {
                                let mut pages: Vec<u32> = open.geometry.keys().copied().collect();
                                pages.sort_by_key(|p| std::cmp::Reverse((*p as i64 - here).abs()));
                                pages.into_iter().take(open.geometry.len() - 4).collect()
                            };
                            for p in far {
                                open.geometry.remove(&p);
                                open.geometry_asked.remove(&p);
                            }
                        }
                    }
                }
                FromWorker::Printed { job, sheets, printer } => self.printed(job, sheets, printer),
                FromWorker::PrintFailed { job, why } => self.printing_failed(job, why),
                FromWorker::LibraryFailed(why) => {
                    self.library_error = Some(why);
                    self.opening = false;
                }
                FromWorker::OpenFailed {
                    doc,
                    why,
                    wants_password,
                } => {
                    let path = self.opening_path.clone();
                    if self.pending_open == Some(doc) {
                        self.pending_open = None;
                        self.opening = false;
                    }
                    match (wants_password, path) {
                        // Not an error yet: the set is locked and nobody has
                        // typed the password. Asking is the whole answer.
                        (true, Some(path)) => {
                            let tried = self.passwords.remove(&path).is_some();
                            let name = name_of(&path);
                            self.status = format!("{name} is locked.");
                            self.asking_password = Some(AskPassword {
                                path,
                                name,
                                typed: String::new(),
                                wrong: tried,
                            });
                        }
                        _ => {
                            self.error = Some(why);
                            self.status = "Could not open that file.".into();
                        }
                    }
                }
                FromWorker::Opened {
                    doc: id,
                    path,
                    pages,
                    millis,
                } => {
                    // Open the same file at object level. The renderer draws
                    // pixels; this is what reads and writes the markups.
                    let password = self.passwords.get(&path).cloned().unwrap_or_default();
                    let mut doc = match Doc::open_with(path.clone(), &password) {
                        Ok(doc) => doc,
                        Err(crate::sheet::Shut::WantsPassword(name)) => {
                            // Pdfium opened it and this did not, which means
                            // the two disagree about the file. Rare, and worth
                            // saying plainly rather than showing half a set.
                            self.error = Some(format!(
                                "{name} opened for drawing but not for its markups. \
                                 The file may be damaged."
                            ));
                            self.opening = false;
                            continue;
                        }
                        Err(crate::sheet::Shut::Trouble(e)) => {
                            self.error = Some(e);
                            self.opening = false;
                            continue;
                        }
                    };
                    doc.open_millis = millis;
                    doc.drawing = self.prefs.drawing();
                    if doc.read_only {
                        self.status = format!(
                            "{} is read-only. You can mark it up, but saving needs Save As.",
                            name_of(&doc.path)
                        );
                    } else if let Some(lock) = doc.locked {
                        self.status = format!(
                            "{} is locked ({}). Markups saved into it are locked the same way.",
                            name_of(&doc.path),
                            if lock.weak() { "weakly" } else { "AES-256" }
                        );
                    }
                    // The renderer's page sizes are authoritative for what is
                    // on screen, so take those where they agree in count.
                    if pages.len() == doc.pages.len() {
                        doc.pages = pages;
                    }
                    self.status = format!(
                        "{} · {} sheets · opened in {millis} ms",
                        name_of(&path),
                        doc.pages.len()
                    );
                    self.opening = false;
                    let found = doc.marks.len();
                    let scaled = doc.scaled_pages();
                    if found > 0 {
                        self.status = format!(
                            "{} · {} sheets · {found} markups · {scaled} sheets with a scale",
                            name_of(&doc.path),
                            doc.pages.len()
                        );
                    }
                    doc.id = id;
                    // A drawing that came off a server keeps the thread back
                    // to it, so its markups go up as well as into the file.
                    if let Some((from, set)) = self.opened_from_server.take() {
                        if from == doc.path {
                            doc.attached = Some(crate::sync::Attached {
                                set,
                                revision: 0,
                                sent: doc.names(),
                            });
                        } else {
                            self.opened_from_server = Some((from, set));
                        }
                    }
                    let attached = doc.attached.is_some();
                    // A drawing that was reopened because something changed
                    // the file underneath — a layer turned off, bookmarks
                    // written — goes back to where it was, or somebody loses
                    // their place in a forty-sheet set for a tick box.
                    if let Some((wanted, page, view)) = self.reopen_to.take() {
                        if wanted == doc.path {
                            doc.page = page.min(doc.pages.len().saturating_sub(1) as u32);
                            doc.view = view;
                        } else {
                            self.reopen_to = Some((wanted, page, view));
                        }
                    }
                    self.docs.push(doc);
                    self.current = self.docs.len() - 1;
                    if self.pending_open == Some(id) {
                        self.pending_open = None;
                    }
                    self.svc.send(ToWorker::Preview { doc: id, page: 0 });
                    self.svc.send(ToWorker::ScanLabels(id));
                    self.retitle(ctx);
                    // Catch up with whatever the office has done to it since.
                    if attached {
                        self.sync_now();
                    }
                }
                FromWorker::Paragraphs { doc: id, page, found } => {
                    if let Some(doc) = self.docs.iter_mut().find(|d| d.id == id && !d.reshuffled) {
                        match found {
                            Ok(found) => {
                                doc.paragraphs.insert(page, found);
                            }
                            Err(why) => {
                                doc.paragraphs.insert(page, Vec::new());
                                log::warn!("could not read the words on sheet {}: {why}", page + 1);
                            }
                        }
                    }
                }
                FromWorker::Rewritten { doc, job, page, result } => self.rewritten(doc, job, page, result),
                FromWorker::Refreshed { doc: id, generation } => {
                    if let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) {
                        doc.refreshes_owed = doc.refreshes_owed.saturating_sub(1);
                        // Once the renderer has read every change asked of
                        // it, what it sends is for the sheets as they are.
                        if doc.reshuffled && doc.refreshes_owed == 0 {
                            doc.reshuffled = false;
                            if std::mem::take(&mut doc.relabel) {
                                self.svc.send(ToWorker::ScanLabels(id));
                            }
                            self.svc.send(ToWorker::Preview { doc: id, page: doc.page });
                        }
                        doc.fresh_from = generation;
                        let pages = std::mem::take(&mut doc.redrawing);
                        doc.tiles.mark_stale(&pages);
                        // The words on those sheets are read again when next wanted.
                        for page in &pages {
                            doc.paragraphs.remove(page);
                            doc.paragraphs_asked.remove(page);
                        }
                        // The thumbnails too, which show the markups.
                        for page in &pages {
                            doc.thumbs.remove(page);
                            doc.asked.remove(page);
                            doc.previews.remove(page);
                        }
                    }
                }
                FromWorker::Tile { key, image, millis, generation } => {
                    self.last_tile_millis = millis;
                    if let Some(bench) = self.bench.as_mut() {
                        bench.tile(millis as f64);
                    }
                    // To whichever tab it belongs, not to whichever is in
                    // front: a tile that arrives while somebody is switching
                    // tabs belongs to the drawing that asked for it.
                    if let Some(doc) = self.docs.iter_mut().find(|d| d.id == key.doc && !d.reshuffled) {
                        let handle = ctx.load_texture(
                            format!("t{}-{}-{}-{}-{}", key.doc, key.page, key.bucket, key.tx, key.ty),
                            image,
                            egui::TextureOptions::LINEAR,
                        );
                        let current = generation >= doc.fresh_from;
                        doc.tiles.insert_drawn(key, handle, current);
                    }
                }
                FromWorker::Preview { doc: id, page, scale, image } => {
                    if let Some(doc) = self.docs.iter_mut().find(|d| d.id == id && !d.reshuffled) {
                        let handle = ctx.load_texture(
                            format!("p{id}-{page}"),
                            image,
                            egui::TextureOptions::LINEAR,
                        );
                        doc.previews.insert(page, (scale, handle));
                        if doc.previews.len() > 8 {
                            let keep = doc.page;
                            let drop: Vec<u32> = doc
                                .previews
                                .keys()
                                .copied()
                                .filter(|p| p.abs_diff(keep) > 3)
                                .collect();
                            for p in drop {
                                doc.previews.remove(&p);
                            }
                        }
                    }
                }
                FromWorker::Thumb { doc: id, page, image } => {
                    if let Some(doc) = self.docs.iter_mut().find(|d| d.id == id && !d.reshuffled) {
                        let handle = ctx.load_texture(
                            format!("h{id}-{page}"),
                            image,
                            egui::TextureOptions::LINEAR,
                        );
                        doc.thumbs.insert(page, handle);
                    }
                }
                FromWorker::Sightings { doc: id, job, page, places, done } => {
                    if self.doc().map(|d| d.id) == Some(id) {
                        self.looking.accept(job, page, places, done);
                    }
                }
                FromWorker::Filled { doc: id, job, outcome } => {
                    if self.doc().map(|d| d.id) == Some(id) {
                        self.filling.accept(job, outcome);
                    }
                }
                FromWorker::Found { doc: id, job, page, hits, done } => {
                    // Answers for a drawing that is no longer in front belong
                    // to a search nobody is looking at.
                    if self.doc().map(|d| d.id) == Some(id) {
                        self.search.accept(job, page, hits, done);
                    }
                }
                FromWorker::Labels { doc: id, first, labels } => {
                    if let Some(doc) = self.docs.iter_mut().find(|d| d.id == id && !d.reshuffled) {
                        let count = labels.len() as u32;
                        for (i, label) in labels.into_iter().enumerate() {
                            let at = first as usize + i;
                            if at < doc.labels.len() {
                                doc.labels[at] = label;
                            }
                        }
                        doc.labelled = doc.labelled.max(first + count);
                    }
                }
            }
        }
    }
}

/// Whoever is sitting at the machine, for the Author column on a markup.
pub fn whoami() -> String {
    for key in ["HYPERVIEW_AUTHOR", "USERNAME", "USER"] {
        if let Ok(name) = std::env::var(key) {
            if !name.trim().is_empty() {
                return name;
            }
        }
    }
    "Unknown".into()
}

/// Turns a shortcut's key name into the key egui reports.
/// Whether the keys belong to a text field rather than to the drawing.
///
/// This used to ask `wants_keyboard_input`, which is a different and much
/// wider question: it is true whenever *anything* holds keyboard focus, and
/// a button keeps focus after it is clicked. So pressing a tool in the
/// toolbar, or any button in any panel, switched off every shortcut in the
/// program until somebody happened to click the sheet again -- copy, paste,
/// save, delete, the arrow keys, page up and down, find, every tool letter.
/// Nothing said so, which is why it read as shortcuts that were never wired
/// up rather than as shortcuts that had been turned off.
///
/// The narrow question is the right one: is the thing with focus a text
/// field? Only a text field keeps `TextEditState` under its id, so this asks
/// for it and believes the answer. While somebody is typing, Ctrl+C copies
/// their text and Delete deletes a character, which is what they mean; the
/// moment they leave the field the drawing has the keys back.
fn somebody_is_typing(ctx: &egui::Context) -> bool {
    let Some(focused) = ctx.memory(|m| m.focused()) else {
        return false;
    };
    egui::TextEdit::load_state(ctx, focused).is_some()
}

#[cfg(test)]
mod the_keys_belong_to_the_drawing {
    //! A button that has been clicked must not take the keyboard with it.
    //!
    //! This is the whole of a bug that made the program look like it had no
    //! shortcuts at all. It is checked against a real egui context rather
    //! than reasoned about, because the thing that was wrong was an
    //! assumption about what egui means by "wants keyboard input".

    use super::somebody_is_typing;

    /// Runs one pass over a context, laying out whatever `build` puts in it.
    ///
    /// `run` takes an `FnMut` because a pass can be repeated, so the closure
    /// inside it cannot consume anything. `build` is therefore an `FnMut`
    /// too, and the id it reports comes back through a cell rather than a
    /// return value.
    fn a_pass(ctx: &egui::Context, mut build: impl FnMut(&mut egui::Ui)) {
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, &mut build);
        });
    }

    #[test]
    fn a_button_holding_focus_does_not_take_the_keyboard() {
        let ctx = egui::Context::default();
        let mut id = None;
        a_pass(&ctx, |ui| {
            let button = ui.button("Length");
            button.request_focus();
            id = Some(button.id);
        });
        // Focus is settled on the next pass, the way it is in the program.
        a_pass(&ctx, |ui| {
            let _ = ui.button("Length");
        });
        assert_eq!(ctx.memory(|m| m.focused()), id, "the button should have focus");
        assert!(
            !somebody_is_typing(&ctx),
            "a focused button means somebody clicked a tool, not that they are typing"
        );
    }

    #[test]
    fn a_text_field_holding_focus_does_take_the_keyboard() {
        let ctx = egui::Context::default();
        let mut typed = String::new();
        let mut id = None;
        a_pass(&ctx, |ui| {
            let field = ui.text_edit_singleline(&mut typed);
            field.request_focus();
            id = Some(field.id);
        });
        a_pass(&ctx, |ui| {
            let _ = ui.text_edit_singleline(&mut typed);
        });
        assert_eq!(ctx.memory(|m| m.focused()), id, "the field should have focus");
        assert!(
            somebody_is_typing(&ctx),
            "Ctrl+C in a field somebody is typing in belongs to the field"
        );
    }

    #[test]
    fn with_nothing_focused_the_drawing_has_the_keys() {
        let ctx = egui::Context::default();
        a_pass(&ctx, |ui| {
            ui.label("a sheet");
        });
        assert!(!somebody_is_typing(&ctx));
    }
}

fn key_named(name: &str) -> Option<egui::Key> {
    use egui::Key;
    Some(match name {
        "A" => Key::A, "B" => Key::B, "C" => Key::C, "D" => Key::D, "E" => Key::E,
        "F" => Key::F, "G" => Key::G, "H" => Key::H, "I" => Key::I, "J" => Key::J,
        "K" => Key::K, "L" => Key::L, "M" => Key::M, "N" => Key::N, "O" => Key::O,
        "P" => Key::P, "Q" => Key::Q, "R" => Key::R, "S" => Key::S, "T" => Key::T,
        "U" => Key::U, "V" => Key::V, "W" => Key::W, "X" => Key::X, "Y" => Key::Y,
        "Z" => Key::Z,
        "0" => Key::Num0, "1" => Key::Num1, "2" => Key::Num2, "3" => Key::Num3,
        "4" => Key::Num4, "5" => Key::Num5, "6" => Key::Num6, "7" => Key::Num7,
        "8" => Key::Num8, "9" => Key::Num9,
        "F1" => Key::F1, "F2" => Key::F2, "F3" => Key::F3, "F4" => Key::F4,
        "F5" => Key::F5, "F6" => Key::F6, "F7" => Key::F7, "F8" => Key::F8,
        "F9" => Key::F9, "F10" => Key::F10, "F11" => Key::F11, "F12" => Key::F12,
        "Del" | "Delete" => Key::Delete,
        "Esc" | "Escape" => Key::Escape,
        "Enter" | "Return" => Key::Enter,
        "Space" => Key::Space,
        "Tab" => Key::Tab,
        _ => return None,
    })
}

pub fn name_of(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    // A set fetched from the office is kept as `<digest>-<its name>.pdf`; the
    // digest is for the cache, not for people.
    if path.parent() == Some(crate::server::cache_folder().as_path()) {
        if let Some(rest) = without_digest(&name) {
            return rest.to_string();
        }
    }
    name
}

/// `8c0028fc2425-S-101.pdf` → `S-101.pdf`.
pub(crate) fn without_digest(name: &str) -> Option<&str> {
    let (digest, rest) = name.split_once('-')?;
    (digest.len() == 12 && digest.bytes().all(|b| b.is_ascii_hexdigit()) && !rest.is_empty()).then_some(rest)
}

impl App {
    /// Writes markups out once the drawing has been left alone for a few
    /// seconds.
    ///
    /// Every save appends to the file — that is what makes the original bytes
    /// safe — so saving on a timer while somebody is still clicking would grow
    /// the drawing for no reason. It waits for quiet instead, and it says once
    /// when it cannot write rather than failing in silence every few seconds.
    /// Saves on the next frame rather than after the usual pause, for the end
    /// of something whole — a markup let go of after moving or resizing it,
    /// words finished being typed — so the sheet shows it where it is now.
    pub fn save_soon(&mut self) {
        let Some(doc) = self.doc() else { return };
        let count = doc.marks.len();
        let long_ago = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(60))
            .unwrap_or_else(std::time::Instant::now);
        self.settled = Some((long_ago, count));
    }

    /// Has every drawing whose markups were saved read again for drawing.
    fn refresh_saved(&mut self) {
        self.sheets_to_renderer();
        for doc in self.docs.iter_mut() {
            if doc.redraw.is_empty() {
                continue;
            }
            let pages = std::mem::take(&mut doc.redraw);
            doc.redrawing.extend(pages);
            doc.refreshes_owed += 1;
            self.svc.send(ToWorker::Refresh { doc: doc.id });
        }
    }

    fn autosave(&mut self) {
        const QUIET: std::time::Duration = std::time::Duration::from_secs(5);
        // Not in the middle of words being typed on the sheet. Finishing them
        // saves straight away.
        if self.editing_text.is_some() {
            return;
        }
        let Some(doc) = self.doc() else {
            self.settled = None;
            return;
        };
        if !doc.dirty {
            self.settled = None;
            return;
        }
        if doc.read_only {
            // Nothing to be done until Save As. The status bar already says so.
            return;
        }
        let count = doc.marks.len();
        let now = std::time::Instant::now();
        match self.settled {
            Some((since, was)) if was == count => {
                if now.duration_since(since) < QUIET {
                    return;
                }
            }
            _ => {
                self.settled = Some((now, count));
                return;
            }
        }
        self.settled = None;
        let author = self.author.clone();
        let Some(doc) = self.doc_mut() else { return };
        match doc.save(&author) {
            Ok(0) => {}
            Ok(n) => {
                self.last_save = std::time::Instant::now();
                self.autosave_complained = false;
                self.status = format!(
                    "Saved {n} markup{} into the drawing.",
                    if n == 1 { "" } else { "s" }
                );
                // Saved here, so it can go to everybody else now.
                if self.doc().is_some_and(|d| d.attached.is_some()) {
                    self.sync_quietly();
                }
            }
            Err(e) => {
                if !self.autosave_complained {
                    self.autosave_complained = true;
                    self.error = Some(format!("Could not save: {e}"));
                }
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let frame_started = std::time::Instant::now();
        let mut marks = self.bench.as_ref().map(|_| crate::bench::Marks::start());
        if self.bench.is_some() {
            self.drive_benchmark(ctx, frame_started);
        }
        self.drain(ctx);
        self.drain_server();
        self.drain_desk(ctx);
        crate::panelbody::set_user_columns(self.profile.as_ref());
        self.show_view_settings();
        crate::bench::mark(&mut marks, "messages");
        // Drawings double-clicked while this window was already open. They
        // open as tabs, and the window comes to the front to show them.
        let handed: Vec<crate::instance::Handed> = self.inbox.try_iter().collect();
        let (combine, handed): (Vec<_>, Vec<_>) = handed.into_iter().partition(|h| h.combine);
        if !combine.is_empty() {
            self.to_combine.extend(combine.into_iter().flat_map(|h| h.files));
            self.to_combine_at = Some(std::time::Instant::now());
        }
        self.combine_when_gathered(ctx);
        if !handed.is_empty() {
            for path in handed.into_iter().flat_map(|h| h.files) {
                // A hyperview:// link rides the same inbox as a drawing.
                match path.to_str().filter(|p| crate::deskui::is_link(p)) {
                    Some(link) => {
                        let link = link.to_string();
                        self.open_link(&link);
                    }
                    None => self.take_file(path),
                }
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            // The start that handed these over may have been a newer version
            // installing itself over this one.
            if self.standing.update_ready.is_none() {
                self.ask(crate::server::Ask::NoticeInstalled);
            }
        }
        // A window nobody is touching is not redrawn, so it is woken now and
        // then to keep the update check honest.
        let every = crate::server::update_interval(
            !self.standing.base.is_empty(),
            self.standing.update_looking,
        );
        ctx.request_repaint_after(every.min(std::time::Duration::from_secs(5 * 60)));
        if self.last_update_check.elapsed() > every {
            self.last_update_check = std::time::Instant::now();
            self.ask(crate::server::Ask::CheckForUpdate {
                running: env!("CARGO_PKG_VERSION").to_string(),
                channel: self.prefs.channel,
                asked: false,
            });
            // And whether the office has shared a new tool chest or plugin
            // since -- and what projects are on it. The panel keeps the list
            // current while it is open; this is the backstop for the seat
            // that had it closed, so a list can never be stale for longer
            // than it takes to look at it.
            if !self.standing.base.is_empty() {
                self.ask(crate::server::Ask::Chests);
                self.ask(crate::server::Ask::Plugins);
                self.ask(crate::server::Ask::Projects);
            }
        }
        self.take_dropped_files(ctx);
        self.keyboard(ctx);

        if let Some(why) = self.library_error.clone() {
            self.library_missing(ctx, &why);
            return;
        }

        self.menus(ctx);
        crate::bench::mark(&mut marks, "menus");
        self.update_box(ctx);
        self.tool_bars(ctx);
        crate::bench::mark(&mut marks, "toolbars");
        self.tabs(ctx);
        self.fill_bar(ctx);
        self.navigation(ctx);
        crate::bench::mark(&mut marks, "tabs and navigation");
        self.side_panels(ctx);
        crate::bench::mark(&mut marks, "side panels");
        // The markups list's bar is always there; open, it is the list.
        self.markups_bar(ctx);
        crate::bench::mark(&mut marks, "markups list");
        self.status_bar(ctx);
        crate::bench::mark(&mut marks, "status bar");
        self.canvas(ctx);
        crate::bench::mark(&mut marks, "sheet");
        self.run_commands(ctx);
        self.calibrate_dialog(ctx);
        self.text_dialog(ctx);
        self.signature_window(ctx);
        self.reword_window(ctx);
        self.prefs_dialog(ctx);
        self.sign_in_dialog(ctx);
        self.print_dialog(ctx);
        self.document_dialog(ctx);
        self.compare_dialog(ctx);
        self.overlay_dialog(ctx);
        self.batch_dialog(ctx);
        self.about_box(ctx);
        self.shortcut_list(ctx);
        self.recent_window(ctx);
        self.revert_dialog(ctx);
        self.fingerprint_window(ctx);
        self.repeat_dialog(ctx);
        self.undo_history(ctx);
        self.address_dialog(ctx);
        self.properties_window(ctx);
        self.password_dialog(ctx);
        self.pen_chooser(ctx);
        self.spell_window(ctx);
        self.depth_dialog(ctx);
        self.shop_window(ctx);
        self.model_windows(ctx);
        self.doubles_window(ctx);
        self.revision_window(ctx);
        self.plugin_windows(ctx);
        self.assistant_window(ctx);
        self.answer_file_questions(ctx);

        crate::bench::mark(&mut marks, "dialogs");
        self.autosave();
        self.refresh_saved();
        self.keep_in_step(ctx);
        crate::bench::mark(&mut marks, "autosave");
        if let Some(bench) = self.bench.as_mut() {
            bench.end_frame(frame_started.elapsed(), marks.take());
        }

        if let Some(error) = self.error.clone() {
            egui::Window::new("That did not work")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .max_width(560.0)
                .show(ctx, |ui| {
                    ui.label(error);
                    ui.add_space(8.0);
                    if ui.button("Close").clicked() {
                        self.error = None;
                    }
                });
        }
    }

    #[cfg(not(target_os = "ios"))]
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.closing();
    }

    #[cfg(target_os = "ios")]
    fn on_exit(&mut self) {
        self.closing();
    }
}

impl App {
    /// What happens when the window closes: the drawing in front is saved.
    fn closing(&mut self) {
        let author = self.author.clone();
        if let Some(doc) = self.doc_mut() {
            let _ = doc.save(&author);
        }
        self.svc.send(ToWorker::Quit);
    }
}

impl App {
    /// A drawing from the office's server fetches what everybody else has
    /// put on it every few seconds, so it works like a Studio session: their
    /// markups appear on your screen shortly after they draw them, without
    /// anybody saving, reopening or pressing anything.
    fn keep_in_step(&mut self, ctx: &egui::Context) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(8);
        const GIVE_UP: std::time::Duration = std::time::Duration::from_secs(45);
        if self.standing.base.is_empty() || !self.doc().is_some_and(|d| d.attached.is_some()) {
            return;
        }
        ctx.request_repaint_after(EVERY);
        if self.sync_asked.is_some_and(|at| at.elapsed() < GIVE_UP) {
            return;
        }
        if self.last_sync.elapsed() < EVERY {
            return;
        }
        self.last_sync = std::time::Instant::now();
        self.sync_quietly();
    }

    /// Does to the window what the benchmark says to, this frame.
    fn drive_benchmark(&mut self, ctx: &egui::Context, now: std::time::Instant) {
        use crate::bench::Action;
        ctx.request_repaint();
        if let Some(last) = self.bench_frame_at.replace(now) {
            if let Some(bench) = self.bench.as_mut() {
                bench.frame_interval(now - last);
            }
        }
        let sheets = self.doc().map(|d| d.pages.len() as u32).unwrap_or(0);
        let Some(bench) = self.bench.as_mut() else { return };
        if !bench.is_open() && sheets > 0 {
            bench.opened(sheets);
            // Measured with the Length tool in hand, the way a takeoff is
            // done, so snapping is part of every frame that is timed.
            self.tool = Tool::Length;
        }
        let middle = ctx.screen_rect().center();
        // What snapping costs where the pointer would be: a query at the
        // middle of the screen each frame, the way hovering does it.
        if let Some(doc) = self.doc() {
            if doc.geometry.contains_key(&doc.page) {
                let s = doc.view.to_sheet(middle);
                let reach = 10.0 / doc.view.zoom as f64;
                let started = std::time::Instant::now();
                let hit = self.snap([s[0] as f64, s[1] as f64], reach);
                let ms = started.elapsed().as_secs_f64() * 1000.0;
                if let Some(bench) = self.bench.as_mut() {
                    bench.snapped(ms, hit.is_some());
                }
            }
        }
        let Some(bench) = self.bench.as_mut() else { return };
        match bench.begin_frame(self.missing_tiles) {
            Action::Nothing => {}
            Action::Pan(by) => {
                if let Some(doc) = self.doc_mut() {
                    doc.view.offset += by;
                    doc.view.fit_requested = false;
                }
            }
            Action::Zoom(factor) => {
                if let Some(doc) = self.doc_mut() {
                    doc.view.zoom_at(middle, factor);
                }
            }
            Action::NextSheet => {
                if let Some(doc) = self.doc() {
                    let next = (doc.page + 1) % (doc.pages.len().max(1) as u32);
                    self.go_to(next);
                }
            }
            Action::Finish => {
                if let Some(bench) = self.bench.take() {
                    let written = bench.write();
                    self.status = match written {
                        Some(at) => format!("Benchmark written to {}.", at.display()),
                        None => "The benchmark could not be written down.".into(),
                    };
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn library_missing(&mut self, ctx: &egui::Context, why: &str) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                ui.heading("The PDF engine is missing");
                ui.add_space(8.0);
                // Named for the platform this is running on. Telling somebody
                // on a Mac to go and find pdfium.dll sends them looking for a
                // file that does not exist on their computer.
                let engine = if cfg!(target_os = "windows") {
                    "pdfium.dll"
                } else if cfg!(target_os = "macos") {
                    "libpdfium.dylib"
                } else {
                    "libpdfium.so"
                };
                ui.label(if cfg!(embedded_pdfium) {
                    format!(
                        "Excalibur View carries {engine} inside itself and writes it out the \
                         first time it opens a drawing, and that did not work here. What it \
                         tried is below."
                    )
                } else {
                    format!(
                        "This copy was built without {engine} inside it, so it needs one \
                         sitting beside the program. What it looked for is below."
                    )
                });
                ui.add_space(12.0);
                ui.collapsing("What it tried", |ui| {
                    ui.monospace(why);
                });
            });
        });
    }

    fn take_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        // PDFs let go over the Thumbnails list go into the drawing, in front
        // of the sheet they were let go on.
        let pdfs: Vec<PathBuf> = dropped.iter().filter(|p| crate::reorder::is_pdf(p)).cloned().collect();
        let front = self.doc().map(|d| d.id);
        let before = match (self.sheet_rows.as_ref(), crate::platform::pointer_now(ctx)) {
            (Some(rows), Some(at)) if self.panel == "Thumbnails" && Some(rows.doc) == front => {
                rows.before_at(at)
            }
            _ => None,
        };
        if let (Some(before), false) = (before, pdfs.is_empty()) {
            self.change_sheets(crate::reorder::Plan::Insert { files: pdfs.clone(), before });
            for path in dropped.into_iter().filter(|p| !pdfs.contains(p)) {
                self.take_file(path);
            }
            return;
        }
        // A drawing set is usually more than one file, so every PDF dropped
        // anywhere else opens. A tool chest dropped is loaded, and a license
        // file added, the same as choosing them from the menus.
        for path in dropped {
            self.take_file(path);
        }
    }

    /// Changes the sheets of the drawing in front: moves them, adds them,
    /// turns them or takes them out. Undo takes it back.
    pub fn change_sheets(&mut self, plan: crate::reorder::Plan) {
        let author = self.author.clone();
        let Some(doc) = self.doc_mut() else { return };
        match doc.change_sheets(&plan, &author) {
            Ok(said) => self.status = said,
            Err(why) => self.error = Some(why),
        }
        self.sheets_to_renderer();
    }

    /// Tells the renderer about drawings whose sheets changed, before it is
    /// asked for anything else about them.
    pub fn sheets_to_renderer(&mut self) {
        let front = self.doc().map(|d| d.id);
        let mut clear_search = false;
        for doc in self.docs.iter_mut() {
            if !doc.sheets_changed {
                continue;
            }
            doc.sheets_changed = false;
            doc.refreshes_owed += 1;
            self.svc.send(ToWorker::Refresh { doc: doc.id });
            clear_search |= Some(doc.id) == front;
        }
        // A search's answers are sheets by their place in the set, and the
        // places just changed.
        if clear_search {
            self.search.clear();
        }
    }

    /// What a file means, wherever it arrived from — dropped on the window,
    /// double-clicked in Explorer, or handed over by a second start. One
    /// place, so a license does the same thing however it reaches the
    /// program.
    pub fn take_file(&mut self, path: PathBuf) {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        match extension.as_str() {
            "evtools" | "bpx" | "btx" => self.load_chest_from(&path),
            "evlicense" if crate::edition::sells() => self.take_license(path),
            "evlicense" => self.status = crate::edition::not_in_this_copy("Adding a license"),
            "ifc" => self.open_model(path),
            _ => self.open(path),
        }
    }

    /// A license, from wherever. If this is already the office server's
    /// administrator it goes on now. If not, it is kept and Studio opens:
    /// whoever was sent the file should not have to know that a license
    /// belongs to a server, or come back and find it again afterwards.
    pub fn take_license(&mut self, path: PathBuf) {
        let admin = self.standing.who.as_ref().map(|w| w.role) == Some(hub::Role::Admin);
        if self.standing.signed_in() && admin {
            self.ask(crate::server::Ask::AddLicense(path));
            return;
        }
        if self.standing.signed_in() {
            self.error = Some(
                "A license goes on the office server, and only an administrator can add one. \
                 Ask whoever set the server up to open this file on their computer."
                    .into(),
            );
            return;
        }
        self.waiting_license = Some(path);
        self.status = "Sign in as an administrator and this license goes on by itself.".into();
        self.chrome.fire("Server.Connect");
    }

    /// Puts a license kept from earlier on the server, now that somebody with
    /// the standing to do it has signed in.
    pub fn apply_waiting_license(&mut self) {
        let admin = self.standing.who.as_ref().map(|w| w.role) == Some(hub::Role::Admin);
        if !admin {
            return;
        }
        if let Some(path) = self.waiting_license.take() {
            self.ask(crate::server::Ask::AddLicense(path));
        }
    }

    /// Moves what is selected by a distance on the sheet, as one undo step.
    pub fn nudge_selection(&mut self, by: [f64; 2]) {
        let Some(doc) = self.doc_mut() else { return };
        let picked: Vec<usize> = doc
            .selection()
            .into_iter()
            .filter(|i| !crate::view::locked(&doc.marks[*i].markup))
            .collect();
        if picked.is_empty() {
            return;
        }
        let frame = doc.frame();
        let (a, b) = (frame.to_pdf([0.0, 0.0]), frame.to_pdf(by));
        doc.checkpoint_named("Nudge");
        for i in picked {
            doc.marks[i].markup.move_by(b[0] - a[0], b[1] - a[1]);
            doc.marks[i].changed = true;
        }
        doc.dirty = true;
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        if somebody_is_typing(ctx) {
            return;
        }
        let (keys, ctrl, shift, alt) = ctx.input(|i| {
            (
                i.keys_down.clone(),
                i.modifiers.command,
                i.modifiers.shift,
                i.modifiers.alt,
            )
        });
        let pressed = |key| ctx.input(|i| i.key_pressed(key));
        let _ = keys;

        if ctrl && pressed(egui::Key::O) {
            self.pick_and_open();
        }
        if pressed(egui::Key::Escape) {
            self.lasso = None;
            if let Some(doc) = self.doc_mut() {
                doc.draft = None;
                doc.choose(None);
            }
            self.calibrating = None;
            self.painting_format = None;
        }
        if self.tool.is_filling() {
            if pressed(egui::Key::Enter) {
                self.apply_fill();
            }
            if pressed(egui::Key::Escape) {
                self.filling.clear();
            }
        }
        if ctrl && pressed(egui::Key::Num2) {
            if let Some(doc) = self.doc_mut() {
                let want = if shift || doc.split == crate::sheet::Split::Vertical {
                    crate::sheet::Split::None
                } else {
                    crate::sheet::Split::Vertical
                };
                doc.set_split(want);
            }
        }
        if ctrl && pressed(egui::Key::H) {
            if let Some(doc) = self.doc_mut() {
                let want = if doc.split == crate::sheet::Split::Horizontal {
                    crate::sheet::Split::None
                } else {
                    crate::sheet::Split::Horizontal
                };
                doc.set_split(want);
            }
        }
        // F6 moves the work to the other pane, the way it moves between panes
        // in every program that has them.
        if pressed(egui::Key::F6) {
            if let Some(doc) = self.doc_mut() {
                doc.switch_pane();
            }
        }
        if ctrl && pressed(egui::Key::W) {
            let at = self.current;
            self.close_tab(at);
            self.retitle(ctx);
        }
        if ctrl && pressed(egui::Key::Tab) && !self.docs.is_empty() {
            let next = if shift {
                (self.current + self.docs.len() - 1) % self.docs.len()
            } else {
                (self.current + 1) % self.docs.len()
            };
            self.switch_to(next);
            self.retitle(ctx);
        }
        if ctrl && pressed(egui::Key::F) {
            self.panel = "Search".into();
        }
        if pressed(egui::Key::F3) {
            let to = if shift {
                self.search.previous()
            } else {
                self.search.next()
            };
            self.go_to_hit(to);
        }
        // Ctrl+Z is not handled here: Edit.Undo carries it in the command
        // registry, and having it in both places undid two steps at a time.
        if ctrl && shift && pressed(egui::Key::S) {
            self.save_as();
        } else if ctrl && pressed(egui::Key::S) {
            self.save_now();
        }
        if pressed(egui::Key::Delete) || pressed(egui::Key::Backspace) {
            self.delete_selected();
        }
        // The arrow keys nudge what is selected: a point at a time, ten with
        // Shift. Each press is one step on the Undo list.
        if !ctrl && !alt {
            let step = if shift { 10.0 } else { 1.0 };
            let nudge = [
                (egui::Key::ArrowLeft, [-step, 0.0]),
                (egui::Key::ArrowRight, [step, 0.0]),
                (egui::Key::ArrowUp, [0.0, -step]),
                (egui::Key::ArrowDown, [0.0, step]),
            ]
            .into_iter()
            .find(|(k, _)| pressed(*k))
            .map(|(_, by)| by);
            if let Some(by) = nudge {
                self.nudge_selection(by);
            }
        }
        let page = self.doc().map(|d| d.page);
        if let Some(page) = page {
            if pressed(egui::Key::PageDown) {
                self.go_to(page + 1);
            }
            if pressed(egui::Key::PageUp) && page > 0 {
                self.go_to(page - 1);
            }
        }
        if let Some(doc) = self.doc_mut() {
            if ctrl && pressed(egui::Key::Num0) {
                doc.view.fit_requested = true;
            }
            if ctrl && pressed(egui::Key::Num1) {
                doc.view.zoom = 1.0;
            }
        }
        // Everything else comes from the command registry, so a command's
        // shortcut is its shortcut — one place decides, the tooltip and the
        // keyboard cannot disagree, and adding a command gets it a working key
        // rather than a line in a table somebody has to remember to update.
        for (keys, id) in ui::command::shortcuts() {
            if keys.ctrl != ctrl || keys.shift != shift || keys.alt != alt {
                continue;
            }
            let Some(key) = key_named(keys.key) else { continue };
            if pressed(key) {
                self.chrome.fire(id);
                if let Some(tool) = crate::panelbody::tool_for(id) {
                    self.chrome.tool = id.to_string();
                    let _ = tool;
                }
            }
        }
    }

    pub fn delete_selected(&mut self) {
        let Some(doc) = self.doc_mut() else { return };
        let picked = doc.selection();
        if picked.is_empty() {
            return;
        }
        doc.checkpoint_named(&format!(
            "Delete {} markup{}",
            picked.len(),
            if picked.len() == 1 { "" } else { "s" }
        ));
        for index in &picked {
            if let Some(mark) = doc.marks.get_mut(*index) {
                mark.gone = true;
            }
        }
        doc.choose(None);
        doc.dirty = true;
        self.status = format!(
            "{} markup{} deleted.",
            picked.len(),
            if picked.len() == 1 { "" } else { "s" }
        );
        self.save_soon();
    }
}
