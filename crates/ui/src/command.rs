//! Every command the program can put on a toolbar or a menu.
//!
//! The identifiers are Bluebeam's, because his profile names them that way and
//! a profile Hyperview reads has to find every button it asks for. The labels,
//! hints and shortcuts are ours.
//!
//! Where a hint reads like an explanation rather than a name, that is on
//! purpose: a toolbar of forty unlabelled squares is only usable if hovering
//! one tells you plainly what it does.

use crate::icon::{self, Glyph};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Does something once.
    Button,
    /// Stays pressed: a tool, or a setting.
    Toggle,
    /// Opens a small chooser.
    DropDown,
    /// A box with a list, such as the scale or the font.
    Combo,
    /// Shows something without being clickable.
    Label,
    /// A gap between groups.
    Separator,
}

#[derive(Clone, Copy, Debug)]
pub struct Command {
    pub id: &'static str,
    pub label: &'static str,
    pub shortcut: Option<&'static str>,
    pub glyph: Option<Glyph>,
    pub kind: Kind,
    pub hint: &'static str,
}

impl Command {
    /// What to show when the pointer rests on the button.
    pub fn tooltip(&self) -> String {
        match (self.label.is_empty(), self.shortcut) {
            (true, _) => self.hint.to_string(),
            (false, Some(keys)) => format!("{}  ({keys})\n{}", self.label, self.hint),
            (false, None) => format!("{}\n{}", self.label, self.hint),
        }
    }

    /// True for the tools that stay selected once picked.
    pub fn is_tool(&self) -> bool {
        self.kind == Kind::Toggle
            && (self.id.starts_with("Markup.")
                || self.id.starts_with("Measure.")
                || self.id.starts_with("Form.")
                || matches!(self.id, "Select" | "Pan" | "Zoom" | "Lasso" | "Snapshot" | "Eraser" | "Space.Add"))
    }
}

pub fn find(id: &str) -> Option<&'static Command> {
    ALL.iter().find(|c| c.id == id)
}

/// The label to show, falling back to the tail of the identifier so an unknown
/// command from somebody else's profile still appears rather than vanishing.
pub fn label_of(id: &str) -> String {
    match find(id) {
        Some(c) if !c.label.is_empty() => c.label.to_string(),
        _ => id.rsplit('.').next().unwrap_or(id).to_string(),
    }
}

pub const ALL: &[Command] = &[
    Command { id: "Document.New", label: "New", shortcut: Some("Ctrl+N"), glyph: Some(icon::OPEN), kind: Kind::Button, hint: "Start an empty sheet." },
    Command { id: "File.CreatePDF", label: "Create PDF", shortcut: None, glyph: Some(icon::OPEN), kind: Kind::Button, hint: "Make a PDF from another kind of file." },
    Command { id: "File.CombinePDFs", label: "Combine", shortcut: None, glyph: Some(icon::COPY), kind: Kind::Button, hint: "Staple several files into one set." },
    Command { id: "File.Open", label: "Open", shortcut: Some("Ctrl+O"), glyph: Some(icon::OPEN), kind: Kind::Button, hint: "Open a drawing set." },
    Command { id: "Document.Save", label: "Save", shortcut: Some("Ctrl+S"), glyph: Some(icon::SAVE), kind: Kind::Button, hint: "Save markups into the file." },
    Command { id: "Document.SaveAll", label: "Save All", shortcut: Some("Shift+F2"), glyph: Some(icon::SAVE), kind: Kind::Button, hint: "Save every open set." },
    Command { id: "Document.Print", label: "Print", shortcut: Some("Ctrl+P"), glyph: Some(icon::PRINT), kind: Kind::Button, hint: "Print the sheet." },
    Command { id: "File.OpenModel", label: "Open Model…", shortcut: None, glyph: Some(icon::OPEN), kind: Kind::Button, hint: "Open a steel model (IFC) and total its tonnage by profile, from the weights the model itself gives." },
    Command { id: "File.LoadChest", label: "Load Tool Chest…", shortcut: None, glyph: Some(icon::CHEST), kind: Kind::Button, hint: "Load an Excalibur View tool chest (.evtools), or import the tools, columns and unit weights from a Revu profile (.bpx)." },
    Command { id: "File.SaveChest", label: "Save Tool Chest As…", shortcut: None, glyph: Some(icon::CHEST), kind: Kind::Button, hint: "Save the tools this computer has as one Excalibur View tool chest (.evtools), to keep or hand to somebody." },
    Command { id: "Document.Combine", label: "Combine", shortcut: None, glyph: Some(icon::COMBINE), kind: Kind::Button, hint: "Several drawing sets into one." },
    Command { id: "Document.RotatePages", label: "Rotate Pages", shortcut: None, glyph: Some(icon::ROTATE_RIGHT), kind: Kind::Button, hint: "Turn sheets that were scanned sideways." },
    Command { id: "Document.InsertPages", label: "Insert Pages", shortcut: None, glyph: Some(icon::INSERT_PAGES), kind: Kind::Button, hint: "Another file's sheets into this one." },
    Command { id: "Document.ExtractPages", label: "Extract Pages", shortcut: None, glyph: Some(icon::EXTRACT_PAGES), kind: Kind::Button, hint: "Chosen sheets into a file of their own." },
    Command { id: "Document.SplitDocument", label: "Split Document", shortcut: None, glyph: Some(icon::SPLIT_DOC), kind: Kind::Button, hint: "One set into several files." },
    Command { id: "Document.SlipSheet", label: "Slip Sheet", shortcut: None, glyph: Some(icon::SLIP_SHEET), kind: Kind::Button, hint: "Swap in revised sheets, matched by sheet number." },
    Command { id: "Document.DeletePages", label: "Delete Pages", shortcut: None, glyph: Some(icon::DELETE_PAGES), kind: Kind::Button, hint: "A copy without the sheets you choose." },
    Command { id: "Document.Stamp", label: "Headers and Footers", shortcut: None, glyph: Some(icon::HEADER_FOOTER), kind: Kind::Button, hint: "Put a job number, a date or a sheet count in the corners of every sheet." },
    Command { id: "Document.Shrink", label: "Reduce File Size", shortcut: None, glyph: Some(icon::SHRINK), kind: Kind::Button, hint: "Render every sheet to make a set small enough to email." },
    Command { id: "File.OpenRecent", label: "Open Recent", shortcut: None, glyph: Some(icon::RECENT), kind: Kind::Button, hint: "Drawings opened lately." },
    Command { id: "File.Revert", label: "Revert", shortcut: None, glyph: Some(icon::REVERT), kind: Kind::Button, hint: "Go back to the drawing as it is saved on disk." },
    Command { id: "Document.NumberPages", label: "Number Pages", shortcut: None, glyph: Some(icon::NUMBER_PAGES), kind: Kind::Button, hint: "Put a running number on every sheet." },
    Command { id: "File.Close", label: "Close", shortcut: Some("Ctrl+W"), glyph: Some(icon::CLOSE), kind: Kind::Button, hint: "Close this drawing. Anything unsaved is saved first." },
    Command { id: "File.CloseAll", label: "Close All", shortcut: None, glyph: Some(icon::CLOSE_ALL), kind: Kind::Button, hint: "Close every open drawing, saving each one." },
    Command { id: "Edit.Offset", label: "Offset", shortcut: None, glyph: Some(icon::OFFSET), kind: Kind::Button, hint: "Move what is selected by a set distance." },
    Command { id: "Edit.Multiply", label: "Multiply", shortcut: None, glyph: Some(icon::MULTIPLY_COPIES), kind: Kind::Button, hint: "Repeat what is selected in a grid. Every copy counts in the takeoff." },
    Command { id: "Edit.SelectAll", label: "Select All", shortcut: Some("Ctrl+A"), glyph: Some(icon::SELECT_ALL), kind: Kind::Button, hint: "Select every markup on this sheet." },
    Command { id: "Help.Shortcuts", label: "Keyboard Shortcuts", shortcut: None, glyph: Some(icon::KEYBOARD), kind: Kind::Button, hint: "Every shortcut this program has." },
    Command { id: "Help.ConnectAssistant", label: "Connect Assistant…", shortcut: None, glyph: Some(icon::LINK), kind: Kind::Button, hint: "Let the assistant app on this computer work alongside you in this window, and read the office's takeoffs when you are signed in to an office server. It proposes; only you draw." }, // WHAT: the Help menu command that connects the assistant app. WHY: a plain name that says what it does.
    Command { id: "Help.TestAssistant", label: "Test Assistant Connection", shortcut: None, glyph: Some(icon::CHECK_UPDATES), kind: Kind::Button, hint: "Check everything the assistant app needs to use Excalibur View on this computer, and say what is wrong." }, // WHAT: the Help menu command that checks the assistant app's connection. WHY: a plain name that says what it does.
    Command { id: "Help.CheckForUpdates", label: "Check for Updates", shortcut: None, glyph: Some(icon::CHECK_UPDATES), kind: Kind::Button, hint: "Look for a newer Excalibur View now, and get it if there is one." },
    Command { id: "Help.About", label: "About Excalibur View", shortcut: None, glyph: Some(icon::INFO), kind: Kind::Button, hint: "Which version this is, and what it is built on." },
    Command { id: "View.Rulers", label: "Rulers", shortcut: None, glyph: Some(icon::RULE), kind: Kind::Toggle, hint: "A rule along the top and left, in the sheet's own units." },
    Command { id: "View.MarkupText", label: "Markup Text on Sheet", shortcut: None, glyph: Some(icon::TEXT_BOX), kind: Kind::Toggle, hint: "Show every markup's text and measurement in a label over the sheet. Off, only the markup you select shows its label." },
    Command { id: "View.SimpleMode", label: "Simple Mode", shortcut: None, glyph: Some(icon::PANEL), kind: Kind::Toggle, hint: "Put the toolbars away and keep one short bar: open, select, measure, markup, undo and the pages. Turn it off here or on that bar to get the toolbars back." }, // WHAT: the switch between the full toolbars and the short bar. WHY: on a tablet the toolbars took five rows of the screen.
    Command { id: "View.QuickTools", label: "Quick Tools", shortcut: None, glyph: Some(icon::CHEST), kind: Kind::Toggle, hint: "Your own most-used tools in a bar down the right-hand side." }, // WHAT: shows or hides the quick tools bar. WHY: somebody who never uses it can put it away.
    Command { id: "View.ChooseQuickTools", label: "Choose Quick Tools…", shortcut: None, glyph: Some(icon::CHEST), kind: Kind::Button, hint: "Pick the tools that go in the quick tools bar, and their order." }, // WHAT: opens the window that sets up the quick tools. WHY: each person picks their own.
    Command { id: "View.Crosshair", label: "Full-Screen Crosshair", shortcut: None, glyph: Some(icon::CROSSHAIR), kind: Kind::Toggle, hint: "Lines across the whole window, for lining something up with the far side of a sheet." },
    Command { id: "View.SnapToContent", label: "Snap to Content", shortcut: None, glyph: Some(icon::SNAP), kind: Kind::Toggle, hint: "Pull a point being drawn onto the drawing's own line-work." },
    Command { id: "View.SnapToMarkup", label: "Snap to Markup", shortcut: None, glyph: Some(icon::SNAP), kind: Kind::Toggle, hint: "Pull a point being drawn onto a markup already there." },
    Command { id: "Batch.Combine", label: "Combine Documents", shortcut: None, glyph: Some(icon::BATCH_COMBINE), kind: Kind::Button, hint: "Several drawing sets into one, in one go." },
    Command { id: "Batch.Split", label: "Split Documents", shortcut: None, glyph: Some(icon::BATCH_SPLIT), kind: Kind::Button, hint: "Split a whole list of sets at once." },
    Command { id: "Batch.Stamp", label: "Headers and Footers", shortcut: None, glyph: Some(icon::BATCH_STAMP), kind: Kind::Button, hint: "Stamp a whole list of sets at once." },
    Command { id: "Batch.Rotate", label: "Rotate Pages", shortcut: None, glyph: Some(icon::BATCH_ROTATE), kind: Kind::Button, hint: "Turn the sheets in a whole list of sets." },
    Command { id: "Batch.Flatten", label: "Flatten Markups", shortcut: None, glyph: Some(icon::BATCH_FLATTEN), kind: Kind::Button, hint: "Flatten a whole list of sets." },
    Command { id: "Batch.Ocr", label: "OCR", shortcut: None, glyph: Some(icon::BATCH_OCR), kind: Kind::Button, hint: "Make a whole folder of scans searchable." },
    Command { id: "Batch.Seal", label: "Sign and Seal", shortcut: None, glyph: Some(icon::BATCH_SEAL), kind: Kind::Button, hint: "Put the seal on a whole list of sets." },
    Command { id: "Batch.SlipSheet", label: "Slip Sheet", shortcut: None, glyph: Some(icon::BATCH_SLIP), kind: Kind::Button, hint: "Slip a folder of revisions into the sets they belong to." },
    Command { id: "Batch.Shrink", label: "Reduce File Size", shortcut: None, glyph: Some(icon::BATCH_SHRINK), kind: Kind::Button, hint: "Render a whole list of sets smaller." },
    Command { id: "Batch.Compare", label: "Compare Documents", shortcut: None, glyph: Some(icon::BATCH_COMPARE), kind: Kind::Button, hint: "Cloud what changed across a whole folder of sets." },
    Command { id: "Batch.Crop", label: "Crop and Page Setup", shortcut: None, glyph: Some(icon::BATCH_CROP), kind: Kind::Button, hint: "Crop a whole list of sets to the same box." },
    Command { id: "Batch.ApplyStamp", label: "Apply Stamp", shortcut: None, glyph: Some(icon::BATCH_STAMP), kind: Kind::Button, hint: "Put the same stamp on every sheet of a whole list of sets." },
    Command { id: "Batch.Overlay", label: "Overlay Pages", shortcut: None, glyph: Some(icon::BATCH_OVERLAY), kind: Kind::Button, hint: "Lay a whole folder of sets over the issue of the same name." },
    Command { id: "Batch.Print", label: "Print", shortcut: None, glyph: Some(icon::BATCH_PRINT), kind: Kind::Button, hint: "Send a whole list of sets to the plotter in one go." },
    Command { id: "Batch.Summary", label: "Summary", shortcut: None, glyph: Some(icon::BATCH_SUMMARY), kind: Kind::Button, hint: "One takeoff report covering a whole list of sets." },
    Command { id: "Batch.Security", label: "Security", shortcut: None, glyph: Some(icon::BATCH_SECURITY), kind: Kind::Button, hint: "Say who may do what with a whole list of sets." },
    Command { id: "Document.Export", label: "Export", shortcut: None, glyph: Some(icon::EXPORT), kind: Kind::Button, hint: "The sheets as pictures, or the markups as a spreadsheet." },
    Command { id: "Document.PageLabels", label: "Create Page Labels", shortcut: None, glyph: Some(icon::PAGE_LABELS), kind: Kind::Button, hint: "Write each sheet's own number into the file, so other readers show it too." },
    Command { id: "Document.Repair", label: "Repair PDF", shortcut: None, glyph: Some(icon::REPAIR), kind: Kind::Button, hint: "Rebuild a file whose cross-reference table is damaged." },
    Command { id: "Document.Unflatten", label: "Unflatten Markups", shortcut: None, glyph: Some(icon::UNFLATTEN), kind: Kind::Button, hint: "Lift markups Excalibur View flattened back out into markups again." },
    Command { id: "Edit.PasteInPlace", label: "Paste in Place", shortcut: Some("Ctrl+Shift+V"), glyph: Some(icon::PASTE_IN_PLACE), kind: Kind::Button, hint: "Paste onto this sheet at exactly the spot it was copied from." },
    Command { id: "Edit.UndoHistory", label: "Undo History", shortcut: None, glyph: Some(icon::UNDO_HISTORY), kind: Kind::Button, hint: "Everything you have done to this drawing, and how far back you can go." },
    Command { id: "View.ShowGrid", label: "Show Grid", shortcut: None, glyph: Some(icon::SHOW_GRID), kind: Kind::Toggle, hint: "A grid over the sheet, spaced in the sheet's own units." },
    Command { id: "View.SnapToGrid", label: "Snap to Grid", shortcut: None, glyph: Some(icon::SNAP_TO_GRID), kind: Kind::Toggle, hint: "Pull a point being drawn onto the nearest grid line." },
    Command { id: "Document.Summary", label: "Summary", shortcut: None, glyph: Some(icon::SUMMARY), kind: Kind::Button, hint: "The takeoff as a report you can print and put in a bid folder." },
    Command { id: "Document.RevisionCost", label: "Revision Cost", shortcut: None, glyph: Some(icon::REVISION_COST), kind: Kind::Button, hint: "What a new issue did to the quantities, between two open drawings." },
    Command { id: "Document.Doubles", label: "Counted Twice?", shortcut: None, glyph: Some(icon::DOUBLES), kind: Kind::Button, hint: "Find markups sitting on top of each other measuring the same thing. It reports; it never removes." },
    Command { id: "Document.ShopList", label: "Shop List", shortcut: None, glyph: Some(icon::SHOP_LIST), kind: Kind::Button, hint: "The takeoff as a cut list: what to cut, how many, and how many sticks to buy." },
    Command { id: "Document.Ocr", label: "OCR", shortcut: Some("Ctrl+Shift+O"), glyph: Some(icon::OCR), kind: Kind::Button, hint: "Read the words off a scanned set so it can be searched." },
    Command { id: "Document.Compare", label: "Compare Documents", shortcut: None, glyph: Some(icon::COMPARE), kind: Kind::Button, hint: "What changed between two issues of a sheet." },
    Command { id: "Document.Overlay", label: "Overlay Pages", shortcut: None, glyph: Some(icon::OVERLAY), kind: Kind::Button, hint: "Two issues of a sheet printed over each other in different colours." },
    Command { id: "Document.FlattenMarkups", label: "Flatten Markups", shortcut: None, glyph: Some(icon::FLATTEN), kind: Kind::Button, hint: "Turn markups into part of the page. They stop being markups." },
    Command { id: "Split.Email", label: "Email", shortcut: Some("Ctrl+E"), glyph: Some(icon::ATTACH), kind: Kind::Button, hint: "Send the set on." },
    Command { id: "Document.NewPackage", label: "Package", shortcut: None, glyph: Some(icon::CHEST), kind: Kind::Button, hint: "Bundle several files together." },
    Command { id: "Search", label: "Search", shortcut: Some("Ctrl+F"), glyph: Some(icon::SEARCH), kind: Kind::Button, hint: "Find text across every sheet." },
    Command { id: "SpellCheck", label: "Spell Check", shortcut: None, glyph: Some(icon::SPELL), kind: Kind::Button, hint: "Check the spelling of the notes." },
    // Revu spells this identifier two ways between versions, and a profile
    // that asks for either has to find the button. The shortcut belongs to
    // one of them, or two buttons claim the same keys.
    Command { id: "Document.OCR", label: "OCR", shortcut: None, glyph: Some(icon::OCR), kind: Kind::Button, hint: "Read the words off a scanned set so it can be searched." },
    Command { id: "Document.Script", label: "Script", shortcut: None, glyph: Some(icon::GRID), kind: Kind::Button, hint: "Run a script over the set." },
    Command { id: "Document.Properties", label: "Properties", shortcut: Some("Ctrl+D"), glyph: Some(icon::PROPERTIES), kind: Kind::Button, hint: "What the file says about itself." },
    Command { id: "Split.Security", label: "Security", shortcut: Some("Ctrl+L"), glyph: Some(icon::SECURITY), kind: Kind::Button, hint: "Who may do what with this file." },
    Command { id: "Split.Profiles", label: "Profiles", shortcut: None, glyph: Some(icon::PANEL), kind: Kind::Button, hint: "Switch the whole arrangement of the program." },
    Command { id: "Split.Dimmer", label: "Dimmer", shortcut: None, glyph: Some(icon::INVERT), kind: Kind::Button, hint: "Dim everything but the markups." },
    Command { id: "Server.Connect", label: "Connect to a Server", shortcut: None, glyph: Some(icon::LINK), kind: Kind::Button, hint: "Your company's drawings, tool chests and takeoffs." },
    Command { id: "File.SaveAs", label: "Save As", shortcut: Some("Ctrl+Shift+S"), glyph: Some(icon::SAVE), kind: Kind::Button, hint: "Save the drawing and its markups under a new name." },
    Command { id: "File.Preferences", label: "Preferences", shortcut: None, glyph: Some(icon::PANEL), kind: Kind::Button, hint: "How this program behaves." },
    Command { id: "Edit.Undo", label: "Undo", shortcut: Some("Ctrl+Z"), glyph: Some(icon::UNDO), kind: Kind::Button, hint: "Take back the last thing you did." },
    Command { id: "Edit.Redo", label: "Redo", shortcut: Some("Ctrl+Y"), glyph: Some(icon::REDO), kind: Kind::Button, hint: "Put it back." },
    Command { id: "Edit.Cut", label: "Cut", shortcut: Some("Ctrl+X"), glyph: Some(icon::CUT), kind: Kind::Button, hint: "Cut the selection." },
    Command { id: "Edit.Copy", label: "Copy", shortcut: Some("Ctrl+C"), glyph: Some(icon::COPY), kind: Kind::Button, hint: "Copy the selection." },
    Command { id: "Edit.Paste", label: "Paste", shortcut: Some("Ctrl+V"), glyph: Some(icon::PASTE), kind: Kind::Button, hint: "Paste it." },
    Command { id: "Delete", label: "Delete", shortcut: Some("Del"), glyph: Some(icon::DELETE), kind: Kind::Button, hint: "Remove the selection." },
    Command { id: "Edit.Text", label: "Edit Text", shortcut: None, glyph: Some(icon::TEXT_BOX), kind: Kind::Toggle, hint: "Change the words already on the sheet." },
    Command { id: "Edit.SelectText", label: "Select Text", shortcut: None, glyph: Some(icon::TEXT_BOX), kind: Kind::Button, hint: "Pick up text from the drawing." },
    Command { id: "Edit.EraseContent", label: "Erase Content", shortcut: None, glyph: Some(icon::ERASER), kind: Kind::Button, hint: "Rub something off the sheet for good." },
    Command { id: "Markup.FormatPainter", label: "Format Painter", shortcut: Some("Ctrl+Shift+C"), glyph: Some(icon::FORMAT_PAINTER), kind: Kind::Button, hint: "Copy one markup's look onto another." },
    Command { id: "Select", label: "Select", shortcut: Some("V"), glyph: Some(icon::POINTER), kind: Kind::Toggle, hint: "Pick markups." },
    Command { id: "Pan", label: "Pan", shortcut: Some("Shift+V"), glyph: Some(icon::HAND), kind: Kind::Toggle, hint: "Move the sheet under the window." },
    Command { id: "Zoom", label: "Zoom", shortcut: None, glyph: Some(icon::MAGNIFIER), kind: Kind::Toggle, hint: "Zoom to a window you drag." },
    Command { id: "Lasso", label: "Lasso", shortcut: Some("Shift+O"), glyph: Some(icon::LASSO), kind: Kind::Toggle, hint: "Select by drawing round things." },
    Command { id: "Snapshot", label: "Snapshot", shortcut: Some("G"), glyph: Some(icon::SNAPSHOT), kind: Kind::Toggle, hint: "Take a picture of part of the sheet." },
    Command { id: "Press.EscapeKey", label: "Escape", shortcut: None, glyph: Some(icon::DELETE), kind: Kind::Button, hint: "Stop what you are doing." },
    Command { id: "Toggle.FullScreen", label: "Full Screen", shortcut: Some("F11"), glyph: Some(icon::FULL_SCREEN), kind: Kind::Toggle, hint: "Fill the screen with the drawing." },
    Command { id: "Toggle.ShiftKey", label: "Hold Shift", shortcut: None, glyph: Some(icon::POINTER), kind: Kind::Toggle, hint: "Hold the shift key down." },
    Command { id: "Blank", label: "", shortcut: None, glyph: None, kind: Kind::Separator, hint: "" },
    Command { id: "Measure.Tool", label: "Measure", shortcut: None, glyph: Some(icon::RULE), kind: Kind::Toggle, hint: "Show what things measure." },
    Command { id: "Measure.Calibrate", label: "Set Scale", shortcut: None, glyph: Some(icon::CALIBRATE), kind: Kind::Button, hint: "Tell the sheet what it is drawn at." },
    Command { id: "Measure.Length", label: "Length", shortcut: Some("Shift+Alt+L"), glyph: Some(icon::LENGTH), kind: Kind::Toggle, hint: "Measure a straight run." },
    Command { id: "Measure.Polylength", label: "Polylength", shortcut: Some("Shift+Alt+Q"), glyph: Some(icon::POLYLENGTH), kind: Kind::Toggle, hint: "Measure a run round corners." },
    Command { id: "Measure.Area", label: "Area", shortcut: Some("Shift+Alt+A"), glyph: Some(icon::AREA), kind: Kind::Toggle, hint: "Measure what a shape covers." },
    Command { id: "Measure.Perimeter", label: "Perimeter", shortcut: Some("Shift+Alt+P"), glyph: Some(icon::PERIMETER), kind: Kind::Toggle, hint: "Measure the way round a shape." },
    Command { id: "Measure.Diameter", label: "Diameter", shortcut: Some("Shift+Alt+D"), glyph: Some(icon::DIAMETER), kind: Kind::Toggle, hint: "Measure across a circle." },
    Command { id: "Measure.Radius", label: "Radius", shortcut: None, glyph: Some(icon::RADIUS), kind: Kind::Toggle, hint: "Measure from the middle of a curve." },
    Command { id: "Measure.Angle", label: "Angle", shortcut: Some("Shift+Alt+G"), glyph: Some(icon::ANGLE), kind: Kind::Toggle, hint: "Measure a corner." },
    Command { id: "Measure.Volume", label: "Volume", shortcut: Some("Shift+Alt+V"), glyph: Some(icon::VOLUME), kind: Kind::Toggle, hint: "Measure an area with a depth." },
    Command { id: "Measure.Count", label: "Count", shortcut: Some("Shift+Alt+C"), glyph: Some(icon::COUNT), kind: Kind::Toggle, hint: "Count items, one click each." },
    Command { id: "Measure.AreaCutout", label: "Area Cutout", shortcut: None, glyph: Some(icon::AREA_CUTOUT), kind: Kind::Toggle, hint: "Take an opening out of an area." },
    Command { id: "Measure.AreaEllipseCutout", label: "Ellipse Cutout", shortcut: None, glyph: Some(icon::ELLIPSE_CUTOUT), kind: Kind::Toggle, hint: "Take a round opening out of an area." },
    Command { id: "Measure.FillBoundary", label: "Fill Boundary", shortcut: Some("Shift+J"), glyph: Some(icon::PEN), kind: Kind::Toggle, hint: "Draw across a doorway so a fill stops there." },
    Command { id: "Measure.ApplyFill", label: "Apply Fill", shortcut: None, glyph: Some(icon::AREA), kind: Kind::Button, hint: "Turn the filled shape into an area measurement." },
    Command { id: "Measure.DynamicFill", label: "Dynamic Fill", shortcut: Some("J"), glyph: Some(icon::FILL), kind: Kind::Toggle, hint: "Fill an area the drawing's own lines close in." },
    Command { id: "Markup.Line", label: "Line", shortcut: None, glyph: Some(icon::LINE), kind: Kind::Toggle, hint: "Draw a line." },
    Command { id: "Markup.Arrow", label: "Arrow", shortcut: None, glyph: Some(icon::ARROW), kind: Kind::Toggle, hint: "Draw an arrow." },
    Command { id: "Markup.Arc", label: "Arc", shortcut: None, glyph: Some(icon::ARC), kind: Kind::Toggle, hint: "Draw a curve." },
    Command { id: "Markup.Polyline", label: "Polyline", shortcut: None, glyph: Some(icon::POLYLINE), kind: Kind::Toggle, hint: "Draw a run of lines." },
    Command { id: "Markup.Dimension", label: "Dimension", shortcut: None, glyph: Some(icon::DIMENSION), kind: Kind::Toggle, hint: "Draw a dimension line." },
    Command { id: "Markup.Rectangle", label: "Rectangle", shortcut: None, glyph: Some(icon::RECTANGLE), kind: Kind::Toggle, hint: "Draw a box." },
    Command { id: "Markup.Ellipse", label: "Ellipse", shortcut: None, glyph: Some(icon::ELLIPSE), kind: Kind::Toggle, hint: "Draw an ellipse." },
    Command { id: "Markup.Polygon", label: "Polygon", shortcut: None, glyph: Some(icon::POLYGON), kind: Kind::Toggle, hint: "Draw a shape with straight sides." },
    Command { id: "Markup.Cloud", label: "Cloud", shortcut: None, glyph: Some(icon::CLOUD), kind: Kind::Toggle, hint: "Cloud something for attention." },
    Command { id: "Markup.Cloud9", label: "Polygon Cloud", shortcut: None, glyph: Some(icon::CLOUD_POLYGON), kind: Kind::Toggle, hint: "Cloud a shape with straight sides." },
    Command { id: "Markup.Signature", label: "Signature", shortcut: None, glyph: Some(icon::SIGNATURE), kind: Kind::Toggle, hint: "Your own signature, drawn or scanned once and put on any sheet." },
    Command { id: "Markup.CloudPlus", label: "Cloud+", shortcut: Some("K"), glyph: Some(icon::CLOUD_PLUS), kind: Kind::Toggle, hint: "A cloud with a leader to a box of words, as one." },
    Command { id: "Markup.TextBox", label: "Text Box", shortcut: None, glyph: Some(icon::TEXT_BOX), kind: Kind::Toggle, hint: "Put words on the sheet." },
    Command { id: "Markup.Typewriter", label: "Typewriter", shortcut: None, glyph: Some(icon::TYPEWRITER), kind: Kind::Toggle, hint: "Type straight onto the sheet." },
    Command { id: "Markup.Callout", label: "Callout", shortcut: None, glyph: Some(icon::CALLOUT), kind: Kind::Toggle, hint: "A note with a line pointing at something." },
    Command { id: "Markup.Note", label: "Note", shortcut: None, glyph: Some(icon::NOTE), kind: Kind::Toggle, hint: "A note that stays folded until opened." },
    Command { id: "Markup.Flag", label: "Flag", shortcut: None, glyph: Some(icon::FLAG), kind: Kind::Toggle, hint: "Flag something to come back to." },
    Command { id: "Markup.Pen", label: "Pen", shortcut: None, glyph: Some(icon::PEN), kind: Kind::Toggle, hint: "Draw freehand." },
    Command { id: "Eraser", label: "Eraser", shortcut: None, glyph: Some(icon::ERASER), kind: Kind::Toggle, hint: "Rub out pen strokes." },
    Command { id: "Markup.Highlight", label: "Highlight", shortcut: None, glyph: Some(icon::HIGHLIGHT), kind: Kind::Toggle, hint: "Highlight part of the sheet." },
    Command { id: "Markup.ReviewText", label: "Review Text", shortcut: None, glyph: Some(icon::UNDERLINE), kind: Kind::Toggle, hint: "Mark up the words on the sheet." },
    Command { id: "Markup.Underline", label: "Underline", shortcut: None, glyph: Some(icon::UNDERLINE), kind: Kind::Toggle, hint: "Underline text." },
    Command { id: "Markup.Squiggly", label: "Squiggly", shortcut: None, glyph: Some(icon::SQUIGGLY), kind: Kind::Toggle, hint: "Squiggle under text." },
    Command { id: "Markup.Strikethrough", label: "Strikethrough", shortcut: None, glyph: Some(icon::STRIKE), kind: Kind::Toggle, hint: "Strike text out." },
    Command { id: "Button.Stamp", label: "Stamp", shortcut: None, glyph: Some(icon::STAMP), kind: Kind::Toggle, hint: "Stamp the sheet." },
    Command { id: "Markup.Image", label: "Image", shortcut: None, glyph: Some(icon::IMAGE), kind: Kind::Toggle, hint: "Put a picture on the sheet." },
    Command { id: "Markup.ImageFromCamera", label: "Photo", shortcut: None, glyph: Some(icon::IMAGE), kind: Kind::Toggle, hint: "Take a photograph onto the sheet." },
    Command { id: "CropImage", label: "Crop Image", shortcut: None, glyph: Some(icon::IMAGE), kind: Kind::Button, hint: "Trim a picture down." },
    Command { id: "Markup.Hyperlink", label: "Hyperlink", shortcut: Some("Shift+H"), glyph: Some(icon::LINK), kind: Kind::Toggle, hint: "Link to another sheet or a web page." },
    Command { id: "Markup.FileAttachment", label: "Attach File", shortcut: Some("F"), glyph: Some(icon::ATTACH), kind: Kind::Toggle, hint: "Hang a file off the sheet." },
    Command { id: "Markup.Redaction", label: "Redaction", shortcut: None, glyph: Some(icon::REDACTION), kind: Kind::Toggle, hint: "Mark something to be blacked out." },
    Command { id: "Document.ApplyRedactions", label: "Apply Redactions", shortcut: None, glyph: Some(icon::APPLY_REDACTION), kind: Kind::Button, hint: "Black it out for good." },
    Command { id: "Markup.DigitalSignature", label: "Signature Field", shortcut: None, glyph: Some(icon::SIGN), kind: Kind::Toggle, hint: "A place for somebody to sign." },
    Command { id: "Document.Seal", label: "Sign and Seal", shortcut: None, glyph: Some(icon::SIGN), kind: Kind::Button, hint: "Put the engineer's seal on the sheets. A picture on the sheet, not a digital signature." },
    Command { id: "Document.Fingerprint", label: "Fingerprint This Set", shortcut: None, glyph: Some(icon::FINGERPRINT), kind: Kind::Button, hint: "A fingerprint anybody can recompute to show a set is the one you sent." },
    Command { id: "Document.Flatten", label: "Flatten", shortcut: Some("Ctrl+Shift+M"), glyph: Some(icon::FLATTEN), kind: Kind::Button, hint: "Press the markups into the sheet." },
    Command { id: "Space.Add", label: "Add Space", shortcut: None, glyph: Some(icon::SPACE), kind: Kind::Toggle, hint: "Mark out a room or an area of the job." },
    Command { id: "Tab.Spaces", label: "Spaces", shortcut: None, glyph: Some(icon::PERIMETER), kind: Kind::Button, hint: "The rooms and areas on this sheet." },
    Command { id: "Markup.PolygonSketchToScale", label: "Sketch Polygon", shortcut: None, glyph: Some(icon::SKETCH), kind: Kind::Toggle, hint: "Draw a shape at its real size." },
    Command { id: "Markup.RectangleSketchToScale", label: "Sketch Rectangle", shortcut: None, glyph: Some(icon::SKETCH), kind: Kind::Toggle, hint: "Draw a box at its real size." },
    Command { id: "Markup.EllipseSketchToScale", label: "Sketch Ellipse", shortcut: None, glyph: Some(icon::SKETCH), kind: Kind::Toggle, hint: "Draw an ellipse at its real size." },
    Command { id: "Markup.PolylineSketchToScale", label: "Sketch Polyline", shortcut: None, glyph: Some(icon::SKETCH), kind: Kind::Toggle, hint: "Draw a run of lines at real size." },
    Command { id: "Markup.BringToFront", label: "Bring to Front", shortcut: None, glyph: Some(icon::TO_FRONT), kind: Kind::Button, hint: "Put it in front of everything." },
    Command { id: "Markup.SendToBack", label: "Send to Back", shortcut: None, glyph: Some(icon::TO_BACK), kind: Kind::Button, hint: "Put it behind everything." },
    Command { id: "Markup.BringForward", label: "Bring Forward", shortcut: None, glyph: Some(icon::FORWARD_ONE), kind: Kind::Button, hint: "One step forward." },
    Command { id: "Markup.SendBackward", label: "Send Backward", shortcut: None, glyph: Some(icon::BACKWARD_ONE), kind: Kind::Button, hint: "One step back." },
    Command { id: "ControlPoint.AddMode", label: "Add Points", shortcut: None, glyph: Some(icon::POLYLINE), kind: Kind::Toggle, hint: "Add corners to a shape." },
    Command { id: "ControlPoint.SubtractMode", label: "Remove Points", shortcut: None, glyph: Some(icon::POLYLINE), kind: Kind::Toggle, hint: "Take corners out of a shape." },
    Command { id: "ControlPoint.ConvertMode", label: "Straighten or Curve", shortcut: None, glyph: Some(icon::ARC), kind: Kind::Toggle, hint: "Turn a corner into a curve, or back." },
    Command { id: "Document.RotateClockwise", label: "Rotate Right", shortcut: None, glyph: Some(icon::ROTATE_RIGHT), kind: Kind::Button, hint: "Turn the sheet a quarter clockwise." },
    Command { id: "Document.RotateCounterclockwise", label: "Rotate Left", shortcut: None, glyph: Some(icon::ROTATE_LEFT), kind: Kind::Button, hint: "Turn the sheet a quarter the other way." },
    Command { id: "Document.HeadersAndFooters", label: "Headers and Footers", shortcut: None, glyph: Some(icon::TEXT_BOX), kind: Kind::Button, hint: "Put a line of text on every sheet." },
    Command { id: "Document.CropPages", label: "Crop Pages", shortcut: Some("Shift+Alt+O"), glyph: Some(icon::SNAPSHOT), kind: Kind::Button, hint: "Trim the sheets down." },
    Command { id: "View.FitPage", label: "Fit Page", shortcut: Some("Ctrl+9"), glyph: Some(icon::FIT_PAGE), kind: Kind::Button, hint: "Show the whole sheet." },
    Command { id: "View.FitWidth", label: "Fit Width", shortcut: Some("Ctrl+0"), glyph: Some(icon::FIT_WIDTH), kind: Kind::Button, hint: "Fill the window sideways." },
    Command { id: "View.ActualSize", label: "Actual Size", shortcut: Some("Ctrl+8"), glyph: Some(icon::ACTUAL_SIZE), kind: Kind::Button, hint: "Show it at its printed size." },
    Command { id: "View.Single", label: "Single Page", shortcut: Some("Ctrl+4"), glyph: Some(icon::PAGE_SINGLE), kind: Kind::Toggle, hint: "One sheet at a time." },
    Command { id: "View.Continuous", label: "Continuous", shortcut: Some("Ctrl+5"), glyph: Some(icon::PAGE_CONTINUOUS), kind: Kind::Toggle, hint: "Scroll straight through the set." },
    Command { id: "View.SideBySide", label: "Side by Side", shortcut: Some("Ctrl+6"), glyph: Some(icon::PAGE_FACING), kind: Kind::Toggle, hint: "Two sheets at once." },
    Command { id: "View.SideBySideContinuous", label: "Continuous Side by Side", shortcut: Some("Ctrl+7"), glyph: Some(icon::PAGE_FACING_CONTINUOUS), kind: Kind::Toggle, hint: "Two at once, scrolling." },
    Command { id: "View.OneFullPage", label: "One Full Page", shortcut: None, glyph: Some(icon::FIT_PAGE), kind: Kind::Toggle, hint: "A sheet at a time, whole." },
    Command { id: "View.ScrollingPages", label: "Scrolling Pages", shortcut: None, glyph: Some(icon::SPLIT_HORIZONTAL), kind: Kind::Toggle, hint: "Scroll through the sheets." },
    Command { id: "View.Split", label: "Split", shortcut: Some("Ctrl+2"), glyph: Some(icon::SPLIT_VERTICAL), kind: Kind::Toggle, hint: "Two views of the same set, side by side." },
    Command { id: "View.SplitHorizontal", label: "Split Across", shortcut: Some("Ctrl+H"), glyph: Some(icon::SPLIT_HORIZONTAL), kind: Kind::Toggle, hint: "Two views, one above the other." },
    Command { id: "View.SyncPanes", label: "Sync Panes", shortcut: None, glyph: Some(icon::LINK), kind: Kind::Toggle, hint: "Move both halves of a split together." },
    Command { id: "View.UnSplit", label: "Unsplit", shortcut: Some("Ctrl+Shift+2"), glyph: Some(icon::FIT_PAGE), kind: Kind::Toggle, hint: "Back to one view." },
    Command { id: "View.InvertColors", label: "Invert", shortcut: Some("Ctrl+3"), glyph: Some(icon::INVERT), kind: Kind::Toggle, hint: "White lines on black, for a dark room." },
    Command { id: "View.ClassicLook", label: "Classic Look", shortcut: None, glyph: Some(icon::PANEL), kind: Kind::Toggle, hint: "Grey with raised buttons and sunken boxes, the look of an office computer from the late nineties. Kept for next time." },
    Command { id: "View.PageFirst", label: "First Sheet", shortcut: Some("Home"), glyph: Some(icon::FIRST), kind: Kind::Button, hint: "Go to the first sheet." },
    Command { id: "View.PagePrevious", label: "Previous Sheet", shortcut: Some("PageUp"), glyph: Some(icon::PREVIOUS), kind: Kind::Button, hint: "Back one sheet." },
    Command { id: "View.PageNext", label: "Next Sheet", shortcut: Some("PageDown"), glyph: Some(icon::NEXT), kind: Kind::Button, hint: "On one sheet." },
    Command { id: "View.PageLast", label: "Last Sheet", shortcut: Some("End"), glyph: Some(icon::LAST), kind: Kind::Button, hint: "Go to the last sheet." },
    Command { id: "View.PreviousView", label: "Back", shortcut: None, glyph: Some(icon::VIEW_BACK), kind: Kind::Button, hint: "Back to where you were looking." },
    Command { id: "View.NextView", label: "Forward", shortcut: None, glyph: Some(icon::VIEW_FORWARD), kind: Kind::Button, hint: "Forward again." },
    Command { id: "TextBox.Page", label: "Sheet", shortcut: None, glyph: None, kind: Kind::Combo, hint: "Which sheet you are on." },
    Command { id: "Navigation.PageScale", label: "Scale", shortcut: None, glyph: None, kind: Kind::Label, hint: "What this sheet is drawn at." },
    Command { id: "Navigation.PageSize", label: "Size", shortcut: None, glyph: None, kind: Kind::Label, hint: "How big the sheet is." },
    Command { id: "Combo.Scale", label: "Scale", shortcut: None, glyph: None, kind: Kind::Combo, hint: "The scale measurements are taken at." },
    Command { id: "DropDown.ColorLine", label: "Line Colour", shortcut: None, glyph: Some(icon::PALETTE), kind: Kind::DropDown, hint: "The colour markups are drawn in." },
    Command { id: "DropDown.ColorFill", label: "Fill Colour", shortcut: None, glyph: Some(icon::PALETTE), kind: Kind::DropDown, hint: "The colour shapes are filled with." },
    Command { id: "DropDown.ColorText", label: "Text Colour", shortcut: None, glyph: Some(icon::PALETTE), kind: Kind::DropDown, hint: "The colour words are written in." },
    Command { id: "DropDown.Opacity", label: "Opacity", shortcut: None, glyph: Some(icon::OPACITY), kind: Kind::DropDown, hint: "How much shows through." },
    Command { id: "DropDown.HatchPattern", label: "Hatch", shortcut: None, glyph: Some(icon::HATCH), kind: Kind::DropDown, hint: "The pattern a shape is filled with." },
    Command { id: "DropDown.LineWidth", label: "Line Width", shortcut: None, glyph: Some(icon::LINE_WIDTH), kind: Kind::DropDown, hint: "How heavy the line is." },
    Command { id: "DropDown.LineStyle", label: "Line Style", shortcut: None, glyph: Some(icon::LINE_STYLE), kind: Kind::DropDown, hint: "Solid, dashed or dotted." },
    Command { id: "DropDown.LineStart", label: "Line Start", shortcut: None, glyph: Some(icon::LINE_END), kind: Kind::DropDown, hint: "What the line starts with." },
    Command { id: "DropDown.LineEnd", label: "Line End", shortcut: None, glyph: Some(icon::LINE_END), kind: Kind::DropDown, hint: "What the line ends with." },
    Command { id: "Label.LinePreview", label: "", shortcut: None, glyph: None, kind: Kind::Label, hint: "How the line will look." },
    Command { id: "Combo.Font", label: "Font", shortcut: None, glyph: None, kind: Kind::Combo, hint: "The typeface for notes." },
    Command { id: "Combo.FontSize", label: "Size", shortcut: None, glyph: None, kind: Kind::Combo, hint: "How big the words are." },
    Command { id: "Button.Bold", label: "Bold", shortcut: None, glyph: Some(icon::BOLD), kind: Kind::Toggle, hint: "Heavier text." },
    Command { id: "Button.Italic", label: "Italic", shortcut: None, glyph: Some(icon::ITALIC), kind: Kind::Toggle, hint: "Sloped text." },
    Command { id: "Button.Underline", label: "Underline", shortcut: None, glyph: Some(icon::UNDERLINE_TEXT), kind: Kind::Toggle, hint: "Underlined text." },
    Command { id: "Button.Strikethrough", label: "Strikethrough", shortcut: None, glyph: Some(icon::STRIKE_TEXT), kind: Kind::Toggle, hint: "Text with a line through it." },
    Command { id: "Button.TextAlignLeft", label: "Align Left", shortcut: None, glyph: Some(icon::ALIGN_LEFT), kind: Kind::Toggle, hint: "Line the words up on the left." },
    Command { id: "Button.TextAlignCenter", label: "Centre", shortcut: None, glyph: Some(icon::ALIGN_CENTRE), kind: Kind::Toggle, hint: "Centre the words." },
    Command { id: "Button.TextAlignRight", label: "Align Right", shortcut: None, glyph: Some(icon::ALIGN_RIGHT), kind: Kind::Toggle, hint: "Line the words up on the right." },
    Command { id: "Button.TextVAlignTop", label: "Align Top", shortcut: None, glyph: Some(icon::ALIGN_LEFT), kind: Kind::Toggle, hint: "Words to the top of the box." },
    Command { id: "Button.TextVAlignMiddle", label: "Align Middle", shortcut: None, glyph: Some(icon::ALIGN_CENTRE), kind: Kind::Toggle, hint: "Words to the middle." },
    Command { id: "Button.TextVAlignBottom", label: "Align Bottom", shortcut: None, glyph: Some(icon::ALIGN_RIGHT), kind: Kind::Toggle, hint: "Words to the bottom." },
    Command { id: "Button.Superscript", label: "Superscript", shortcut: None, glyph: Some(icon::SUPERSCRIPT), kind: Kind::Toggle, hint: "Raised text." },
    Command { id: "Button.Subscript", label: "Subscript", shortcut: None, glyph: Some(icon::SUBSCRIPT), kind: Kind::Toggle, hint: "Lowered text." },
    Command { id: "Align.Left", label: "Align Left", shortcut: None, glyph: Some(icon::ALIGN_LEFT), kind: Kind::Button, hint: "Line the markups up on the left." },
    Command { id: "Align.Center", label: "Align Centre", shortcut: None, glyph: Some(icon::ALIGN_CENTRE), kind: Kind::Button, hint: "Line them up down the middle." },
    Command { id: "Align.Right", label: "Align Right", shortcut: None, glyph: Some(icon::ALIGN_RIGHT), kind: Kind::Button, hint: "Line them up on the right." },
    Command { id: "Align.Top", label: "Align Top", shortcut: None, glyph: Some(icon::ALIGN_TOP), kind: Kind::Button, hint: "Line them up along the top." },
    Command { id: "Align.Middle", label: "Align Middle", shortcut: None, glyph: Some(icon::ALIGN_MIDDLE), kind: Kind::Button, hint: "Line them up across the middle." },
    Command { id: "Align.Bottom", label: "Align Bottom", shortcut: None, glyph: Some(icon::ALIGN_BOTTOM), kind: Kind::Button, hint: "Line them up along the bottom." },
    Command { id: "Align.Width", label: "Match Width", shortcut: None, glyph: Some(icon::SAME_WIDTH), kind: Kind::Button, hint: "Make them all the same width." },
    Command { id: "Align.Height", label: "Match Height", shortcut: None, glyph: Some(icon::SAME_HEIGHT), kind: Kind::Button, hint: "Make them all the same height." },
    Command { id: "Align.Size", label: "Match Size", shortcut: None, glyph: Some(icon::SAME_SIZE), kind: Kind::Button, hint: "Make them all the same size." },
    Command { id: "Align.CenterDocument", label: "Centre on Sheet", shortcut: None, glyph: Some(icon::CENTRE_ON_SHEET), kind: Kind::Button, hint: "Put it in the middle of the sheet." },
    Command { id: "Align.SpacingHorizontal", label: "Space Across", shortcut: None, glyph: Some(icon::SPACE_ACROSS), kind: Kind::Button, hint: "Even gaps left to right." },
    Command { id: "Align.SpacingVertical", label: "Space Down", shortcut: None, glyph: Some(icon::SPACE_DOWN), kind: Kind::Button, hint: "Even gaps top to bottom." },
    Command { id: "Align.FlipHorizontal", label: "Flip Across", shortcut: None, glyph: Some(icon::FLIP_ACROSS), kind: Kind::Button, hint: "Mirror it left to right." },
    Command { id: "Align.FlipVertical", label: "Flip Down", shortcut: None, glyph: Some(icon::FLIP_DOWN), kind: Kind::Button, hint: "Mirror it top to bottom." },
    Command { id: "Form.Editor", label: "Form Editor", shortcut: None, glyph: Some(icon::FORM_EDITOR), kind: Kind::Toggle, hint: "Lay out a form." },
    Command { id: "Form.TextField", label: "Text Field", shortcut: None, glyph: Some(icon::FORM_FIELD), kind: Kind::Toggle, hint: "A box to type in." },
    Command { id: "Form.RadioButton", label: "Radio Button", shortcut: None, glyph: Some(icon::RADIO_BUTTON), kind: Kind::Toggle, hint: "One choice out of several." },
    Command { id: "Form.CheckBox", label: "Check Box", shortcut: None, glyph: Some(icon::CHECK_BOX), kind: Kind::Toggle, hint: "A box to tick." },
    Command { id: "Form.ListBox", label: "List Box", shortcut: None, glyph: Some(icon::LIST_BOX), kind: Kind::Toggle, hint: "A list to choose from." },
    Command { id: "Form.ComboBox", label: "Combo Box", shortcut: None, glyph: Some(icon::COMBO_BOX), kind: Kind::Toggle, hint: "A list you can also type in." },
    Command { id: "Form.Button", label: "Button", shortcut: None, glyph: Some(icon::PUSH_BUTTON), kind: Kind::Toggle, hint: "A button." },
    Command { id: "Form.DigitalSignature", label: "Signature Field", shortcut: None, glyph: Some(icon::SIGN), kind: Kind::Toggle, hint: "A place to sign." },
    Command { id: "Combo.DMS", label: "Document System", shortcut: None, glyph: None, kind: Kind::Combo, hint: "A document management system." },
    Command { id: "DMS.Login", label: "Sign In", shortcut: None, glyph: None, kind: Kind::Button, hint: "Sign in to the document system." },
    Command { id: "DMS.Open", label: "Open from System", shortcut: None, glyph: None, kind: Kind::Button, hint: "Open a file held in the system." },
    Command { id: "DMS.SaveAs", label: "Save to System", shortcut: None, glyph: None, kind: Kind::Button, hint: "Put the file back in the system." },
    Command { id: "DMS.CheckIn", label: "Check In", shortcut: None, glyph: None, kind: Kind::Button, hint: "Hand the file back." },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_a_label_or_is_deliberately_blank() {
        for c in ALL {
            if c.label.is_empty() {
                assert!(
                    matches!(c.kind, Kind::Separator | Kind::Label),
                    "{} has no label",
                    c.id
                );
            }
        }
    }

    #[test]
    fn every_command_says_what_it_does() {
        for c in ALL {
            if c.kind == Kind::Separator {
                continue;
            }
            assert!(!c.hint.is_empty(), "{} has no hint", c.id);
            assert!(
                c.hint.ends_with('.'),
                "{} reads like a fragment: {:?}",
                c.id,
                c.hint
            );
        }
    }

    #[test]
    fn identifiers_are_unique() {
        let mut seen: Vec<&str> = Vec::new();
        for c in ALL {
            assert!(!seen.contains(&c.id), "{} appears twice", c.id);
            seen.push(c.id);
        }
    }

    #[test]
    fn the_measure_toolbar_from_his_profile_is_covered() {
        for id in [
            "Measure.Tool", "Measure.Calibrate", "Measure.Length", "Measure.Polylength",
            "Measure.Area", "Measure.Perimeter", "Measure.Diameter", "Measure.Angle",
            "Measure.Radius", "Measure.Volume", "Measure.Count", "Measure.AreaCutout",
            "Measure.AreaEllipseCutout", "Measure.DynamicFill",
        ] {
            let c = find(id).unwrap_or_else(|| panic!("no command {id}"));
            assert!(c.glyph.is_some(), "{id} has no icon, and the measure bar is all icons");
        }
    }

    #[test]
    fn the_shortcuts_people_have_in_their_fingers_are_the_ones_revu_uses() {
        assert_eq!(find("Measure.Length").unwrap().shortcut, Some("Shift+Alt+L"));
        assert_eq!(find("Measure.Count").unwrap().shortcut, Some("Shift+Alt+C"));
        assert_eq!(find("Measure.DynamicFill").unwrap().shortcut, Some("J"));
        assert_eq!(find("Snapshot").unwrap().shortcut, Some("G"));
        assert_eq!(find("Pan").unwrap().shortcut, Some("Shift+V"));
        assert_eq!(find("Select").unwrap().shortcut, Some("V"));
        assert_eq!(find("View.FitPage").unwrap().shortcut, Some("Ctrl+9"));
        assert_eq!(find("View.FitWidth").unwrap().shortcut, Some("Ctrl+0"));
        assert_eq!(find("View.ActualSize").unwrap().shortcut, Some("Ctrl+8"));
    }

    #[test]
    fn no_two_commands_claim_the_same_shortcut() {
        let mut seen: Vec<(&str, &str)> = Vec::new();
        for c in ALL {
            let Some(keys) = c.shortcut else { continue };
            if let Some((other, _)) = seen.iter().find(|(_, k)| *k == keys) {
                panic!("{} and {} both want {keys}", other, c.id);
            }
            seen.push((c.id, keys));
        }
    }

    #[test]
    fn a_tooltip_names_the_command_and_its_keys() {
        let tip = find("Measure.Length").unwrap().tooltip();
        assert!(tip.starts_with("Length  (Shift+Alt+L)"), "{tip}");
        assert!(tip.contains("straight run"), "{tip}");
    }

    #[test]
    fn a_command_nobody_wrote_a_label_for_still_shows_something() {
        assert_eq!(label_of("Some.Unknown.Thing"), "Thing");
        assert_eq!(label_of("Measure.Length"), "Length");
    }

    #[test]
    fn the_tools_that_stay_pressed_are_the_ones_you_draw_with() {
        assert!(find("Measure.Area").unwrap().is_tool());
        assert!(find("Markup.Pen").unwrap().is_tool());
        assert!(find("Pan").unwrap().is_tool());
        assert!(!find("Document.Save").unwrap().is_tool());
        assert!(!find("View.InvertColors").unwrap().is_tool(), "a setting is not a tool");
    }
}

/// A shortcut, as the profile writes it: `Ctrl+Shift+S`, `J`, `F3`, `Del`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// The key itself, upper case and without modifiers.
    pub key: &'static str,
}

impl Command {
    /// The shortcut broken up, so a key press can be matched against it.
    ///
    /// Read from the same string the tooltip shows. One place decides what a
    /// command's shortcut is, which is why adding a command gets it a working
    /// shortcut rather than a line in a table somebody has to remember.
    pub fn keys(&self) -> Option<Shortcut> {
        let text = self.shortcut?;
        let mut out = Shortcut {
            ctrl: false,
            shift: false,
            alt: false,
            key: "",
        };
        for part in text.split('+') {
            match part.trim() {
                "Ctrl" | "ctrl" => out.ctrl = true,
                "Shift" | "shift" => out.shift = true,
                "Alt" | "alt" => out.alt = true,
                key => out.key = key,
            }
        }
        (!out.key.is_empty()).then_some(out)
    }
}

/// Every command with a shortcut, so a key press can be looked up once.
pub fn shortcuts() -> Vec<(Shortcut, &'static str)> {
    ALL.iter()
        .filter_map(|c| c.keys().map(|k| (k, c.id)))
        .collect()
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn a_shortcut_is_read_the_way_a_profile_writes_it() {
        let ctrl_shift_s = find("File.SaveAs").unwrap().keys().unwrap();
        assert!(ctrl_shift_s.ctrl && ctrl_shift_s.shift);
        assert_eq!(ctrl_shift_s.key, "S");

        let plain = find("Measure.DynamicFill").unwrap().keys().unwrap();
        assert!(!plain.ctrl && !plain.shift);
        assert_eq!(plain.key, "J");
    }

    #[test]
    fn a_command_with_no_shortcut_has_no_keys() {
        assert!(find("File.CreatePDF").unwrap().keys().is_none());
    }

    #[test]
    fn no_two_commands_claim_the_same_shortcut() {
        let all = shortcuts();
        let mut seen: std::collections::BTreeMap<String, &str> = Default::default();
        for (keys, id) in all {
            let name = format!(
                "{}{}{}{}",
                if keys.ctrl { "Ctrl+" } else { "" },
                if keys.shift { "Shift+" } else { "" },
                if keys.alt { "Alt+" } else { "" },
                keys.key
            );
            if let Some(other) = seen.insert(name.clone(), id) {
                panic!("{name} is claimed by both {other} and {id}");
            }
        }
    }
}
