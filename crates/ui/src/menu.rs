//! The menu bar.
//!
//! The arrangement is Revu's, taken from the user's own menus, because a person
//! who has used Revu for years should find things where their hand already goes.
//! Items Excalibur Hyperview cannot do yet are listed and shown greyed rather
//! than left out, so nobody hunts for something that is simply not there.


#[derive(Clone, Copy, Debug)]
pub enum Entry {
    /// A command, by identifier.
    Item(&'static str),
    /// A command shown under a different name here than on its toolbar.
    Named(&'static str, &'static str),
    /// A line between groups.
    Line,
    /// A submenu.
    More(&'static str, &'static [Entry]),
    /// Something not built yet. Shown greyed, with the reason on hover.
    Later(&'static str, &'static str),
}

pub struct Menu {
    pub name: &'static str,
    pub entries: &'static [Entry],
}

pub const MARKUP: &[Entry] = &[
    Entry::Item("Markup.TextBox"),
    Entry::Item("Markup.Callout"),
    Entry::Item("Markup.Note"),
    Entry::Item("Markup.Flag"),
    Entry::Line,
    Entry::Item("Markup.Pen"),
    Entry::Item("Markup.Highlight"),
    Entry::Item("Eraser"),
    Entry::Line,
    Entry::Item("Markup.Line"),
    Entry::Item("Markup.Arrow"),
    Entry::Item("Markup.Arc"),
    Entry::Item("Markup.Polyline"),
    Entry::Item("Markup.Dimension"),
    Entry::Line,
    Entry::Item("Markup.Rectangle"),
    Entry::Item("Markup.Ellipse"),
    Entry::Item("Markup.Polygon"),
    Entry::Item("Markup.Cloud"),
    Entry::Item("Markup.CloudPlus"),
    Entry::Item("Markup.Cloud9"),
    Entry::Line,
    Entry::Item("Markup.Image"),
    Entry::Item("Markup.Signature"),
    Entry::Item("Snapshot"),
    Entry::Item("Markup.Redaction"),
];

pub const MEASURE: &[Entry] = &[
    Entry::Named("Measure.Calibrate", "Set Scale…"),
    Entry::Line,
    Entry::Item("Measure.Length"),
    Entry::Item("Measure.Polylength"),
    Entry::Item("Measure.Area"),
    Entry::Item("Measure.Perimeter"),
    Entry::Item("Measure.Diameter"),
    Entry::Item("Measure.Radius"),
    Entry::Named("Measure.Radius", "3-Point Radius"),
    Entry::Item("Measure.Angle"),
    Entry::Item("Measure.Volume"),
    Entry::Line,
    Entry::Item("Measure.AreaCutout"),
    Entry::Item("Measure.AreaEllipseCutout"),
    Entry::Line,
    Entry::Item("Measure.Count"),
    Entry::Item("Measure.DynamicFill"),
];

/// Dynamic Fill's own working entries. Not measurement tools — one draws a
/// boundary to close an opening and the other turns a filled shape into a
/// measurement — so they sit below Revu's fourteen rather than among them.
pub const FILLING: &[Entry] = &[
    Entry::Item("Measure.FillBoundary"),
    Entry::Item("Measure.ApplyFill"),
];

pub const SKETCH: &[Entry] = &[
    Entry::Item("Markup.PolygonSketchToScale"),
    Entry::Item("Markup.RectangleSketchToScale"),
    Entry::Item("Markup.EllipseSketchToScale"),
    Entry::Item("Markup.PolylineSketchToScale"),
];

pub const FORM: &[Entry] = &[
    Entry::Item("Form.Editor"),
    Entry::Line,
    Entry::Item("Form.TextField"),
    Entry::Item("Form.RadioButton"),
    Entry::Item("Form.CheckBox"),
    Entry::Item("Form.ListBox"),
    Entry::Item("Form.ComboBox"),
    Entry::Item("Form.Button"),
    Entry::Item("Form.DigitalSignature"),
];

pub const FILE: &[Entry] = &[
    Entry::Named("Document.New", "New PDF…"),
    Entry::Item("File.CreatePDF"),
    Entry::Named("File.Open", "Open…"),
    Entry::Named("File.OpenRecent", "Open Recent…"),
    Entry::Named("File.OpenModel", "Open Model (IFC)…"),
    Entry::Named("Server.Connect", "Connect to a Server…"),
    Entry::Named("File.CombinePDFs", "Combine…"),
    Entry::Line,
    Entry::Named("File.LoadChest", "Load Tool Chest…"),
    Entry::Named("File.SaveChest", "Save Tool Chest As…"),
    Entry::Line,
    Entry::Item("File.Close"),
    Entry::Item("File.CloseAll"),
    Entry::Line,
    Entry::Item("Document.Save"),
    Entry::Named("File.SaveAs", "Save As…"),
    Entry::Item("Document.SaveAll"),
    Entry::Named("File.Revert", "Revert"),
    Entry::Line,
    Entry::Named("Split.Email", "Email…"),
    Entry::Named("Document.Export", "Export…"),
    Entry::Named("Document.Print", "Print…"),
];

pub const EDIT: &[Entry] = &[
    Entry::Item("Edit.Undo"),
    Entry::Item("Edit.Redo"),
    Entry::Named("Edit.UndoHistory", "Undo History…"),
    Entry::Line,
    Entry::Item("Edit.Cut"),
    Entry::Item("Edit.Copy"),
    Entry::Item("Edit.Paste"),
    Entry::Item("Edit.PasteInPlace"),
    Entry::Line,
    Entry::Named("Edit.Offset", "Offset…"),
    Entry::Named("Edit.Multiply", "Multiply…"),
    Entry::Item("Delete"),
    Entry::Line,
    Entry::Item("Pan"),
    Entry::Item("Select"),
    Entry::Item("Edit.SelectAll"),
    Entry::Item("Lasso"),
    Entry::Line,
    Entry::Item("Snapshot"),
    Entry::Item("Markup.FormatPainter"),
    Entry::Item("Edit.SelectText"),
    Entry::Item("Edit.Text"),
    Entry::Line,
    Entry::More("Arrange", ARRANGE),
    Entry::More("Align", ALIGN),
    Entry::More("Edit Points", POINTS),
    Entry::Line,
    Entry::Item("SpellCheck"),
    Entry::Line,
    Entry::Named("File.Preferences", "Preferences…"),
];

pub const ARRANGE: &[Entry] = &[
    Entry::Item("Markup.BringToFront"),
    Entry::Item("Markup.BringForward"),
    Entry::Item("Markup.SendBackward"),
    Entry::Item("Markup.SendToBack"),
];

pub const ALIGN: &[Entry] = &[
    Entry::Item("Align.Left"),
    Entry::Item("Align.Center"),
    Entry::Item("Align.Right"),
    Entry::Item("Align.Top"),
    Entry::Item("Align.Middle"),
    Entry::Item("Align.Bottom"),
    Entry::Line,
    Entry::Item("Align.SpacingHorizontal"),
    Entry::Item("Align.SpacingVertical"),
    Entry::Line,
    Entry::Item("Align.Width"),
    Entry::Item("Align.Height"),
    Entry::Item("Align.Size"),
    Entry::Line,
    Entry::Item("Align.FlipHorizontal"),
    Entry::Item("Align.FlipVertical"),
    Entry::Item("Align.CenterDocument"),
];

pub const POINTS: &[Entry] = &[
    Entry::Item("ControlPoint.AddMode"),
    Entry::Item("ControlPoint.SubtractMode"),
    Entry::Item("ControlPoint.ConvertMode"),
];

pub const VIEW: &[Entry] = &[
    Entry::Item("View.SimpleMode"), // WHAT: simple mode, first in the menu. WHY: the way back to the toolbars has to be easy to find.
    Entry::Item("View.QuickTools"), // WHAT: the quick tools bar on or off. WHY: beside the other choice about what the window shows.
    Entry::Item("View.ChooseQuickTools"), // WHAT: setting the quick tools up. WHY: next to the switch for them.
    Entry::Line,
    Entry::Item("View.FitPage"),
    Entry::Item("View.FitWidth"),
    Entry::Item("View.ActualSize"),
    Entry::Line,
    Entry::Item("View.Single"),
    Entry::Item("View.Continuous"),
    Entry::Item("View.SideBySide"),
    Entry::Item("View.SideBySideContinuous"),
    Entry::Line,
    Entry::Item("Document.RotateClockwise"),
    Entry::Item("Document.RotateCounterclockwise"),
    Entry::Line,
    Entry::Item("View.Split"),
    Entry::Item("View.SplitHorizontal"),
    Entry::Item("View.UnSplit"),
    Entry::Item("View.SyncPanes"),
    Entry::Named("View.SyncPanes", "Synchronise Views"),
    Entry::Line,
    Entry::Item("View.Rulers"),
    Entry::Item("View.Crosshair"),
    Entry::Item("View.MarkupText"),
    Entry::Item("View.ShowGrid"),
    Entry::Item("View.SnapToGrid"),
    Entry::Item("View.SnapToContent"),
    Entry::Item("View.SnapToMarkup"),
    Entry::Line,
    Entry::Item("View.InvertColors"),
    Entry::Item("View.ClassicLook"),
    Entry::Item("Split.Dimmer"),
    Entry::Line,
    Entry::Item("Toggle.FullScreen"),
];

pub const DOCUMENT: &[Entry] = &[
    Entry::Item("Document.Properties"),
    Entry::Line,
    Entry::Named("Document.Combine", "Combine…"),
    Entry::Named("Document.RotatePages", "Rotate Pages…"),
    Entry::Named("Document.InsertPages", "Insert Pages…"),
    Entry::Named("Document.ExtractPages", "Extract Pages…"),
    Entry::Named("Document.SplitDocument", "Split Document…"),
    Entry::Named("Document.SlipSheet", "Slip Sheet…"),
    Entry::Named("Document.DeletePages", "Delete Pages…"),
    Entry::Named("Document.CropPages", "Crop Pages…"),
    Entry::Named("Document.NumberPages", "Number Pages…"),
    Entry::Named("Document.PageLabels", "Create Page Labels…"),
    Entry::Line,
    Entry::Named("Document.Stamp", "Headers and Footers…"),
    Entry::Named("Split.Security", "Security…"),
    Entry::Line,
    Entry::Named("Document.Compare", "Compare Documents…"),
    Entry::Named("Document.Overlay", "Overlay Pages…"),
    Entry::Line,
    Entry::Named("Document.Summary", "Summary…"),
    Entry::Named("Document.ShopList", "Shop List…"),
    Entry::Named("Document.Doubles", "Counted Twice?…"),
    Entry::Named("Document.RevisionCost", "Revision Cost…"),
    Entry::Named("Document.Ocr", "OCR…"),
    Entry::Line,
    Entry::Named("Document.Shrink", "Reduce File Size…"),
    Entry::Named("Document.Repair", "Repair PDF…"),
    Entry::Line,
    Entry::Named("Document.FlattenMarkups", "Flatten Markups…"),
    Entry::Named("Document.Unflatten", "Unflatten Markups…"),
    Entry::Line,
    Entry::Item("Markup.Redaction"),
    Entry::Item("Document.ApplyRedactions"),
    Entry::Named("CropImage", "Crop Image…"),
];

pub const BATCH: &[Entry] = &[
    Entry::Named("Batch.Combine", "Combine Documents…"),
    Entry::Named("Batch.Split", "Split Documents…"),
    Entry::Line,
    Entry::Named("Batch.Stamp", "Headers and Footers…"),
    Entry::Named("Batch.Rotate", "Rotate Pages…"),
    Entry::Line,
    Entry::Named("Batch.Flatten", "Flatten Markups…"),
    Entry::Named("Batch.Shrink", "Reduce File Size…"),
    Entry::Line,
    Entry::Named("Batch.Crop", "Crop and Page Setup…"),
    Entry::Named("Batch.Security", "Security…"),
    Entry::Named("Batch.Seal", "Sign and Seal…"),
    Entry::Named("Batch.ApplyStamp", "Apply Stamp…"),
    Entry::Named("Batch.Compare", "Compare Documents…"),
    Entry::Named("Batch.Overlay", "Overlay Pages…"),
    Entry::Named("Batch.SlipSheet", "Slip Sheet…"),
    Entry::Named("Batch.Ocr", "OCR…"),
    Entry::Named("Batch.Print", "Print…"),
    Entry::Named("Batch.Summary", "Summary…"),
];

pub const TOOLS: &[Entry] = &[
    Entry::More("Markup", MARKUP),
    Entry::More("Measure", MEASURE),
    Entry::More("Dynamic Fill", FILLING),
    Entry::More("Sketch to Scale", SKETCH),
    Entry::More("Form", FORM),
    Entry::Line,
    Entry::Item("Markup.Hyperlink"),
    Entry::Item("Markup.FileAttachment"),
    Entry::Item("Space.Add"),
    Entry::Line,
    Entry::Named("Document.Seal", "Sign and Seal…"),
    Entry::Named("Document.Fingerprint", "Fingerprint This Set…"),
];

pub const HELP: &[Entry] = &[
    Entry::Item("Help.Shortcuts"),
    Entry::Line,
    Entry::Item("Help.ConnectAssistant"),
    Entry::Item("Help.TestAssistant"),
    Entry::Item("Help.CheckForUpdates"),
    Entry::Item("Help.About"),
];

pub const BAR: &[Menu] = &[
    Menu { name: "File", entries: FILE },
    Menu { name: "Edit", entries: EDIT },
    Menu { name: "View", entries: VIEW },
    Menu { name: "Document", entries: DOCUMENT },
    Menu { name: "Batch", entries: BATCH },
    Menu { name: "Tools", entries: TOOLS },
    Menu { name: "Help", entries: HELP },
];

/// Walks every entry, including the ones inside submenus.
pub fn walk(entries: &'static [Entry], visit: &mut impl FnMut(&'static Entry)) {
    for entry in entries {
        visit(entry);
        if let Entry::More(_, inner) = entry {
            walk(inner, visit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command;

    #[test]
    fn every_menu_item_names_a_command_that_exists() {
        for menu in BAR {
            walk(menu.entries, &mut |entry| match entry {
                Entry::Item(id) | Entry::Named(id, _) => {
                    assert!(
                        command::find(id).is_some(),
                        "{} in the {} menu names no command",
                        id,
                        menu.name
                    );
                }
                _ => {}
            });
        }
    }

    #[test]
    fn nothing_that_is_not_built_pretends_otherwise() {
        for menu in BAR {
            walk(menu.entries, &mut |entry| {
                if let Entry::Later(label, why) = entry {
                    assert!(!label.is_empty());
                    assert!(
                        why.ends_with('.') && why.len() > 20,
                        "{label} should say plainly why: {why:?}"
                    );
                }
            });
        }
    }

    #[test]
    fn the_menus_are_the_ones_revu_has() {
        let names: Vec<&str> = BAR.iter().map(|m| m.name).collect();
        assert_eq!(
            names,
            vec!["File", "Edit", "View", "Document", "Batch", "Tools", "Help"]
        );
    }

    #[test]
    fn the_measure_menu_offers_all_fourteen_tools() {
        let mut found = 0;
        walk(MEASURE, &mut |e| {
            if matches!(e, Entry::Item(_) | Entry::Named(_, _)) {
                found += 1;
            }
        });
        assert_eq!(found, 14, "Revu's Measure menu has fourteen entries");
    }

    #[test]
    fn dynamic_fill_s_own_entries_are_not_counted_among_the_tools() {
        // They belong to the workflow, not to the list of ways to measure —
        // which is why they are kept apart rather than padding the fourteen.
        for entry in FILLING {
            let Entry::Item(id) = entry else {
                panic!("only commands belong here");
            };
            assert!(command::find(id).is_some(), "{id} names no command");
            assert!(
                !MEASURE.iter().any(|e| matches!(e, Entry::Item(other) if other == id)),
                "{id} must not also be in the tool list"
            );
        }
    }

    #[test]
    fn no_menu_starts_or_ends_with_a_dividing_line() {
        for menu in BAR {
            assert!(
                !matches!(menu.entries.first(), Some(Entry::Line)),
                "{} starts with a line",
                menu.name
            );
            assert!(
                !matches!(menu.entries.last(), Some(Entry::Line)),
                "{} ends with a line",
                menu.name
            );
        }
    }
}
