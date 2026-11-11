//! Words and pictures from other programs, put on a sheet.
//!
//! Ctrl+V, Edit → Paste, and a picture file let go over a sheet all end up
//! here. Words become a text box and a picture becomes an image markup, the
//! same markups the Text and Image tools make, on whatever sheet is showing:
//! a sheet of a drawing, or a blank sheet added to the set, which has no
//! drawing of its own at all.
//!
//! Before this, Paste only ever put down markups copied inside the program.
//! What another program had copied never reached a sheet, and on a desktop
//! the keyboard's Ctrl+V did not reach the program at all: the window layer
//! turns the keys into a "paste" of the copied words and passes no key on, and
//! the shortcut was listening for the key.

use crate::sheet::{Doc, Mark};

/// The widest a pasted text box starts, in points: six inches. Longer lines
/// wrap inside it rather than run across the sheet.
const WIDEST_WORDS: f64 = 432.0;

/// Points to a pixel: a picture is put down at the size it is on a screen.
const POINT_PER_PIXEL: f64 = 0.75;

/// What is on the operating system's clipboard, as far as a sheet cares.
#[derive(Clone, Debug, PartialEq)]
pub enum Outside {
    Words(String),
    Picture(image::RgbaImage),
    Nothing,
}

/// What the operating system's clipboard holds now. Words first: a program
/// that copies both words and a picture of them, as a spreadsheet does, is
/// usually being copied from for its words.
pub fn outside() -> Outside {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        let Ok(mut board) = arboard::Clipboard::new() else {
            return Outside::Nothing;
        }; // WHAT: the clipboard, if there is one to open. WHY: a machine with no window system has none.
        if let Ok(words) = board.get_text() {
            if !words.trim().is_empty() {
                return Outside::Words(words); // WHAT: the words copied. WHY: they go down as a text box.
            }
        }
        if let Ok(picture) = board.get_image() {
            let (w, h) = (picture.width as u32, picture.height as u32); // WHAT: the picture's size. WHY: the pixels are read into an image of it.
            if let Some(image) = image::RgbaImage::from_raw(w, h, picture.bytes.into_owned()) {
                return Outside::Picture(image); // WHAT: the picture copied. WHY: it goes down as an image markup.
            }
        }
        Outside::Nothing
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        Outside::Nothing // WHAT: nothing read here on a tablet. WHY: its words arrive as egui's paste, and the clipboard library has no tablet side.
    }
}

/// What a paste puts down, given what the operating system's clipboard holds
/// and the words copying markups here put there: those words coming back are
/// the markups, and so is an empty clipboard. `Nothing` means the markups.
pub fn pick(outside: Outside, said: Option<&str>) -> Outside {
    match outside {
        Outside::Words(words) if said == Some(words.as_str()) => Outside::Nothing, // WHAT: the program's own copy coming back. WHY: the markups go down, not their words.
        Outside::Words(words) if words.trim().is_empty() => Outside::Nothing, // WHAT: no words to speak of. WHY: nothing to make a text box of.
        other => other,
    }
}

/// Puts words down as a text box on the sheet showing, centred on a point in
/// sheet space, in the toolbar's look. Comes back with the new markup's place
/// in the list, or `None` when there were no words.
pub fn words_onto(
    doc: &mut Doc,
    at: [f64; 2],
    words: &str,
    pen: &crate::pen::Pen,
) -> Option<usize> {
    let words = crate::typing::editable(words); // WHAT: every line break a \n. WHY: words pasted from Windows bring \r\n, which would show as a blank line between every line.
    if words.trim().is_empty() {
        return None; // WHAT: nothing to put down. WHY: an empty box is a markup nobody asked for.
    }
    let page = doc.page;
    let frame = doc.frame_of(page);
    let draft = crate::sheet::Draft {
        tool: crate::app::Tool::Text,
        points: vec![at, [at[0] + 72.0, at[1] + 20.0]],
        subject: crate::app::Tool::Text.label().to_string(),
        colour: pen.line,
        pen: Some(pen.clone()),
        ..crate::sheet::Draft::default()
    }; // WHAT: a text box as the Text tool makes one. WHY: the same markup, so it is moved, sized and typed into like any other.
    let mut markup = draft.into_markup(&frame, None)?;
    markup.set_contents(&words);
    crate::grips::fit_to_words(&mut markup, &words); // WHAT: the box made the size of its words. WHY: as a box put down with a click grows.
    if let Some(b) = markup.drawn_box() {
        if b[2] - b[0] > WIDEST_WORDS {
            markup.set_box([b[0], b[1], b[0] + WIDEST_WORDS, b[3]]); // WHAT: no wider than six inches. WHY: a pasted paragraph is one long line, and would run across the sheet.
            crate::grips::taller_for_words(&mut markup, &words); // WHAT: as tall as the wrapped lines need. WHY: so none are cut off.
        }
    }
    centre_on(&mut markup, frame.to_pdf(at));
    doc.checkpoint_named("Paste words");
    doc.marks.push(Mark::new(page, markup));
    let index = doc.marks.len() - 1;
    doc.choose(Some(index));
    doc.dirty = true;
    Some(index)
}

/// Puts a picture down as an image markup on the sheet showing, centred on a
/// point in sheet space, at its size on a screen and never more than half the
/// sheet.
pub fn picture_onto(
    doc: &mut Doc,
    at: [f64; 2],
    picture: annot::markup::Picture,
    pen: &crate::pen::Pen,
) -> Option<usize> {
    if picture.width == 0 || picture.height == 0 {
        return None; // WHAT: a picture with no pixels. WHY: nothing to show.
    }
    let page = doc.page;
    let frame = doc.frame_of(page);
    let (sheet_w, sheet_h) = frame.size();
    let (mut w, mut h) = (
        picture.width as f64 * POINT_PER_PIXEL,
        picture.height as f64 * POINT_PER_PIXEL,
    ); // WHAT: its size on a screen, in points. WHY: a screenshot pasted comes out the size it was seen.
    let fits = (sheet_w * 0.5 / w).min(sheet_h * 0.5 / h).min(1.0); // WHAT: how much smaller it has to be to fit half the sheet. WHY: a big photo pasted on a letter-size sheet would cover it and run off.
    w *= fits;
    h *= fits;
    let draft = crate::sheet::Draft {
        tool: crate::app::Tool::Image,
        points: vec![
            [at[0] - w * 0.5, at[1] - h * 0.5],
            [at[0] + w * 0.5, at[1] + h * 0.5],
        ],
        subject: crate::app::Tool::Image.label().to_string(),
        colour: pen.line,
        picture: Some(picture),
        ..crate::sheet::Draft::default()
    }; // WHAT: an image markup as the Image tool makes one. WHY: the same markup, so it is moved and sized like any other.
    let mut markup = draft.into_markup(&frame, None)?;
    markup.set_width(0.0); // WHAT: no border round the picture. WHY: a pasted picture is the picture, not a box.
    doc.checkpoint_named("Paste a picture");
    doc.marks.push(Mark::new(page, markup));
    let index = doc.marks.len() - 1;
    doc.choose(Some(index));
    doc.dirty = true;
    Some(index)
}

/// Whether a file is a picture that can go on a sheet.
pub fn is_picture(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg"))
    // WHAT: PNG and JPEG. WHY: the two the picture reader is built for.
}

/// Moves a markup so the middle of its box is on a point in PDF space.
fn centre_on(markup: &mut annot::Markup, at: [f64; 2]) {
    if let Some(b) = markup.drawn_box() {
        markup.move_by(at[0] - (b[0] + b[2]) * 0.5, at[1] - (b[1] + b[3]) * 0.5);
        // WHAT: shifted by the difference. WHY: the box keeps its size.
    }
}

impl crate::app::App {
    /// Paste: whatever was copied last goes on the sheet showing.
    ///
    /// `said` is the words egui's paste brought, when it brought some; with
    /// none, the operating system's clipboard is read here. Words copied from
    /// the program's own markups are the program's own copy coming back, and
    /// put the markups down; any other words make a text box, a picture makes
    /// an image markup, and with nothing outside the markups copied here go
    /// down as before.
    pub fn paste_anything(&mut self, in_place: bool, said: Option<String>) {
        let outside = match said {
            Some(words) if !words.trim().is_empty() => Outside::Words(words), // WHAT: the words egui's paste brought. WHY: already read from the clipboard.
            _ => outside(),
        };
        match pick(outside, self.clipboard.said.as_deref()) {
            Outside::Words(words) => self.paste_words(&words),
            Outside::Picture(image) => self.paste_picture(crate::picture::from_image(&image), None),
            Outside::Nothing => self.paste(in_place), // WHAT: the markups copied here. WHY: nothing newer was copied anywhere else.
        }
        self.save_soon();
    }

    /// Where something pasted goes: the pointer, when it is over the sheet,
    /// or else the middle of the sheet as it is seen.
    pub fn paste_point(&self) -> Option<[f64; 2]> {
        let doc = self.doc()?;
        let canvas = self.last_canvas;
        let over = self
            .pointer_now
            .filter(|p| canvas.contains(*p))
            .unwrap_or(canvas.center()); // WHAT: the pointer if it is over the sheet. WHY: Ctrl+V puts it where somebody is pointing; the menu, clicked away from the sheet, puts it in the middle.
        let s = doc.view.to_sheet(over);
        Some([s[0] as f64, s[1] as f64])
    }

    fn paste_words(&mut self, words: &str) {
        let Some(at) = self.paste_point() else {
            self.status = "Open a drawing to paste onto.".into();
            return;
        };
        let pen = self.pen.clone();
        let Some(doc) = self.doc_mut() else { return };
        if words_onto(doc, at, words, &pen).is_some() {
            self.status =
                "The words copied are on the sheet as a text box. Double-click it to change them."
                    .into();
        }
    }

    /// Puts a picture down at a point on the screen, or where Paste puts things.
    pub fn paste_picture(&mut self, picture: annot::markup::Picture, at: Option<egui::Pos2>) {
        let point = match (at, self.doc()) {
            (Some(p), Some(doc)) if self.last_canvas.contains(p) => {
                let s = doc.view.to_sheet(p);
                Some([s[0] as f64, s[1] as f64])
            }
            _ => self.paste_point(),
        };
        let Some(point) = point else {
            self.status = "Open a drawing to paste onto.".into();
            return;
        };
        let pen = self.pen.clone();
        let Some(doc) = self.doc_mut() else { return };
        if picture_onto(doc, point, picture, &pen).is_some() {
            self.status = "The picture is on the sheet. Drag its corners to size it.".into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reorder::Plan;

    fn sample() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../mobile/samples/Northgate Warehouse - Structural Set.pdf")
    }

    /// A copy of the sample set to change, in a folder of its own.
    fn copy_of_sample(name: &str) -> std::path::PathBuf {
        // WHAT: in the build's own folder for this crate. WHY: not the shared temporary folder, which other people on the machine can write to.
        let beside = std::path::PathBuf::from(env!("OUT_DIR"));
        let dir = beside.join(format!("excalibur-pasted-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("set.pdf");
        std::fs::copy(sample(), &path).unwrap();
        path
    }

    /// The sample with a blank sheet added after its first, showing it.
    fn with_a_blank_sheet(name: &str) -> (std::path::PathBuf, Doc) {
        let path = copy_of_sample(name);
        let mut doc = Doc::open(path.clone()).unwrap();
        doc.change_sheets(&Plan::Blank { before: 1 }, "Test")
            .unwrap();
        assert_eq!(doc.page, 1, "the blank sheet is the one showing");
        (path, doc)
    }

    fn middle(doc: &Doc) -> [f64; 2] {
        let (w, h) = doc.frame().size();
        [w * 0.5, h * 0.5]
    }

    /// The raw bytes of a markup's /Contents as the file has them.
    fn contents_bytes(markup: &annot::Markup) -> Vec<u8> {
        markup
            .dict
            .get("Contents")
            .and_then(|o| o.as_string())
            .map(|s| s.to_vec())
            .unwrap_or_default()
    }

    #[test]
    fn a_paste_puts_down_whatever_was_copied_last() {
        let red = image::RgbaImage::from_pixel(2, 2, image::Rgba([220, 30, 30, 255]));
        let said = Some("BEAM ABOVE");
        assert_eq!(
            pick(Outside::Words("BEAM ABOVE".into()), said),
            Outside::Nothing,
            "the markups copied here coming back"
        );
        assert_eq!(
            pick(Outside::Words("RFI 014".into()), said),
            Outside::Words("RFI 014".into()),
            "words copied since in another program"
        );
        assert_eq!(
            pick(Outside::Picture(red.clone()), said),
            Outside::Picture(red),
            "a picture copied since"
        );
        assert_eq!(pick(Outside::Nothing, said), Outside::Nothing);
        assert_eq!(
            pick(Outside::Words("RFI 014".into()), None),
            Outside::Words("RFI 014".into()),
            "nothing copied here"
        );
    }

    const TRICKY: &str =
        "W12x26 @ 16\" O.C. ±½\" TYP — 90°\nSEE (DETAIL 3) \\ S-501  😀 Café\n\n   INDENTED";

    #[test]
    fn words_pasted_onto_a_blank_sheet_are_there_after_a_save_and_a_reopen() {
        let (path, mut doc) = with_a_blank_sheet("words");
        let at = middle(&doc);
        let index = words_onto(&mut doc, at, TRICKY, &crate::pen::Pen::default())
            .expect("the words go down");
        assert_eq!(doc.marks[index].page, 1);
        assert_eq!(doc.marks[index].markup.subtype(), annot::Subtype::FreeText);
        doc.save("Test").unwrap();
        let read = Doc::open(path.clone()).unwrap();
        let on_blank: Vec<_> = read.marks.iter().filter(|m| m.page == 1).collect();
        assert_eq!(on_blank.len(), 1, "one markup on the blank sheet");
        assert_eq!(
            on_blank[0].markup.contents(),
            TRICKY,
            "the words come back exactly"
        );
        let b = on_blank[0].markup.drawn_box().unwrap();
        let mid = read.frame_of(1).to_pdf(at);
        assert!(
            (b[0] + b[2]) * 0.5 - mid[0] < 0.5 && (b[1] + b[3]) * 0.5 - mid[1] < 0.5,
            "centred where it was put"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_picture_pasted_onto_a_blank_sheet_and_onto_a_drawing_is_there_after_a_reopen() {
        let (path, mut doc) = with_a_blank_sheet("picture");
        let red = image::RgbaImage::from_pixel(40, 20, image::Rgba([220, 30, 30, 255]));
        let at = middle(&doc);
        let index = picture_onto(
            &mut doc,
            at,
            crate::picture::from_image(&red),
            &crate::pen::Pen::default(),
        )
        .unwrap();
        let b = doc.marks[index].markup.drawn_box().unwrap();
        assert!(
            ((b[2] - b[0]) - 30.0).abs() < 0.01 && ((b[3] - b[1]) - 15.0).abs() < 0.01,
            "its size on a screen: {b:?}"
        );
        doc.page = 0;
        let at = middle(&doc);
        picture_onto(
            &mut doc,
            at,
            crate::picture::from_image(&red),
            &crate::pen::Pen::default(),
        )
        .unwrap();
        doc.save("Test").unwrap();
        let read = Doc::open(path.clone()).unwrap();
        for page in [0, 1] {
            let pictures: Vec<_> = read
                .marks
                .iter()
                .filter(|m| {
                    m.page == page
                        && m.markup.subtype() == annot::Subtype::Stamp
                        && m.markup.dict.has("AP")
                })
                .collect();
            assert!(
                !pictures.is_empty(),
                "a picture with its appearance on sheet {page}"
            );
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_huge_picture_is_put_down_no_bigger_than_half_the_sheet() {
        let (path, mut doc) = with_a_blank_sheet("huge");
        let big = image::RgbaImage::from_pixel(8000, 2000, image::Rgba([0, 0, 0, 255]));
        let at = middle(&doc);
        let index = picture_onto(
            &mut doc,
            at,
            crate::picture::from_image(&big),
            &crate::pen::Pen::default(),
        )
        .unwrap();
        let b = doc.marks[index].markup.drawn_box().unwrap();
        let (w, _) = doc.frame().size();
        assert!(b[2] - b[0] <= w * 0.5 + 0.01, "{b:?}");
        assert!(
            (((b[2] - b[0]) / (b[3] - b[1])) - 4.0).abs() < 0.01,
            "its shape is kept"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_long_line_pasted_wraps_in_a_box_six_inches_wide() {
        let (path, mut doc) = with_a_blank_sheet("long");
        let long = "FIELD VERIFY ALL DIMENSIONS BEFORE FABRICATION ".repeat(8);
        let at = middle(&doc);
        let index = words_onto(&mut doc, at, &long, &crate::pen::Pen::default()).unwrap();
        let b = doc.marks[index].markup.drawn_box().unwrap();
        assert!(b[2] - b[0] <= WIDEST_WORDS + 0.01, "{b:?}");
        let setting = annot::text::Setting::of(&doc.marks[index].markup);
        let lines = annot::text::wrap(
            &long,
            b[2] - b[0] - 4.0 - 2.0 * doc.marks[index].markup.width().max(0.0),
            &setting,
        );
        assert!(
            b[3] - b[1] + 0.01 >= lines.len() as f64 * setting.size * 1.18,
            "tall enough for every line"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn windows_line_breaks_pasted_are_one_break_each() {
        let (path, mut doc) = with_a_blank_sheet("crlf");
        let at = middle(&doc);
        let index =
            words_onto(&mut doc, at, "FIRST\r\nSECOND", &crate::pen::Pen::default()).unwrap();
        assert_eq!(doc.marks[index].markup.contents(), "FIRST\nSECOND");
        assert_eq!(
            words_onto(&mut doc, at, "  \r\n ", &crate::pen::Pen::default()),
            None,
            "nothing to put down"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_text_box_edited_after_a_reopen_comes_back_byte_for_byte_through_font_and_size_changes() {
        // WHAT: a text box saved, reopened, typed into, given a new font and size, saved and reopened again, on a drawing and on a blank sheet. WHY: the words must come back exactly as typed, every time.
        let (path, mut doc) = with_a_blank_sheet("roundtrip");
        for page in [0u32, 1] {
            doc.page = page;
            let at = middle(&doc);
            words_onto(&mut doc, at, "FIRST", &crate::pen::Pen::default()).unwrap();
        }
        doc.save("Test").unwrap();
        let first = contents_bytes(
            &doc.marks
                .iter()
                .find(|m| m.page == 0 && m.markup.contents() == "FIRST")
                .unwrap()
                .markup,
        );
        drop(doc);
        let mut doc = Doc::open(path.clone()).unwrap();
        for (i, family) in [
            (0usize, annot::text::Family::Courier),
            (1, annot::text::Family::Times),
        ] {
            let index = doc
                .marks
                .iter()
                .position(|m| m.page == i as u32 && m.markup.contents() == "FIRST")
                .unwrap();
            let mark = &mut doc.marks[index];
            mark.markup.set_contents(TRICKY);
            let mut setting = annot::text::Setting::of(&mark.markup);
            setting.family = family;
            setting.size = 21.5;
            setting.bold = true;
            setting.onto(&mut mark.markup);
            mark.changed = true;
            doc.dirty = true;
        }
        let typed: Vec<u8> = {
            let mut m = annot::Markup::new(annot::Subtype::FreeText);
            m.set_contents(TRICKY);
            contents_bytes(&m)
        };
        doc.save("Test").unwrap();
        let read = Doc::open(path.clone()).unwrap();
        for page in [0u32, 1] {
            let mark = read
                .marks
                .iter()
                .find(|m| {
                    m.page == page
                        && m.markup.subtype() == annot::Subtype::FreeText
                        && m.markup.contents() != ""
                })
                .unwrap();
            assert_eq!(mark.markup.contents(), TRICKY, "sheet {page}");
            assert_eq!(
                contents_bytes(&mark.markup),
                typed,
                "sheet {page}: the same bytes"
            );
            let setting = annot::text::Setting::of(&mark.markup);
            assert_eq!(setting.size, 21.5);
            assert!(setting.bold);
        }
        assert!(!first.is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
