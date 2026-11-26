//! Edit Text, checked against the sheet as pdfium draws it.
//!
//! Every case here is a piece of lettering the way real drawings carry it: a
//! title, a numbered note, CAD letters set one at a time, kerning in a TJ, a
//! drawing scaled by its own `cm`, squeezed by `Tz`, spaced by `Tc`, turned,
//! inside a Form XObject (how a flattened stamp or markup is kept), in a CID
//! font with a ligature, in a Type3 font, an OCR layer, and lettering that is
//! only lines. Each case is drawn alone on a sheet turned 0, 90, 180 and 270
//! degrees and on a cropped sheet, and the box Edit Text puts round it has to
//! hold the ink pdfium drew, edge to edge, within a pixel and a half.
//!
//! With `EDIT_TEXT_SHOTS` set to a folder, every case is also drawn on one
//! sheet per turn with the boxes over it, as the tool shows them, so a person
//! can look.

use std::path::{Path, PathBuf}; // WHAT: file paths. WHY: fixtures are written and read as files.

/// Pixels to a point in every picture here. // WHAT: the scale. WHY: one pixel is half a point, fine enough to see a box a pixel off
const SCALE: f64 = 2.0; // WHAT: two pixels a point. WHY: the same everywhere so tolerances are in known units

/// How far a box edge may sit inside the ink, in points. // WHAT: the tolerance. WHY: 0.75 pt is a pixel and a half here; anti-aliasing alone moves an edge by a pixel
const SLACK: f64 = 0.75; // WHAT: a pixel and a half. WHY: the bar the task sets for "lines up"

/// A scratch folder that cleans up after itself. // WHAT: a temp folder. WHY: fixtures go there
struct Scratch(PathBuf); // WHAT: its path. WHY: removed when the test ends

// WHAT: making and using the folder. WHY: every test.
impl Scratch {
    // WHAT: a new folder. WHY: one per test.
    fn new(what: &str) -> Scratch {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0); // WHAT: the clock. WHY: a different folder every run
        let path = std::env::temp_dir().join(format!("hyperview-edittext-{what}-{n:x}")); // WHAT: the folder. WHY: one per test, in the temp folder
        std::fs::create_dir_all(&path).expect("a scratch folder"); // WHAT: makes it. WHY: fixtures go in it
        Scratch(path) // WHAT: hands it back. WHY: dropped at the end
    }
}

// WHAT: cleanup. WHY: nothing left behind.
impl Drop for Scratch {
    // WHAT: removes it. WHY: the same.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0); // WHAT: deletes it. WHY: tests leave nothing behind
    }
}

/// The folder pdfium is in, when it is not built in. // WHAT: where the engine is. WHY: the same guesses the other pdfium tests make
fn library() -> Option<PathBuf> {
    for guess in ["third_party/pdfium/linux-x64/lib", "../third_party/pdfium/linux-x64/lib", "../../third_party/pdfium/linux-x64/lib"] { // WHAT: each place it might be. WHY: tests run from the crate or the workspace
        let path = PathBuf::from(guess); // WHAT: one guess. WHY: tried in turn
        if path.join("libpdfium.so").exists() { // WHAT: it is there. WHY: used
            return Some(path); // WHAT: found. WHY: the engine loads it from there
        }
    }
    None // WHAT: the built-in copy. WHY: as the program does
}

/// A PDF written by hand, an object at a time. // WHAT: a tiny writer. WHY: every fixture is made here, no customer file is ever used
struct Pdf {
    bodies: Vec<Vec<u8>>, // WHAT: each object's body, numbered from 1. WHY: written out in order with an xref
}

// WHAT: adding objects and finishing the file. WHY: the fixtures.
impl Pdf {
    // WHAT: an empty file. WHY: objects are added to it.
    fn new() -> Pdf {
        Pdf { bodies: Vec::new() } // WHAT: no objects yet. WHY: numbered as they come
    }
    // WHAT: an object from its text. WHY: dictionaries and arrays.
    fn add(&mut self, body: &str) -> usize {
        self.bodies.push(body.as_bytes().to_vec()); // WHAT: kept. WHY: written later
        self.bodies.len() // WHAT: its number. WHY: referred to by others
    }
    // WHAT: a stream object. WHY: content, fonts, glyphs.
    fn stream(&mut self, dict: &str, data: &[u8]) -> usize {
        let mut body = format!("<<{dict}/Length {}>>\nstream\n", data.len()).into_bytes(); // WHAT: its dictionary. WHY: the length says where it ends
        body.extend_from_slice(data); // WHAT: the data. WHY: as given, uncompressed
        body.extend_from_slice(b"\nendstream"); // WHAT: the close. WHY: the syntax
        self.bodies.push(body); // WHAT: kept. WHY: written later
        self.bodies.len() // WHAT: its number. WHY: referred to by others
    }
    // WHAT: the whole file. WHY: written to disk.
    fn finish(&self, root: usize) -> Vec<u8> {
        let mut file = b"%PDF-1.7\n".to_vec(); // WHAT: the header. WHY: pdfium checks it
        let mut offsets = Vec::new(); // WHAT: where each object starts. WHY: the xref
        for (i, body) in self.bodies.iter().enumerate() { // WHAT: each object. WHY: in number order
            offsets.push(file.len()); // WHAT: its start. WHY: the xref points at it
            file.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes()); // WHAT: its header. WHY: the syntax
            file.extend_from_slice(body); // WHAT: its body. WHY: the object
            file.extend_from_slice(b"\nendobj\n"); // WHAT: its close. WHY: the syntax
        }
        let table = file.len(); // WHAT: where the xref starts. WHY: startxref says so
        file.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", self.bodies.len() + 1).as_bytes()); // WHAT: the xref head. WHY: the syntax
        for offset in &offsets { // WHAT: each object's entry. WHY: the xref
            file.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes()); // WHAT: one entry. WHY: twenty bytes each, as PDF wants
        }
        file.extend_from_slice(format!("trailer\n<</Size {}/Root {root} 0 R>>\nstartxref\n{table}\n%%EOF\n", self.bodies.len() + 1).as_bytes()); // WHAT: the trailer. WHY: where to start reading
        file // WHAT: the file. WHY: written by the caller
    }
}

/// One piece of lettering to try. // WHAT: a case. WHY: each is drawn alone and with the others
struct Case {
    name: &'static str, // WHAT: what it is. WHY: named in failures and shots
    content: String, // WHAT: the content-stream operators that draw it. WHY: as a real drawing would have them
    expect: Option<&'static str>, // WHAT: the words Edit Text should offer, or None for nothing to offer. WHY: OCR layers and lines aren't text anybody can retype
}

/// The words in a CID font with a ligature, as glyph codes, kerned by a TJ. // WHAT: the spec-book case. WHY: specs come in subset Type0 fonts with ligatures and TJ kerning
fn cid_show(face: &ttf_parser::Face) -> String {
    let gid = |c: char| face.glyph_index(c).map(|g| g.0).unwrap_or(0); // WHAT: a letter's glyph. WHY: Identity-H codes are glyph numbers here
    let fi = face.glyph_index('\u{FB01}').map(|g| g.0); // WHAT: the fi ligature, if the font has one. WHY: specs set "fi" as one glyph
    let mut out = String::from("["); // WHAT: a TJ array. WHY: kerned the way a typesetter writes it
    for (i, word) in ["SECTION", "05", "12", "00", "fireproofing"].iter().enumerate() { // WHAT: each word. WHY: kerned apart, no space glyphs, as many writers do
        if i > 0 { // WHAT: between words. WHY: the gap is a kern, not a space
            out.push_str(" -280 "); // WHAT: a word gap as a kern. WHY: the hardest case for finding words
        }
        out.push('<'); // WHAT: the codes start. WHY: hex string
        let mut chars = word.chars().peekable(); // WHAT: its letters. WHY: "fi" may become one glyph
        while let Some(c) = chars.next() { // WHAT: each letter. WHY: coded
            if c == 'f' && chars.peek() == Some(&'i') && fi.is_some() { // WHAT: an f then an i. WHY: set as the ligature
                chars.next(); // WHAT: the i is in the ligature. WHY: one glyph for two letters
                out.push_str(&format!("{:04X}", fi.unwrap_or(0))); // WHAT: the ligature's code. WHY: two bytes per glyph
            } else {
                out.push_str(&format!("{:04X}", gid(c))); // WHAT: the letter's code. WHY: two bytes per glyph
            }
        }
        out.push('>'); // WHAT: the codes end. WHY: hex string
        if i == 0 { // WHAT: after the first word. WHY: a small kern inside a run, as tracking
            out.push_str(" 15 <> "); // WHAT: a tiny kern and an empty string. WHY: real files have these
        }
    }
    out.push_str("] TJ"); // WHAT: shows it. WHY: one operator
    out // WHAT: the operator. WHY: put in the case's content
}

/// Every case, in where it sits on a 792 by 612 sheet. // WHAT: the fixtures. WHY: what a drawing set really carries
fn cases(face: &ttf_parser::Face) -> Vec<Case> {
    let mut cad = String::new(); // WHAT: CAD letters one object each. WHY: how many CAD writers set text
    for (i, c) in "W12X26 BEAM".chars().enumerate() { // WHAT: each letter. WHY: one text object each
        if c != ' ' { // WHAT: no object for the space. WHY: CAD skips it and just moves on
            cad.push_str(&format!("BT /F2 10 Tf 1 0 0 1 {} 440 Tm ({c}) Tj ET\n", 50 + 6 * i)); // WHAT: one letter, placed. WHY: Courier 10 is 6 points a letter
        }
    }
    vec![ // WHAT: the cases. WHY: each a way real drawings carry lettering
        Case { name: "title", content: "BT /F1 18 Tf 1 0 0 1 50 560 Tm (GENERAL NOTES) Tj ET".into(), expect: Some("GENERAL NOTES") }, // WHAT: a plain title. WHY: the baseline case
        Case { name: "note", content: "BT /F1 10 Tf 1 0 0 1 50 520 Tm (1. ALL STEEL SHALL CONFORM) Tj 0 -12 Td (TO ASTM A992 UNLESS) Tj 0 -12 Td (NOTED OTHERWISE.) Tj ET".into(), expect: Some("1. ALL STEEL SHALL CONFORM TO ASTM A992 UNLESS NOTED OTHERWISE.") }, // WHAT: a three-line note. WHY: lines become one paragraph
        Case { name: "cad-letters", content: cad, expect: Some("W12X26 BEAM") }, // WHAT: letters one at a time. WHY: glyph-by-glyph boxes are what CAD text gives
        Case { name: "tj-kern", content: "BT /F1 10 Tf 1 0 0 1 50 400 Tm [(DRAWN) -2500 (BY)] TJ ET".into(), expect: Some("DRAWN BY") }, // WHAT: a wide kern inside one TJ. WHY: a title block cell as plotted
        Case { name: "cm-scaled", content: "q 0.24 0 0 0.24 0 0 cm BT /F1 50 Tf 1 0 0 1 208.333 1500 Tm (SCALED BY CM) Tj ET Q".into(), expect: Some("SCALED BY CM") }, // WHAT: a whole drawing scaled by cm. WHY: AutoCAD and Revit plots do this
        Case { name: "tz-50", content: "q BT /F1 12 Tf 50 Tz 1 0 0 1 50 320 Tm (NARROW TZ FIFTY) Tj ET Q".into(), expect: Some("NARROW TZ FIFTY") }, // WHAT: squeezed to half width. WHY: width factors from CAD styles
        Case { name: "tc-3", content: "q BT /F1 10 Tf 3 Tc 1 0 0 1 50 280 Tm (SPACED OUT) Tj ET Q".into(), expect: Some("SPACED OUT") }, // WHAT: letter spacing. WHY: tracked titles
        Case { name: "turned-90", content: "BT /F1 12 Tf 0 1 -1 0 740 60 Tm (VERTICAL NOTE) Tj ET".into(), expect: Some("VERTICAL NOTE") }, // WHAT: running up the sheet. WHY: notes along a grid line
        Case { name: "turned-45", content: "BT /F1 12 Tf 0.7071 0.7071 -0.7071 0.7071 560 60 Tm (AT FORTY FIVE) Tj ET".into(), expect: Some("AT FORTY FIVE") }, // WHAT: at an angle. WHY: labels along a sloped member
        Case { name: "in-a-form", content: "q 1 0 0 1 400 200 cm /Fm1 Do Q".into(), expect: Some("INSIDE A FORM") }, // WHAT: text in a Form XObject. WHY: flattened stamps and markups are kept that way
        Case { name: "form-scaled", content: "q 1 0 0 1 400 150 cm /Fm2 Do Q".into(), expect: Some("FORM MATRIX TWO") }, // WHAT: in a form with its own Matrix. WHY: the form's matrix and the CTM both apply
        Case { name: "cid-ligature", content: format!("BT /F3 11 Tf 1 0 0 1 50 220 Tm {} ET", cid_show(face)), expect: Some("SECTION 05 12 00 fireproofing") }, // WHAT: a subset CID font, ligature, kerned TJ. WHY: spec books
        Case { name: "type3", content: "BT /F4 12 Tf 1 0 0 1 50 180 Tm (SHX TEST) Tj ET".into(), expect: Some("SHX TEST") }, // WHAT: a Type3 font. WHY: SHX fonts and ClearScan come as Type3
        Case { name: "ocr-layer", content: "q 0.8 g 50 100 200 40 re f 0 g BT 3 Tr /F1 12 Tf 1 0 0 1 60 115 Tm (OCR LAYER WORDS) Tj ET Q".into(), expect: None }, // WHAT: invisible OCR words over a scan. WHY: nothing anybody sees, so nothing to edit
        Case { name: "ocr-under-scan", content: "BT /F1 12 Tf 1 0 0 1 310 75 Tm (SCAN WORDS UNDER) Tj ET q 200 0 0 40 300 60 cm /Im0 Do Q".into(), expect: None }, // WHAT: visible OCR words with the scan drawn over them. WHY: some OCR writers do this; nobody sees the words
        Case { name: "shx-lines", content: "q 0 0 0 RG 1 w 300 110 m 300 130 l 312 130 l S 316 110 m 316 130 l 328 110 l 328 130 l S Q".into(), expect: None }, // WHAT: lettering drawn as lines. WHY: SHX text exported as geometry is not text
    ]
}

/// One sheet holding the given cases, turned and cropped as asked. // WHAT: a fixture file. WHY: each case alone, and all together for the shots
fn sheet(cases: &[&Case], rotate: i32, crop: Option<[f64; 4]>, face_bytes: &[u8], face: &ttf_parser::Face) -> Vec<u8> {
    let mut pdf = Pdf::new(); // WHAT: a new file. WHY: everything below goes in it
    let helvetica = pdf.add("<</Type/Font/Subtype/Type1/BaseFont/Helvetica/Encoding/WinAnsiEncoding>>"); // WHAT: Helvetica. WHY: the most common drawing font that isn't embedded
    let courier = pdf.add("<</Type/Font/Subtype/Type1/BaseFont/Courier/Encoding/WinAnsiEncoding>>"); // WHAT: Courier. WHY: fixed width, for the CAD letters
    // WHAT: the CID font: the font file, its widths, its letters back to Unicode. WHY: a spec book's font.
    let file = pdf.stream(&format!("/Length1 {}", face_bytes.len()), face_bytes); // WHAT: the font program. WHY: embedded, as subset fonts are
    let upem = f64::from(face.units_per_em()); // WHAT: font units to the em. WHY: widths are in thousandths of an em
    let mut used: Vec<(u16, String)> = Vec::new(); // WHAT: the glyphs used and what each reads as. WHY: widths and ToUnicode
    for c in "SECTION0512fireproofing".chars() { // WHAT: every letter used. WHY: each needs a width and a reading
        if let Some(g) = face.glyph_index(c) { // WHAT: its glyph. WHY: coded by glyph number
            used.push((g.0, c.to_string())); // WHAT: kept. WHY: below
        }
    }
    if let Some(g) = face.glyph_index('\u{FB01}') { // WHAT: the ligature. WHY: reads as two letters
        used.push((g.0, "fi".into())); // WHAT: kept. WHY: ToUnicode says "fi"
    }
    used.sort(); // WHAT: in glyph order. WHY: tidy, and duplicates fall together
    used.dedup(); // WHAT: each once. WHY: one entry each
    let widths: String = used.iter().map(|(g, _)| format!("{g}[{}] ", (f64::from(face.glyph_hor_advance(ttf_parser::GlyphId(*g)).unwrap_or(500)) * 1000.0 / upem).round())).collect(); // WHAT: each glyph's width. WHY: the W array, so pdfium spaces them as the font does
    let mut cmap = String::from("/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Fixture def 1 begincodespacerange <0000> <FFFF> endcodespacerange\n"); // WHAT: the ToUnicode head. WHY: so the words can be read
    cmap.push_str(&format!("{} beginbfchar\n", used.len())); // WHAT: how many entries. WHY: the syntax
    for (g, s) in &used { // WHAT: each glyph. WHY: what it reads as
        let hex: String = s.encode_utf16().map(|u| format!("{u:04X}")).collect(); // WHAT: its reading in UTF-16. WHY: ToUnicode is UTF-16BE
        cmap.push_str(&format!("<{g:04X}> <{hex}>\n")); // WHAT: one entry. WHY: the syntax
    }
    cmap.push_str("endbfchar endcmap CMapName currentdict /CMap defineresource pop end end"); // WHAT: the close. WHY: the syntax
    let tounicode = pdf.stream("", cmap.as_bytes()); // WHAT: the ToUnicode stream. WHY: referred to by the font
    let descriptor = pdf.add(&format!("<</Type/FontDescriptor/FontName/ABCDEF+Ubuntu-Light/Flags 32/FontBBox[-200 -250 1200 1000]/ItalicAngle 0/Ascent 932/Descent -189/CapHeight 693/StemV 60/FontFile2 {file} 0 R>>")); // WHAT: the descriptor. WHY: points at the font program
    let cid = pdf.add(&format!("<</Type/Font/Subtype/CIDFontType2/BaseFont/ABCDEF+Ubuntu-Light/CIDSystemInfo<</Registry(Adobe)/Ordering(Identity)/Supplement 0>>/FontDescriptor {descriptor} 0 R/CIDToGIDMap/Identity/DW 500/W[{widths}]>>")); // WHAT: the CID font. WHY: glyph numbers as codes
    let type0 = pdf.add(&format!("<</Type/Font/Subtype/Type0/BaseFont/ABCDEF+Ubuntu-Light/Encoding/Identity-H/DescendantFonts[{cid} 0 R]/ToUnicode {tounicode} 0 R>>")); // WHAT: the Type0 font. WHY: what the page uses
    // WHAT: the Type3 font, each letter a filled block. WHY: SHX and ClearScan fonts are Type3.
    let block = pdf.stream("", b"600 0 0 0 600 700 d1 60 0 480 700 re f"); // WHAT: a letter-shaped block. WHY: ink where a letter is
    let blank = pdf.stream("", b"600 0 0 0 0 0 d1"); // WHAT: an empty glyph. WHY: the space
    let widths3: String = (32..=88).map(|_| "600 ").collect(); // WHAT: every code's width. WHY: 600 thousandths each
    let type3 = pdf.add(&format!("<</Type/Font/Subtype/Type3/FontBBox[0 0 600 700]/FontMatrix[0.001 0 0 0.001 0 0]/CharProcs<</space {blank} 0 R/S {block} 0 R/H {block} 0 R/X {block} 0 R/T {block} 0 R/E {block} 0 R>>/Encoding<</Type/Encoding/Differences[32/space 69/E 72/H 83/S 84/T 88/X]>>/FirstChar 32/LastChar 88/Widths[{widths3}]/Resources<<>>>>")); // WHAT: the Type3 font. WHY: its glyphs are content streams
    let fonts = format!("/Font<</F1 {helvetica} 0 R/F2 {courier} 0 R/F3 {type0} 0 R/F4 {type3} 0 R>>"); // WHAT: the fonts by name. WHY: every sheet and form uses them
    let form1 = pdf.stream(&format!("/Type/XObject/Subtype/Form/BBox[0 0 300 40]/Resources<<{fonts}>>"), b"BT /F1 12 Tf 2 10 Td (INSIDE A FORM) Tj ET"); // WHAT: a form with text. WHY: a flattened stamp
    let form2 = pdf.stream(&format!("/Type/XObject/Subtype/Form/BBox[0 0 150 20]/Matrix[2 0 0 2 0 0]/Resources<<{fonts}>>"), b"BT /F1 6 Tf 1 5 Td (FORM MATRIX TWO) Tj ET"); // WHAT: a form scaled by its own matrix. WHY: both matrices apply
    let scan = pdf.stream("/Type/XObject/Subtype/Image/Width 1/Height 1/ColorSpace/DeviceGray/BitsPerComponent 8", &[0xCC]); // WHAT: a one-pixel grey picture. WHY: stands in for a scanned page, light enough not to count as ink
    let content: String = cases.iter().map(|c| format!("{}\n", c.content)).collect(); // WHAT: the cases' drawing. WHY: the sheet's content
    let stream = pdf.stream("", content.as_bytes()); // WHAT: the content stream. WHY: the page draws it
    let crop = crop.map(|c| format!("/CropBox[{} {} {} {}]", c[0], c[1], c[2], c[3])).unwrap_or_default(); // WHAT: a crop box, if asked. WHY: a cropped sheet's origin isn't 0,0
    let page = pdf.add(&format!("<</Type/Page/Parent {} 0 R/MediaBox[0 0 792 612]{crop}/Rotate {rotate}/Resources<<{fonts}/XObject<</Fm1 {form1} 0 R/Fm2 {form2} 0 R/Im0 {scan} 0 R>>>>/Contents {stream} 0 R>>", pdf.bodies.len() + 2)); // WHAT: the page. WHY: one sheet
    let pages = pdf.add(&format!("<</Type/Pages/Kids[{page} 0 R]/Count 1>>")); // WHAT: the page tree. WHY: the syntax
    let root = pdf.add(&format!("<</Type/Catalog/Pages {pages} 0 R>>")); // WHAT: the catalog. WHY: where reading starts
    pdf.finish(root) // WHAT: the file. WHY: written by the caller
}

/// A sheet drawn at two pixels a point. // WHAT: the drawing. WHY: the ink is the truth the boxes are checked against
fn draw(path: &Path, scratch: &Scratch) -> image::RgbaImage {
    let frame = frame_of(path); // WHAT: the sheet's frame. WHY: its size as shown
    let (w, h) = frame.size(); // WHAT: its width and height as shown. WHY: the picture's size
    let png = scratch.0.join(format!("{}.png", path.file_stem().and_then(|s| s.to_str()).unwrap_or("sheet"))); // WHAT: where the picture goes. WHY: picture_of writes a file
    hyperview::render::picture_of(library(), path, 0, (w * SCALE).round() as u32, (h * SCALE).round() as u32, &png).expect("the sheet draws"); // WHAT: draws it. WHY: through pdfium, as the window does
    image::open(&png).expect("the picture opens").to_rgba8() // WHAT: its pixels. WHY: looked at below
}

/// The frame of a fixture's one sheet. // WHAT: page box and turn. WHY: PDF space to the picture
fn frame_of(path: &Path) -> annot::Frame {
    let file = pdf::Document::from_bytes(std::fs::read(path).expect("the fixture")); // WHAT: the file, read. WHY: its page box and turn
    let page = file.page(0).expect("its sheet"); // WHAT: the sheet. WHY: one per fixture
    annot::Frame::of(&file, &page) // WHAT: its frame. WHY: what the window uses
}

/// Where the ink is, measured along a direction and up from it, in PDF space. // WHAT: the ink's extent in the text's own space. WHY: a turned note is checked along itself
fn ink_extent(picture: &image::RgbaImage, frame: &annot::Frame, along: [f64; 2], up: [f64; 2]) -> Option<[f64; 4]> {
    let mut out: Option<[f64; 4]> = None; // WHAT: least along, most along, least up, most up. WHY: grown as ink is found
    for (x, y, p) in picture.enumerate_pixels() { // WHAT: every pixel. WHY: the ink is wherever it is dark
        if p[0].max(p[1]).max(p[2]) >= 140 { // WHAT: not dark. WHY: paper and the grey of a scan aren't ink
            continue; // WHAT: skipped. WHY: the same
        }
        let at = frame.to_pdf([(f64::from(x) + 0.5) / SCALE, (f64::from(y) + 0.5) / SCALE]); // WHAT: the pixel's middle in PDF space. WHY: the boxes are in PDF space
        let (a, u) = (at[0] * along[0] + at[1] * along[1], at[0] * up[0] + at[1] * up[1]); // WHAT: along and up. WHY: the text's own space
        let e = out.get_or_insert([a, a, u, u]); // WHAT: the extent so far. WHY: grown
        e[0] = e[0].min(a); // WHAT: least along. WHY: the left of the ink
        e[1] = e[1].max(a); // WHAT: most along. WHY: the right
        e[2] = e[2].min(u); // WHAT: least up. WHY: the bottom
        e[3] = e[3].max(u); // WHAT: most up. WHY: the top
    }
    out.map(|e| [e[0] - 0.5 / SCALE, e[1] + 0.5 / SCALE, e[2] - 0.5 / SCALE, e[3] + 0.5 / SCALE]) // WHAT: out to the pixels' edges. WHY: a pixel's middle is half a pixel in from its edge
}

/// The box Edit Text draws, measured the same way. // WHAT: the box's extent in its own space. WHY: compared with the ink
fn box_extent(found: &hyperview::textedit::Found) -> [f64; 4] {
    let mut e = [f64::MAX, f64::MIN, f64::MAX, f64::MIN]; // WHAT: least and most, along and up. WHY: grown over its corners
    for c in found.corners() { // WHAT: each corner, as the tool draws it. WHY: the box itself, not a copy of its sums
        let (a, u) = (c[0] * found.along[0] + c[1] * found.along[1], c[0] * found.up[0] + c[1] * found.up[1]); // WHAT: along and up. WHY: its own space
        e = [e[0].min(a), e[1].max(a), e[2].min(u), e[3].max(u)]; // WHAT: grown. WHY: the extent
    }
    e // WHAT: the extent. WHY: compared
}

/// Words with their spacing ignored. // WHAT: letters only. WHY: whether a kern reads as a space is not what is checked here
fn letters(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect() // WHAT: no whitespace. WHY: the same
}

/// What is wrong with one case on one sheet, if anything. // WHAT: the checks. WHY: every way the box and the ink can disagree
fn check(case: &Case, found: &[hyperview::textedit::Found], picture: &image::RgbaImage, frame: &annot::Frame) -> Vec<String> {
    let mut wrong = Vec::new(); // WHAT: what failed. WHY: all of it reported, not just the first
    let Some(expect) = case.expect else { // WHAT: nothing should be offered. WHY: OCR and lines
        if !found.is_empty() { // WHAT: something was. WHY: a box round words nobody can see
            wrong.push(format!("{}: offered {:?} for editing, which nobody sees as text", case.name, found.iter().map(|f| f.text()).collect::<Vec<_>>())); // WHAT: said. WHY: the failure
        }
        return wrong; // WHAT: done. WHY: no box to measure
    };
    if found.len() != 1 { // WHAT: not one paragraph. WHY: missing text, or broken into pieces
        wrong.push(format!("{}: {} paragraphs where one was expected: {:?}", case.name, found.len(), found.iter().map(|f| f.text()).collect::<Vec<_>>())); // WHAT: said. WHY: the failure
        return wrong; // WHAT: done. WHY: nothing single to measure
    }
    let f = &found[0]; // WHAT: the one. WHY: measured
    if f.held != case.name.contains("form") { // WHAT: words in a form are held, others not. WHY: held words are shown but never changed in place
        wrong.push(format!("{}: held is {}, expected {}", case.name, f.held, case.name.contains("form"))); // WHAT: said. WHY: the failure
    }
    if letters(&f.text()) != letters(expect) { // WHAT: its words are wrong. WHY: a wrong reading means a wrong rewrite
        wrong.push(format!("{}: reads {:?}, expected {expect:?}", case.name, f.text())); // WHAT: said. WHY: the failure
    }
    let Some(ink) = ink_extent(picture, frame, f.along, f.up) else { // WHAT: where the ink is. WHY: the truth
        wrong.push(format!("{}: no ink on the sheet", case.name)); // WHAT: the fixture drew nothing. WHY: a broken fixture fails loudly
        return wrong; // WHAT: done. WHY: nothing to compare
    };
    let b = box_extent(f); // WHAT: the box. WHY: compared with the ink
    let s = f.size; // WHAT: the letters' height. WHY: slack for side bearings and ascenders scales with it
    let edges = [ // WHAT: each edge, how far off, and how far it may be. WHY: inside the ink by a pixel and a half at most, outside by a little more than the font's own white
        ("left", ink[0] - b[0], 0.12 * s), // WHAT: the box's left past the ink's left. WHY: a letter's side bearing
        ("right", b[1] - ink[1], 0.15 * s), // WHAT: the box's right past the ink's right. WHY: the same, and a closing bearing
        ("bottom", ink[2] - b[2], 0.45 * s), // WHAT: the box's bottom below the ink. WHY: room for descenders the words may not have
        ("top", b[3] - ink[3], 0.35 * s), // WHAT: the box's top above the ink. WHY: room for accents and ascenders over capitals
    ];
    for (edge, past, room) in edges { // WHAT: each edge. WHY: checked both ways
        if past < -SLACK { // WHAT: the box cuts into the ink. WHY: misplaced or too small
            wrong.push(format!("{}: box {edge} edge is {:.2} pt inside the ink (box {b:.1?}, ink {ink:.1?})", case.name, -past)); // WHAT: said. WHY: the failure
        } else if past > room + SLACK { // WHAT: the box stands well off the ink. WHY: misplaced or too big
            wrong.push(format!("{}: box {edge} edge is {past:.2} pt off the ink, more than {:.2} (box {b:.1?}, ink {ink:.1?})", case.name, room + SLACK)); // WHAT: said. WHY: the failure
        }
    }
    wrong // WHAT: what failed. WHY: the caller gathers it
}

/// Draws a straight line into a picture. // WHAT: a line, pixel by pixel. WHY: the boxes in the shots
fn line(picture: &mut image::RgbaImage, a: [f64; 2], b: [f64; 2], colour: image::Rgba<u8>) {
    let steps = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()).ceil() as usize).max(1); // WHAT: one step a pixel. WHY: no gaps
    for i in 0..=steps { // WHAT: each step. WHY: along the line
        let t = i as f64 / steps as f64; // WHAT: how far along. WHY: 0 to 1
        let (x, y) = (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t); // WHAT: the point. WHY: drawn
        if x >= 0.0 && y >= 0.0 && (x as u32) < picture.width() && (y as u32) < picture.height() { // WHAT: on the picture. WHY: a box off the sheet isn't drawn off the end
            picture.put_pixel(x as u32, y as u32, colour); // WHAT: the pixel. WHY: the line
        }
    }
}

/// The sheet with every box drawn over it, as the tool shows them. // WHAT: a shot. WHY: for a person to look at
fn shot(mut picture: image::RgbaImage, frame: &annot::Frame, found: &[hyperview::textedit::Found], to: &Path) {
    let blue = image::Rgba([40, 120, 255, 255]); // WHAT: the tool's blue. WHY: looks like the window
    for f in found { // WHAT: each paragraph. WHY: each gets its box
        let corners: Vec<[f64; 2]> = f.corners().iter().map(|c| { let s = frame.to_sheet(*c); [s[0] * SCALE, s[1] * SCALE] }).collect(); // WHAT: its corners in pixels. WHY: drawn
        for i in 0..corners.len() { // WHAT: each side. WHY: a closed box
            line(&mut picture, corners[i], corners[(i + 1) % corners.len()], blue); // WHAT: the side. WHY: the box
        }
    }
    picture.save(to).expect("the shot saves"); // WHAT: written. WHY: looked at
}

/// The turns and crops every case is tried on. // WHAT: the sheets. WHY: rotation and a page box off the origin are where boxes go wrong
const SHEETS: &[(&str, i32, Option<[f64; 4]>)] = &[
    ("rot0", 0, None), // WHAT: upright. WHY: the plain case
    ("rot90", 90, None), // WHAT: turned a quarter. WHY: D-size sheets are often stored this way
    ("rot180", 180, None), // WHAT: upside down. WHY: the half turn
    ("rot270", 270, None), // WHAT: turned the other quarter. WHY: the other way round
    ("crop-rot90", 90, Some([30.0, 20.0, 780.0, 600.0])), // WHAT: cropped and turned. WHY: the page box off 0,0 and a turn together
];

/// Every case, on every sheet: the box round its words holds its ink, edge to edge. // WHAT: the alignment test. WHY: "comes up all screwed up" is boxes not where the words are
#[test]
fn every_box_sits_on_its_words_at_every_turn() {
    let scratch = Scratch::new("align"); // WHAT: a folder. WHY: fixtures and pictures
    let bytes = epaint_default_fonts::UBUNTU_LIGHT; // WHAT: a real TrueType font. WHY: the CID case embeds it; it is already in the build
    let face = ttf_parser::Face::parse(bytes, 0).expect("the font parses"); // WHAT: the font read. WHY: glyph numbers and widths
    let all = cases(&face); // WHAT: every case. WHY: tried below
    let shots = std::env::var_os("EDIT_TEXT_SHOTS").map(PathBuf::from); // WHAT: where to put shots, if anywhere. WHY: only when a person asks
    let tag = std::env::var("EDIT_TEXT_SHOTS_TAG").unwrap_or_else(|_| "now".into()); // WHAT: a name for this set of shots. WHY: before and after side by side
    let mut wrong = Vec::new(); // WHAT: everything that failed. WHY: reported together
    for (sheet_name, rotate, crop) in SHEETS { // WHAT: each sheet. WHY: every turn
        for case in &all { // WHAT: each case alone. WHY: its ink is all that is on the sheet
            let path = scratch.0.join(format!("{}-{sheet_name}.pdf", case.name)); // WHAT: the fixture. WHY: one file each
            std::fs::write(&path, sheet(&[case], *rotate, *crop, bytes, &face)).expect("the fixture writes"); // WHAT: written. WHY: pdfium reads files
            let (_, found) = hyperview::render::text_on_sheet(library(), &path, 0).expect("the sheet reads"); // WHAT: what Edit Text finds. WHY: checked
            let picture = draw(&path, &scratch); // WHAT: the sheet drawn. WHY: the ink
            let frame = frame_of(&path); // WHAT: its frame. WHY: pixels to PDF space
            for w in check(case, &found, &picture, &frame) { // WHAT: what is wrong. WHY: gathered
                wrong.push(format!("[{sheet_name}] {w}")); // WHAT: with its sheet. WHY: says which turn
            }
        }
        if let Some(dir) = &shots { // WHAT: shots wanted. WHY: for a person
            std::fs::create_dir_all(dir).expect("the shots folder"); // WHAT: the folder. WHY: written into
            let refs: Vec<&Case> = all.iter().collect(); // WHAT: every case. WHY: one sheet with all of them
            let path = scratch.0.join(format!("all-{sheet_name}.pdf")); // WHAT: the whole sheet. WHY: as a drawing would be
            std::fs::write(&path, sheet(&refs, *rotate, *crop, bytes, &face)).expect("the fixture writes"); // WHAT: written. WHY: pdfium reads files
            let (_, found) = hyperview::render::text_on_sheet(library(), &path, 0).expect("the sheet reads"); // WHAT: what Edit Text finds. WHY: drawn over it
            shot(draw(&path, &scratch), &frame_of(&path), &found, &dir.join(format!("{tag}-{sheet_name}.png"))); // WHAT: the shot. WHY: looked at
            std::fs::copy(&path, dir.join(format!("fixture-{sheet_name}.pdf"))).expect("the fixture copies"); // WHAT: the fixture beside it. WHY: opened in the program by hand
        }
    }
    for w in &wrong { // WHAT: each failure. WHY: all of them in the log
        eprintln!("MISALIGNED {w}"); // WHAT: printed. WHY: grep-able
    }
    assert!(wrong.is_empty(), "{} boxes don't sit on their words", wrong.len()); // WHAT: none may fail. WHY: the bar
}

/// The editor's words sit on the sheet's: first letter, every baseline and the widest line's end, within a pixel. // WHAT: the overlay alignment test. WHY: the editor coming up off its words is the other half of "screwed up"
#[test]
fn the_editor_types_over_the_words_where_they_are() {
    let scratch = Scratch::new("editor"); // WHAT: a folder. WHY: fixtures
    let bytes = epaint_default_fonts::UBUNTU_LIGHT; // WHAT: the CID case's font. WHY: the same fixtures as above
    let face = ttf_parser::Face::parse(bytes, 0).expect("the font parses"); // WHAT: read. WHY: the same
    let all = cases(&face); // WHAT: every case. WHY: the same fixtures
    let ctx = egui::Context::default(); // WHAT: a window's worth of egui, with no window. WHY: its fonts lay out the editor's words exactly as the program does
    let _ = ctx.run(egui::RawInput::default(), |_| {}); // WHAT: one frame. WHY: egui loads its fonts on the first
    let zoom = 2.0_f32; // WHAT: two screen points a PDF point. WHY: a typical close-in zoom
    let mut wrong = Vec::new(); // WHAT: what failed. WHY: reported together
    let mut level = 0; // WHAT: how many paragraphs were edited over their words. WHY: a test that checked none would pass by doing nothing
    for (sheet_name, rotate, crop) in SHEETS { // WHAT: each turn. WHY: level on the screen is a different paragraph on each
        let refs: Vec<&Case> = all.iter().collect(); // WHAT: every case. WHY: one sheet
        let path = scratch.0.join(format!("all-{sheet_name}.pdf")); // WHAT: the fixture. WHY: read through pdfium
        std::fs::write(&path, sheet(&refs, *rotate, *crop, bytes, &face)).expect("the fixture writes"); // WHAT: written. WHY: the same
        let (_, found) = hyperview::render::text_on_sheet(library(), &path, 0).expect("the sheet reads"); // WHAT: its paragraphs. WHY: each placed
        let frame = frame_of(&path); // WHAT: its frame. WHY: PDF space to the screen
        let to_screen = |p: [f64; 2]| { let s = frame.to_sheet(p); egui::pos2(s[0] as f32 * zoom, s[1] as f32 * zoom) }; // WHAT: the screen at this zoom. WHY: as the window maps it, less its scroll
        for f in &found { // WHAT: each paragraph. WHY: each placed
            let place = hyperview::textedit::placement(f, &to_screen, &|j| ctx.fonts(|fonts| fonts.layout_job(j))); // WHAT: where the editor goes. WHY: checked
            let turned = { let a = to_screen(f.to_page(f.left, f.top)); let b = to_screen(f.to_page(f.left + 10.0, f.top)); (b - a).normalized().x < 0.999 }; // WHAT: not level on the screen. WHY: those go beside
            if turned { // WHAT: turned on the screen. WHY: must go beside, not crooked over it
                if place.over { // WHAT: it didn't. WHY: a failure
                    wrong.push(format!("[{sheet_name}] {:?}: turned on the screen but edited over the words", f.text())); // WHAT: said. WHY: the failure
                }
                continue; // WHAT: next. WHY: nothing to line up
            }
            if !place.over { // WHAT: level but put beside. WHY: a failure
                wrong.push(format!("[{sheet_name}] {:?}: level on the screen but edited beside it", f.text())); // WHAT: said. WHY: the failure
                continue; // WHAT: next. WHY: nothing to line up
            }
            level += 1; // WHAT: one more checked. WHY: counted
            let galley = ctx.fonts(|fonts| fonts.layout_job(hyperview::textedit::job(&hyperview::textedit::Rewrite::of(f).text, &place, egui::Color32::BLACK, place.width))); // WHAT: the words the editor opens with, laid out as it lays them out. WHY: measured // WHAT: the editor's words, laid out as the editor lays them out. WHY: measured
            if galley.rows.len() != f.lines.len() { // WHAT: a different number of lines. WHY: the editor would wrap where the sheet didn't
                wrong.push(format!("[{sheet_name}] {:?}: {} lines in the editor, {} on the sheet", f.text(), galley.rows.len(), f.lines.len())); // WHAT: said. WHY: the failure
                continue; // WHAT: next. WHY: rows can't be matched
            }
            for (i, row) in galley.rows.iter().enumerate() { // WHAT: each line. WHY: each baseline checked
                let said: String = row.row.glyphs.iter().map(|g| g.chr).collect(); // WHAT: the editor's line. WHY: compared with the sheet's
                if letters(&said) != letters(&f.lines[i]) { // WHAT: it breaks somewhere else. WHY: the editor's lines must be the sheet's
                    wrong.push(format!("[{sheet_name}] {:?} line {i}: the editor has {said:?}, the sheet {:?}", f.text(), f.lines[i])); // WHAT: said. WHY: the failure
                }
                let Some(first) = row.row.glyphs.first() else { continue }; // WHAT: its first letter. WHY: where it starts
                let baseline = place.at.y + row.pos.y + first.pos.y; // WHAT: its baseline on the screen. WHY: compared
                let start = place.at.x + row.pos.x + first.pos.x; // WHAT: where it starts on the screen. WHY: compared
                let want = to_screen(f.to_page(f.ends[i][0], f.top - f.spacing * i as f64)); // WHAT: where the sheet's line starts. WHY: the truth
                if (baseline - want.y).abs() > 1.0 { // WHAT: baselines a pixel or more apart. WHY: the bar
                    wrong.push(format!("[{sheet_name}] {:?} line {i}: baseline {baseline:.1}, sheet {:.1}", f.text(), want.y)); // WHAT: said. WHY: the failure
                }
                if i == 0 && (start - want.x).abs() > 1.0 { // WHAT: the first line starts off its words. WHY: the bar
                    wrong.push(format!("[{sheet_name}] {:?}: starts at {start:.1}, sheet {:.1}", f.text(), want.x)); // WHAT: said. WHY: the failure
                }
            }
            let k = (0..f.ends.len()).max_by(|a, b| (f.ends[*a][1] - f.ends[*a][0]).total_cmp(&(f.ends[*b][1] - f.ends[*b][0]))).unwrap_or(0); // WHAT: the widest line. WHY: fitted to the sheet's
            let row = &galley.rows[k]; // WHAT: its row in the editor. WHY: its end measured
            if let (Some(first), Some(last)) = (row.row.glyphs.first(), row.row.glyphs.iter().rev().find(|g| !g.chr.is_whitespace())) { // WHAT: its first and last letters. WHY: where it starts and ends
                let start = place.at.x + row.pos.x + first.pos.x; // WHAT: the line's start in the editor. WHY: its run is measured from here
                let end = start + last.pos.x + last.advance_width - first.pos.x; // WHAT: the end of the line in the editor. WHY: compared
                let want = start + (to_screen(f.to_page(f.ends[k][1], f.top)).x - to_screen(f.to_page(f.ends[k][0], f.top)).x); // WHAT: where it should end: as far along as the sheet's line runs. WHY: the run is the truth; the start is checked above
                if (end - want).abs() > 2.0 { // WHAT: more than two pixels at the line's far end. WHY: egui sets letters on its own grid, so a long line's end can only land within a step of it; starts and baselines are held to one pixel above
                    wrong.push(format!("[{sheet_name}] {:?}: widest line ends at {end:.1}, sheet {want:.1}", f.text())); // WHAT: said. WHY: the failure
                }
            }
        }
    }
    for w in &wrong { // WHAT: each failure. WHY: in the log
        eprintln!("EDITOR {w}"); // WHAT: printed. WHY: grep-able
    }
    assert!(level >= 13, "only {level} paragraphs were checked over their words"); // WHAT: every level paragraph: eleven upright, one on each quarter-turned sheet // WHAT: enough were checked. WHY: a test that checks nothing proves nothing
    assert!(wrong.is_empty(), "{} editor placements are off their words", wrong.len()); // WHAT: none may fail. WHY: the bar
}

/// Words inside a form are found and shown, but changing them is refused. // WHAT: the held guard. WHY: a form may be on every sheet of a set
#[test]
fn words_inside_a_form_are_never_rewritten() {
    let scratch = Scratch::new("held"); // WHAT: a folder. WHY: fixtures
    let bytes = epaint_default_fonts::UBUNTU_LIGHT; // WHAT: the CID case's font. WHY: the same fixtures
    let face = ttf_parser::Face::parse(bytes, 0).expect("the font parses"); // WHAT: read. WHY: the same
    let all = cases(&face); // WHAT: every case. WHY: one is used
    let form = all.iter().find(|c| c.name == "in-a-form").expect("the form case"); // WHAT: the form case. WHY: tried
    let path = scratch.0.join("held.pdf"); // WHAT: the fixture. WHY: one file
    std::fs::write(&path, sheet(&[form], 0, None, bytes, &face)).expect("the fixture writes"); // WHAT: written. WHY: pdfium reads files
    let (_, found) = hyperview::render::text_on_sheet(library(), &path, 0).expect("the sheet reads"); // WHAT: its paragraph. WHY: rewritten below
    let f = found.first().expect("the form's words are found"); // WHAT: the one. WHY: there is one
    let mut rewrite = hyperview::textedit::Rewrite::of(f); // WHAT: a change. WHY: tried
    rewrite.text = "CHANGED".into(); // WHAT: new words. WHY: something to write
    let said = hyperview::render::rewrite_on_sheet(library(), &path, 0, f, &rewrite); // WHAT: tried. WHY: must be refused
    assert_eq!(said.err().as_deref(), Some(hyperview::render::HELD), "words inside a form were rewritten"); // WHAT: refused, saying why. WHY: the guard
}

/// A level paragraph on a turned sheet is rewritten where it was: the new words' box sits on the old one. // WHAT: the round trip. WHY: placement on the screen is no use if the written words land elsewhere
#[test]
fn rewritten_words_land_where_the_old_ones_were() {
    let scratch = Scratch::new("rewrite"); // WHAT: a folder. WHY: fixtures
    let bytes = epaint_default_fonts::UBUNTU_LIGHT; // WHAT: the CID case's font. WHY: the same fixtures
    let face = ttf_parser::Face::parse(bytes, 0).expect("the font parses"); // WHAT: read. WHY: the same
    let all = cases(&face); // WHAT: every case. WHY: some are used
    for (sheet_name, rotate, crop) in SHEETS { // WHAT: every turn. WHY: the written matrix must follow the text, not the sheet
        for name in ["title", "turned-90", "cm-scaled"] { // WHAT: upright, turned, and scaled by cm. WHY: the three ways a matrix can differ
            let case = all.iter().find(|c| c.name == name).expect("the case"); // WHAT: the case. WHY: tried
            let path = scratch.0.join(format!("{name}-{sheet_name}.pdf")); // WHAT: the fixture. WHY: one each
            std::fs::write(&path, sheet(&[case], *rotate, *crop, bytes, &face)).expect("the fixture writes"); // WHAT: written. WHY: pdfium reads files
            let (_, found) = hyperview::render::text_on_sheet(library(), &path, 0).expect("the sheet reads"); // WHAT: its paragraph. WHY: rewritten
            let before = found.first().expect("its words").clone(); // WHAT: the one. WHY: compared after
            let mut rewrite = hyperview::textedit::Rewrite::of(&before); // WHAT: the same words. WHY: in a standard font, so the box should barely move
            rewrite.text = rewrite.text.replace('E', "F"); // WHAT: a letter changed. WHY: something to write, the same width in Helvetica
            let (bytes_out, _) = hyperview::render::rewrite_on_sheet(library(), &path, 0, &before, &rewrite).expect("rewritten"); // WHAT: written. WHY: read back
            let out = scratch.0.join(format!("{name}-{sheet_name}-out.pdf")); // WHAT: where. WHY: read back by pdfium
            std::fs::write(&out, bytes_out).expect("written"); // WHAT: saved. WHY: the same
            let (_, again) = hyperview::render::text_on_sheet(library(), &out, 0).expect("reads again"); // WHAT: its paragraph now. WHY: compared
            let after = again.first().expect("the new words are found"); // WHAT: the one. WHY: there is one
            let (a, b) = (before.to_page(before.left, before.top), after.to_page(after.left, after.top)); // WHAT: where each starts. WHY: compared
            assert!((a[0] - b[0]).abs() < SLACK && (a[1] - b[1]).abs() < SLACK, "[{sheet_name}] {name}: started at {a:?}, now {b:?}"); // WHAT: the same start. WHY: within a pixel and a half
            assert!((before.size - after.size).abs() < 0.05, "[{sheet_name}] {name}: size {} now {}", before.size, after.size); // WHAT: the same size. WHY: the same
            assert!((before.along[0] - after.along[0]).abs() < 1e-6 && (before.along[1] - after.along[1]).abs() < 1e-6, "[{sheet_name}] {name}: turned"); // WHAT: the same direction. WHY: the same
        }
    }
}

/// With `EDIT_TEXT_SAMPLE` naming a drawing and `EDIT_TEXT_SHOTS` a folder, its first sheet is drawn with every outline (as Edit Text used to show it, and as Text Outlines on shows it) and with one (as it shows now by default), and how many pieces and paragraphs it has is printed. // WHAT: a look at a real drawing set. WHY: the repo's own sample set, never a customer's file
#[test]
fn a_sample_drawing_with_and_without_outlines() {
    let (Some(sample), Some(dir)) = (std::env::var_os("EDIT_TEXT_SAMPLE").map(PathBuf::from), std::env::var_os("EDIT_TEXT_SHOTS").map(PathBuf::from)) else { return }; // WHAT: only when asked. WHY: a look for a person, not a check
    let scratch = Scratch::new("sample"); // WHAT: a folder. WHY: the picture
    std::fs::create_dir_all(&dir).expect("the shots folder"); // WHAT: the folder. WHY: written into
    let (runs, found) = hyperview::render::text_on_sheet(library(), &sample, 0).expect("the sample reads"); // WHAT: what Edit Text finds. WHY: drawn and counted
    eprintln!("SAMPLE {} pieces, {} paragraphs, {} held in forms", runs.len(), found.len(), found.iter().filter(|f| f.held).count()); // WHAT: the counts. WHY: in the report
    let frame = frame_of(&sample); // WHAT: its frame. WHY: boxes to pixels
    let picture = draw(&sample, &scratch); // WHAT: the sheet drawn. WHY: the boxes go over it
    shot(picture.clone(), &frame, &found, &dir.join("sample-every-outline.png")); // WHAT: every box. WHY: what the tool showed before, and what Text Outlines on shows
    let middle = found.iter().max_by_key(|f| f.text().len()).cloned().into_iter().collect::<Vec<_>>(); // WHAT: one paragraph, the longest. WHY: stands for the one under the pointer
    shot(picture, &frame, &middle, &dir.join("sample-default-hover-only.png")); // WHAT: one box. WHY: what the tool shows now by default
}

/// A schedule drawn the way CAD draws one, cells boxed with `re`, is read cell by cell through pdfium, at every turn. // WHAT: tables through the real reader. WHY: the sample set's sheet index came up one box per column
#[test]
fn a_ruled_schedule_is_read_cell_by_cell() {
    let scratch = Scratch::new("table"); // WHAT: a folder. WHY: fixtures
    let bytes = epaint_default_fonts::UBUNTU_LIGHT; // WHAT: the shared fixtures' font. WHY: sheet() embeds it
    let face = ttf_parser::Face::parse(bytes, 0).expect("the font parses"); // WHAT: read. WHY: the same
    let mut content = String::from("q 0.5 w 0 G\n"); // WHAT: thin black rules. WHY: as plotted
    let rows = [("SHEET", "TITLE"), ("S-001", "GENERAL NOTES"), ("S-101", "FOUNDATION PLAN"), ("S-201", "FRAMING PLAN")]; // WHAT: a header and three rows. WHY: a sheet index
    for (i, (a, b)) in rows.iter().enumerate() { // WHAT: each row. WHY: boxed and lettered
        let top = 500.0 - 12.0 * i as f64; // WHAT: the row's top. WHY: 12 points a row
        content.push_str(&format!("100 {} 50 12 re S 150 {} 150 12 re S\n", top - 12.0, top - 12.0)); // WHAT: the row's two cells boxed. WHY: how CAD draws a table
        let font = if i == 0 { "/F2" } else { "/F1" }; // WHAT: the header in another font. WHY: headers differ
        content.push_str(&format!("BT {font} 7 Tf 1 0 0 1 103 {} Tm ({a}) Tj ET BT {font} 7 Tf 1 0 0 1 153 {} Tm ({b}) Tj ET\n", top - 9.0, top - 9.0)); // WHAT: the two cells' words. WHY: on one baseline
    }
    content.push('Q'); // WHAT: done. WHY: the state put back
    let case = Case { name: "schedule", content, expect: None }; // WHAT: the schedule. WHY: drawn by sheet()
    for (sheet_name, rotate, crop) in SHEETS { // WHAT: every turn. WHY: rules are compared in the text's own space
        let path = scratch.0.join(format!("table-{sheet_name}.pdf")); // WHAT: the fixture. WHY: one each
        std::fs::write(&path, sheet(&[&case], *rotate, *crop, bytes, &face)).expect("the fixture writes"); // WHAT: written. WHY: pdfium reads files
        let (_, found) = hyperview::render::text_on_sheet(library(), &path, 0).expect("the sheet reads"); // WHAT: its paragraphs. WHY: counted
        let texts: Vec<String> = found.iter().map(|f| f.text()).collect(); // WHAT: what each says. WHY: shown on failure
        assert_eq!(found.len(), 8, "[{sheet_name}] {texts:?}"); // WHAT: one a cell. WHY: a cell is what somebody changes
        assert!(texts.iter().any(|t| t == "FOUNDATION PLAN"), "[{sheet_name}] {texts:?}"); // WHAT: a cell alone. WHY: the same
    }
}

// ---------------------------------------------------------------------------
// Whole fonts for letters a cut-down font lacks.
// ---------------------------------------------------------------------------

/// A name table naming a font: family, style, full name and PostScript name, in Windows Unicode. // WHAT: a font's names, written fresh. WHY: the fixtures' fonts pose as Arial and Liberation Sans
fn name_table(family: &str, postscript: &str) -> Vec<u8> {
    let names: [(u16, &str); 4] = [(1, family), (2, "Regular"), (4, family), (6, postscript)]; // WHAT: family, style, full name, PostScript name. WHY: what the font shelf reads
    let strings: Vec<Vec<u8>> = names.iter().map(|(_, s)| s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()).collect(); // WHAT: each in UTF-16BE. WHY: Windows Unicode names are
    let mut out = Vec::new(); // WHAT: the table. WHY: filled below
    out.extend_from_slice(&0u16.to_be_bytes()); // WHAT: format 0. WHY: the plain one
    out.extend_from_slice(&(names.len() as u16).to_be_bytes()); // WHAT: how many names. WHY: the header
    out.extend_from_slice(&(6 + 12 * names.len() as u16).to_be_bytes()); // WHAT: where the strings start. WHY: after the records
    let mut offset = 0u16; // WHAT: where each string starts. WHY: its record says
    for ((id, _), bytes) in names.iter().zip(&strings) { // WHAT: each name. WHY: one record each
        for v in [3u16, 1, 0x0409, *id, bytes.len() as u16, offset] { // WHAT: Windows, Unicode, US English, its number, length, offset. WHY: the record
            out.extend_from_slice(&v.to_be_bytes()); // WHAT: written. WHY: the same
        }
        offset += bytes.len() as u16; // WHAT: the next one's start. WHY: they follow each other
    }
    for bytes in &strings { // WHAT: the strings. WHY: after the records
        out.extend_from_slice(bytes); // WHAT: written. WHY: the same
    }
    out // WHAT: the table. WHY: put in the font
}

/// A TrueType font rebuilt from another: its own names and licence flags, and, when asked, outlines for only some letters, the way a PDF writer cuts a font down. // WHAT: a fixture font. WHY: real subset fonts and fonts of known names and licences, with no font from anybody's computer
fn rebuilt_font(base: &[u8], family: &str, postscript: &str, fs_type: u16, keep: Option<&str>) -> Vec<u8> {
    let be16 = |b: &[u8], at: usize| u16::from_be_bytes([b[at], b[at + 1]]); // WHAT: a 16-bit number. WHY: TrueType is big-endian
    let be32 = |b: &[u8], at: usize| u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]); // WHAT: a 32-bit number. WHY: the same
    let count = be16(base, 4) as usize; // WHAT: how many tables. WHY: the directory
    let mut tables: Vec<([u8; 4], Vec<u8>)> = (0..count).map(|i| { // WHAT: every table. WHY: changed and written back
        let e = 12 + 16 * i; // WHAT: its directory entry. WHY: where it is
        let (offset, length) = (be32(base, e + 8) as usize, be32(base, e + 12) as usize); // WHAT: where and how long. WHY: copied
        ([base[e], base[e + 1], base[e + 2], base[e + 3]], base[offset..offset + length].to_vec()) // WHAT: its tag and bytes. WHY: kept
    }).collect();
    let find = |tables: &[([u8; 4], Vec<u8>)], tag: &[u8; 4]| tables.iter().position(|(t, _)| t == tag).expect("the table"); // WHAT: a table by tag. WHY: changed below
    let n = find(&tables, b"name"); // WHAT: the names. WHY: replaced
    tables[n].1 = name_table(family, postscript); // WHAT: the fixture's names. WHY: it poses as that font
    let o = find(&tables, b"OS/2"); // WHAT: the OS/2 table. WHY: its licence flags
    tables[o].1[8..10].copy_from_slice(&fs_type.to_be_bytes()); // WHAT: fsType set. WHY: installable, editable or restricted as the test needs
    if let Some(keep) = keep { // WHAT: a cut-down font. WHY: as PDF writers embed one
        let face = ttf_parser::Face::parse(base, 0).expect("the font parses"); // WHAT: read. WHY: its glyph numbers
        let mut kept: Vec<u16> = vec![0]; // WHAT: the glyphs kept, .notdef always. WHY: a font needs it
        kept.extend(keep.chars().filter_map(|c| face.glyph_index(c)).map(|g| g.0)); // WHAT: each kept letter's glyph. WHY: only those keep outlines
        let h = find(&tables, b"head"); // WHAT: the head. WHY: says how loca is written
        let long = be16(&tables[h].1, 50) == 1; // WHAT: long or short loca. WHY: read the old one
        let glyphs = be16(&tables[find(&tables, b"maxp")].1, 4) as usize; // WHAT: how many glyphs. WHY: every one keeps its number
        let loca = tables[find(&tables, b"loca")].1.clone(); // WHAT: the old loca. WHY: where each glyph's outline was
        let at = |g: usize| if long { be32(&loca, g * 4) as usize } else { be16(&loca, g * 2) as usize * 2 }; // WHAT: a glyph's start. WHY: in either form
        let glyf = tables[find(&tables, b"glyf")].1.clone(); // WHAT: the old outlines. WHY: the kept ones copied
        let (mut new_glyf, mut new_loca) = (Vec::new(), Vec::new()); // WHAT: the cut-down tables. WHY: built below
        for g in 0..glyphs { // WHAT: every glyph number. WHY: numbers stay, outlines go
            new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes()); // WHAT: where it starts now. WHY: long loca
            if kept.contains(&(g as u16)) { // WHAT: a kept one. WHY: its outline copied
                let outline = &glyf[at(g)..at(g + 1)]; // WHAT: its outline. WHY: copied
                assert!(outline.len() < 2 || be16(outline, 0) as i16 >= 0, "glyph {g} is built from others; pick simple letters"); // WHAT: a simple glyph. WHY: a composite one would need its parts kept
                new_glyf.extend_from_slice(outline); // WHAT: copied. WHY: kept
                while new_glyf.len() % 4 != 0 { // WHAT: to four bytes. WHY: tidy alignment
                    new_glyf.push(0); // WHAT: padding. WHY: the same
                }
            }
        }
        new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes()); // WHAT: the end. WHY: loca has one more entry than glyphs
        let gi = find(&tables, b"glyf"); // WHAT: the outlines. WHY: replaced
        tables[gi].1 = new_glyf; // WHAT: only the kept ones. WHY: cut down
        let li = find(&tables, b"loca"); // WHAT: the index. WHY: replaced
        tables[li].1 = new_loca; // WHAT: long form. WHY: simplest
        tables[h].1[50..52].copy_from_slice(&1u16.to_be_bytes()); // WHAT: loca is long now. WHY: the head says so
    }
    let mut out = base[..12].to_vec(); // WHAT: the sfnt header. WHY: the same tables count
    let mut offset = 12 + 16 * tables.len(); // WHAT: where the first table goes. WHY: after the directory
    let mut body = Vec::new(); // WHAT: the tables' bytes. WHY: after the directory
    for (tag, bytes) in &tables { // WHAT: each table. WHY: an entry each
        out.extend_from_slice(tag); // WHAT: its tag. WHY: the entry
        out.extend_from_slice(&0u32.to_be_bytes()); // WHAT: no checksum. WHY: nothing reading fixtures checks it
        out.extend_from_slice(&(offset as u32).to_be_bytes()); // WHAT: where it is. WHY: the entry
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes()); // WHAT: how long. WHY: the entry
        body.extend_from_slice(bytes); // WHAT: its bytes. WHY: written after
        while body.len() % 4 != 0 { // WHAT: to four bytes. WHY: tables start on four
            body.push(0); // WHAT: padding. WHY: the same
        }
        offset = 12 + 16 * tables.len() + body.len(); // WHAT: the next one's start. WHY: the same
    }
    out.extend_from_slice(&body); // WHAT: the tables. WHY: the font
    out // WHAT: the font file. WHY: embedded or put in a font folder
}

/// The letters the fixture drawing's cut-down font has. // WHAT: what the subset can set. WHY: anything else typed needs a whole font
const SUBSET: &str = "BEAM 12"; // WHAT: two notes' letters. WHY: short and simple glyphs

/// A sheet with two notes, "BEAM 1" and "BEAM 2", in a cut-down TrueType font named ABCDEF+ArialMT, the way AutoCAD and Revit plot Arial. // WHAT: the subset fixture. WHY: the drawings people edit
fn subset_sheet(rotate: i32, crop: Option<[f64; 4]>) -> (Vec<u8>, usize) {
    let base = epaint_default_fonts::UBUNTU_LIGHT; // WHAT: a real TrueType font. WHY: posing as Arial, cut down
    let face = ttf_parser::Face::parse(base, 0).expect("the font parses"); // WHAT: read. WHY: widths
    let program = rebuilt_font(base, "Arial", "ArialMT", 0x0008, Some(SUBSET)); // WHAT: Arial cut down to two notes' letters. WHY: the subset
    let upem = f64::from(face.units_per_em()); // WHAT: units to the em. WHY: widths in thousandths
    let widths: String = (32u8..=126).map(|c| { // WHAT: every code's width. WHY: the Widths array
        let ch = c as char; // WHAT: the letter. WHY: looked up
        if SUBSET.contains(ch) { format!("{} ", (f64::from(face.glyph_index(ch).and_then(|g| face.glyph_hor_advance(g)).unwrap_or(500)) * 1000.0 / upem).round()) } else { "0 ".into() } // WHAT: its width if kept, else none. WHY: as a subset writes it
    }).collect();
    let mut pdf = Pdf::new(); // WHAT: a new file. WHY: the fixture
    let file = pdf.stream(&format!("/Length1 {}", program.len()), &program); // WHAT: the cut-down font program. WHY: embedded
    let descriptor = pdf.add(&format!("<</Type/FontDescriptor/FontName/ABCDEF+ArialMT/Flags 32/FontBBox[-200 -250 1200 1000]/ItalicAngle 0/Ascent 932/Descent -189/CapHeight 693/StemV 60/FontFile2 {file} 0 R>>")); // WHAT: its descriptor. WHY: points at the program
    let font = pdf.add(&format!("<</Type/Font/Subtype/TrueType/BaseFont/ABCDEF+ArialMT/FirstChar 32/LastChar 126/Widths[{widths}]/Encoding/WinAnsiEncoding/FontDescriptor {descriptor} 0 R>>")); // WHAT: the font. WHY: a simple TrueType subset, as plotted
    let content = "BT /F1 12 Tf 1 0 0 1 100 400 Tm (BEAM 1) Tj ET\nBT /F1 12 Tf 1 0 0 1 100 200 Tm (BEAM 2) Tj ET\n"; // WHAT: two notes far apart. WHY: two paragraphs in the one font
    let stream = pdf.stream("", content.as_bytes()); // WHAT: the content. WHY: the page draws it
    let crop = crop.map(|c| format!("/CropBox[{} {} {} {}]", c[0], c[1], c[2], c[3])).unwrap_or_default(); // WHAT: a crop box, if asked. WHY: the same as the other fixtures
    let page = pdf.add(&format!("<</Type/Page/Parent {} 0 R/MediaBox[0 0 792 612]{crop}/Rotate {rotate}/Resources<</Font<</F1 {font} 0 R>>>>/Contents {stream} 0 R>>", pdf.bodies.len() + 2)); // WHAT: the page. WHY: one sheet
    let pages = pdf.add(&format!("<</Type/Pages/Kids[{page} 0 R]/Count 1>>")); // WHAT: the page tree. WHY: the syntax
    let root = pdf.add(&format!("<</Type/Catalog/Pages {pages} 0 R>>")); // WHAT: the catalog. WHY: where reading starts
    (pdf.finish(root), program.len()) // WHAT: the file, and how big its font is. WHY: growth is compared with it
}

/// A font folder holding these fonts, rebuilt from the shared one: (file name, family, PostScript name, fsType). // WHAT: a test's "computer's fonts". WHY: never what the build machine has installed
fn font_folder(scratch: &Scratch, name: &str, fonts: &[(&str, &str, &str, u16)]) -> PathBuf {
    let folder = scratch.0.join(name); // WHAT: the folder. WHY: one per case
    std::fs::create_dir_all(folder.join("truetype")).expect("the font folder"); // WHAT: with a sub-folder. WHY: Linux keeps fonts a folder down, and the walk must find them
    for (file, family, postscript, fs_type) in fonts { // WHAT: each font. WHY: written
        let bytes = rebuilt_font(epaint_default_fonts::UBUNTU_LIGHT, family, postscript, *fs_type, None); // WHAT: a whole font of that name and licence. WHY: posing as it
        std::fs::write(folder.join("truetype").join(file), bytes).expect("the font writes"); // WHAT: written. WHY: found by the walk
    }
    folder // WHAT: the folder. WHY: given to the rewrite
}

/// The whole of a font rebuilt for the tests, in bytes. // WHAT: how big a whole font is. WHY: a file that grew less than this didn't take a copy of it
fn whole_font_size() -> usize {
    rebuilt_font(epaint_default_fonts::UBUNTU_LIGHT, "Arial", "ArialMT", 0, None).len() // WHAT: its size. WHY: the same
}

/// One note on the subset sheet rewritten with the given fonts: the new file, what was said, the note as read before and after. // WHAT: one edit. WHY: every font test does it
fn edit_note(scratch: &Scratch, path: &Path, which: &str, text: &str, align: Option<hyperview::textedit::Align>, fonts: Vec<PathBuf>) -> Result<(PathBuf, String, hyperview::textedit::Found, hyperview::textedit::Found), String> {
    let (_, found) = hyperview::render::text_on_sheet(library(), path, 0).expect("the sheet reads"); // WHAT: its paragraphs. WHY: one is rewritten
    let before = found.iter().find(|f| f.text() == which).unwrap_or_else(|| panic!("{which:?} among {:?}", found.iter().map(|f| f.text()).collect::<Vec<_>>())).clone(); // WHAT: the note. WHY: rewritten
    let mut rewrite = hyperview::textedit::Rewrite::of(&before); // WHAT: as it is. WHY: changed below
    rewrite.text = text.into(); // WHAT: the new words. WHY: typed
    if let Some(a) = align { // WHAT: an alignment asked for. WHY: the right-aligned case
        rewrite.align = a; // WHAT: set. WHY: the line is placed by its width
    }
    let (bytes, said) = hyperview::render::rewrite_on_sheet_with_fonts(library(), path, 0, &before, &rewrite, fonts)?; // WHAT: written. WHY: the code under test
    let out = scratch.0.join(format!("{}-{}.pdf", path.file_stem().and_then(|s| s.to_str()).unwrap_or("out"), letters(text))); // WHAT: where the result goes. WHY: read back
    std::fs::write(&out, bytes).expect("written"); // WHAT: saved. WHY: read back by pdfium
    let (_, again) = hyperview::render::text_on_sheet(library(), &out, 0).expect("reads again"); // WHAT: its paragraphs now. WHY: the new note found
    let after = again.iter().find(|f| f.text() == text).unwrap_or_else(|| panic!("{text:?} among {:?}", again.iter().map(|f| f.text()).collect::<Vec<_>>())).clone(); // WHAT: the new note. WHY: checked
    Ok((out, said, before, after)) // WHAT: all of it. WHY: the test checks what it needs
}

/// A typed letter the drawing's cut-down font lacks is set in the same family's whole font, on every turn, where the old words were, and its ink sits in its box. // WHAT: the full-font fix. WHY: the edited note used to come out in Helvetica
#[test]
fn a_letter_the_subset_lacks_is_typed_in_the_same_family() {
    let scratch = Scratch::new("font-family"); // WHAT: a folder. WHY: fixtures
    let fonts = font_folder(&scratch, "fonts", &[("arial.ttf", "Arial", "ArialMT", 0x0008), ("LiberationSans-Regular.ttf", "Liberation Sans", "LiberationSans", 0)]); // WHAT: Arial and its metric twin installed. WHY: Arial must win
    for (sheet_name, rotate, crop) in SHEETS { // WHAT: every turn. WHY: the new font's matrix follows the text
        let path = scratch.0.join(format!("subset-{sheet_name}.pdf")); // WHAT: the fixture. WHY: one each
        std::fs::write(&path, subset_sheet(*rotate, *crop).0).expect("the fixture writes"); // WHAT: written. WHY: pdfium reads files
        let (out, said, before, after) = edit_note(&scratch, &path, "BEAM 1", "BEAM 1X", None, vec![fonts.clone()]).expect("rewritten"); // WHAT: an X typed, which the subset lacks. WHY: the case
        assert!(said.contains("Typed in Arial because the drawing's font didn't include those letters."), "[{sheet_name}] {said}"); // WHAT: the notice. WHY: the owner's wording
        assert!(!after.font.to_lowercase().contains("helvetica"), "[{sheet_name}] set in {}", after.font); // WHAT: not Helvetica. WHY: the bug
        assert!(after.font.to_lowercase().contains("arial"), "[{sheet_name}] set in {}", after.font); // WHAT: Arial's whole font. WHY: the same family
        let (a, b) = (before.to_page(before.left, before.top), after.to_page(after.left, after.top)); // WHAT: where each starts. WHY: compared
        assert!((a[0] - b[0]).abs() < SLACK && (a[1] - b[1]).abs() < SLACK, "[{sheet_name}] started at {a:?}, now {b:?}"); // WHAT: the same start. WHY: within a pixel and a half
        assert!((before.size - after.size).abs() < 0.05, "[{sheet_name}] size {} now {}", before.size, after.size); // WHAT: the same size. WHY: the same
        let case = Case { name: "subset-typed", content: String::new(), expect: Some("BEAM 1X") }; // WHAT: the new note as a case. WHY: the alignment checks measure it
        let (_, all) = hyperview::render::text_on_sheet(library(), &out, 0).expect("reads again"); // WHAT: every paragraph. WHY: only the new one is measured
        let picture = draw(&out, &scratch); // WHAT: the new sheet drawn. WHY: its ink
        let mut alone = all.clone(); // WHAT: the new note alone. WHY: the other note's ink is masked below
        alone.retain(|f| f.text() == "BEAM 1X"); // WHAT: the one. WHY: measured
        let frame = frame_of(&out); // WHAT: its frame. WHY: boxes to pixels
        let other = all.iter().find(|f| f.text() == "BEAM 2").expect("the other note is untouched"); // WHAT: the other note. WHY: left as it was, and kept out of the measure
        let masked = mask(picture, &frame, other); // WHAT: the other note's ink painted out. WHY: ink_extent measures all ink
        let wrong = check(&case, &alone, &masked, &frame); // WHAT: ink against box. WHY: the existing tolerances
        assert!(wrong.is_empty(), "[{sheet_name}] {wrong:?}"); // WHAT: none off. WHY: the bar
    }
}

/// Paints a paragraph's box white, so another paragraph's ink can be measured alone. // WHAT: a mask. WHY: the subset sheet has two notes
fn mask(mut picture: image::RgbaImage, frame: &annot::Frame, f: &hyperview::textedit::Found) -> image::RgbaImage {
    let e = box_extent(f); // WHAT: its box along and up. WHY: everything in it, and a little round it, is painted out
    let (w, h) = (picture.width(), picture.height()); // WHAT: the picture's size. WHY: every pixel looked at
    for y in 0..h { // WHAT: each row. WHY: every pixel
        for x in 0..w { // WHAT: each pixel. WHY: the same
            let at = frame.to_pdf([(f64::from(x) + 0.5) / SCALE, (f64::from(y) + 0.5) / SCALE]); // WHAT: in PDF space. WHY: the box is
            let (a, u) = (at[0] * f.along[0] + at[1] * f.along[1], at[0] * f.up[0] + at[1] * f.up[1]); // WHAT: along and up. WHY: its own space
            if a > e[0] - 3.0 && a < e[1] + 3.0 && u > e[2] - 3.0 && u < e[3] + 3.0 { // WHAT: in or near it. WHY: anti-aliasing round the edge
                picture.put_pixel(x, y, image::Rgba([255, 255, 255, 255])); // WHAT: white. WHY: not ink
            }
        }
    }
    picture // WHAT: masked. WHY: measured
}

/// With no Arial installed, its metric twin (Liberation Sans) is used, and said, before Helvetica. // WHAT: the stand-in. WHY: the owner's order
#[test]
fn a_metric_twin_comes_before_helvetica() {
    let scratch = Scratch::new("font-twin"); // WHAT: a folder. WHY: fixtures
    let fonts = font_folder(&scratch, "fonts", &[("LiberationSans-Regular.ttf", "Liberation Sans", "LiberationSans", 0)]); // WHAT: only the twin. WHY: no Arial
    let path = scratch.0.join("subset.pdf"); // WHAT: the fixture. WHY: one
    std::fs::write(&path, subset_sheet(0, None).0).expect("the fixture writes"); // WHAT: written. WHY: read
    let (_, said, _, after) = edit_note(&scratch, &path, "BEAM 1", "BEAM 1X", None, vec![fonts]).expect("rewritten"); // WHAT: an X typed. WHY: the case
    assert!(said.contains("Typed in Liberation Sans, the closest font on this computer, because"), "{said}"); // WHAT: the notice. WHY: says it is a stand-in
    assert!(after.font.to_lowercase().contains("liberation"), "set in {}", after.font); // WHAT: the twin. WHY: not Helvetica
}

/// With nothing that fits installed, Helvetica, and the sentence says why. // WHAT: the last resort. WHY: still works, and says so
#[test]
fn helvetica_only_when_nothing_else_fits() {
    let scratch = Scratch::new("font-none"); // WHAT: a folder. WHY: fixtures
    let fonts = font_folder(&scratch, "fonts", &[("ubuntu.ttf", "Ubuntu", "Ubuntu-Light", 0)]); // WHAT: an unrelated font only. WHY: nothing of Arial's widths
    let path = scratch.0.join("subset.pdf"); // WHAT: the fixture. WHY: one
    std::fs::write(&path, subset_sheet(0, None).0).expect("the fixture writes"); // WHAT: written. WHY: read
    let (_, said, _, after) = edit_note(&scratch, &path, "BEAM 1", "BEAM 1X", None, vec![fonts]).expect("rewritten"); // WHAT: an X typed. WHY: the case
    assert!(said.contains("Typed in Helvetica because the drawing's font didn't include those letters and no matching font is on this computer."), "{said}"); // WHAT: said. WHY: plain
    assert!(after.font.to_lowercase().contains("helvetica"), "set in {}", after.font); // WHAT: Helvetica. WHY: nothing else fits
}

/// Letters the cut-down font already has keep its own glyphs: no whole font is reached for. // WHAT: the existing glyphs kept. WHY: the owner's rule: only when needed
#[test]
fn letters_the_subset_has_keep_its_own_glyphs() {
    let scratch = Scratch::new("font-own"); // WHAT: a folder. WHY: fixtures
    let fonts = font_folder(&scratch, "fonts", &[("arial.ttf", "Arial", "ArialMT", 0x0008)]); // WHAT: Arial installed. WHY: available but not wanted
    let path = scratch.0.join("subset.pdf"); // WHAT: the fixture. WHY: one
    let (file, _) = subset_sheet(0, None); // WHAT: the sheet. WHY: its size compared
    std::fs::write(&path, &file).expect("the fixture writes"); // WHAT: written. WHY: read
    let (out, said, before, after) = edit_note(&scratch, &path, "BEAM 1", "MAE 21", None, vec![fonts]).expect("rewritten"); // WHAT: only letters it has. WHY: the case
    assert!(!said.contains("Typed in"), "{said}"); // WHAT: no notice. WHY: nothing was substituted
    assert_eq!(after.font, before.font, "its own font was replaced"); // WHAT: the same font. WHY: its glyphs kept
    let grew = std::fs::metadata(&out).expect("the result").len() as usize - file.len(); // WHAT: how much the file grew. WHY: a whole font would add its size
    assert!(grew < whole_font_size() / 2, "grew by {grew} bytes: a whole font went in"); // WHAT: no font added. WHY: the same
}

/// A font whose licence forbids embedding is never put in the file: the next permitted one is used, and the person is told in one sentence. // WHAT: the fsType rule. WHY: the owner's rule
#[test]
fn a_font_that_forbids_embedding_is_not_embedded() {
    let scratch = Scratch::new("font-licence"); // WHAT: a folder. WHY: fixtures
    let path = scratch.0.join("subset.pdf"); // WHAT: the fixture. WHY: one
    let (file, _) = subset_sheet(0, None); // WHAT: the sheet. WHY: its size compared
    std::fs::write(&path, &file).expect("the fixture writes"); // WHAT: written. WHY: read
    // Arial restricted, Liberation Sans allowed: Liberation Sans, and said.
    let fonts = font_folder(&scratch, "twin", &[("arial.ttf", "Arial", "ArialMT", 0x0002), ("LiberationSans-Regular.ttf", "Liberation Sans", "LiberationSans", 0)]); // WHAT: a restricted Arial and a free twin. WHY: the twin must be used
    let (_, said, _, after) = edit_note(&scratch, &path, "BEAM 1", "BEAM 1X", None, vec![fonts]).expect("rewritten"); // WHAT: an X typed. WHY: the case
    assert!(after.font.to_lowercase().contains("liberation"), "set in {}", after.font); // WHAT: the permitted one. WHY: Arial forbids embedding
    assert!(said.contains("Arial's licence doesn't allow it to be saved inside a drawing, so it wasn't used."), "{said}"); // WHAT: told. WHY: one plain sentence
    // Only a restricted Arial, and one that allows preview and print only: Helvetica, nothing embedded.
    let fonts = font_folder(&scratch, "restricted", &[("arial.ttf", "Arial", "ArialMT", 0x0002), ("LiberationSans-Regular.ttf", "Liberation Sans", "LiberationSans", 0x0004)]); // WHAT: neither allows an editable file. WHY: nothing may go in
    let (out, said, _, after) = edit_note(&scratch, &path, "BEAM 1", "BEAM 1X", None, vec![fonts]).expect("rewritten"); // WHAT: the same edit. WHY: the case
    assert!(after.font.to_lowercase().contains("helvetica"), "set in {}", after.font); // WHAT: Helvetica. WHY: nothing permitted fits
    assert!(said.contains("The licences of Arial and Liberation Sans don't allow them to be saved inside a drawing"), "{said}"); // WHAT: told. WHY: the same
    let grew = std::fs::metadata(&out).expect("the result").len() as usize - file.len(); // WHAT: how much the file grew. WHY: an embedded font would add its size
    assert!(grew < whole_font_size() / 2, "grew by {grew} bytes: a restricted font went in"); // WHAT: no font added. WHY: the rule
    assert_eq!(whole_fonts_named(&out, "arial") + whole_fonts_named(&out, "liberation"), 0, "a font that forbids embedding is in the sheet"); // WHAT: neither is named by the sheet. WHY: the same rule, read from the file
}

/// A right-aligned note set in the whole font ends where the old one did: its line is laid out with the new font's widths. // WHAT: re-layout with the new widths. WHY: widths from the cut-down font would misplace it
#[test]
fn a_note_set_in_the_whole_font_is_laid_out_with_its_widths() {
    let scratch = Scratch::new("font-widths"); // WHAT: a folder. WHY: fixtures
    let fonts = font_folder(&scratch, "fonts", &[("arial.ttf", "Arial", "ArialMT", 0x0008)]); // WHAT: Arial installed. WHY: used
    for (sheet_name, rotate, crop) in SHEETS { // WHAT: every turn. WHY: the same
        let path = scratch.0.join(format!("subset-{sheet_name}.pdf")); // WHAT: the fixture. WHY: one each
        std::fs::write(&path, subset_sheet(*rotate, *crop).0).expect("the fixture writes"); // WHAT: written. WHY: read
        let (_, _, before, after) = edit_note(&scratch, &path, "BEAM 1", "XBEAM 1", Some(hyperview::textedit::Align::Right), vec![fonts.clone()]).expect("rewritten"); // WHAT: an X in front, set right-aligned. WHY: its start depends on every letter's width
        let (old, new) = (box_extent(&before), box_extent(&after)); // WHAT: both boxes. WHY: their right ends compared
        assert!((old[1] - new[1]).abs() < 0.15 * before.size + SLACK, "[{sheet_name}] ended at {:.2}, now {:.2}", old[1], new[1]); // WHAT: the same right end. WHY: within the alignment test's own slack
    }
}

/// The whole fonts a sheet's resources name, by name, without counting cut-down ones. // WHAT: the live whole fonts on a sheet. WHY: pdfium's incremental save writes every object it read again, so file size can't tell one copy from two
fn whole_fonts_named(path: &Path, family: &str) -> usize {
    let file = pdf::Document::from_bytes(std::fs::read(path).expect("the file")); // WHAT: the file. WHY: its latest objects
    let page = file.page(0).expect("its sheet"); // WHAT: the sheet. WHY: one
    let resources = file.page_attr(&page, "Resources"); // WHAT: its resources. WHY: its fonts are named there
    let fonts = file.at(resources.as_dict().expect("resources"), "Font"); // WHAT: its fonts. WHY: counted
    let mut seen = std::collections::HashSet::new(); // WHAT: font objects already counted. WHY: two names for one object are one font
    for (_, value) in fonts.as_dict().expect("fonts").iter() { // WHAT: each font named. WHY: counted
        let font = file.follow(value); // WHAT: the font. WHY: its name
        let Some(dict) = font.as_dict() else { continue }; // WHAT: a dictionary. WHY: anything else is not a font
        let name = dict.get("BaseFont").and_then(|o| o.as_name()).map(|n| n.as_str().to_string()).unwrap_or_default(); // WHAT: its name. WHY: matched
        if !hyperview::textedit::is_subset(&name) && name.to_lowercase().contains(family) { // WHAT: a whole one of that family. WHY: counted
            seen.insert(format!("{value:?}")); // WHAT: by its reference. WHY: the same object once
        }
    }
    seen.len() // WHAT: how many. WHY: one is right
}

/// A second note on the same sheet set in the same whole font uses the copy already in the file. // WHAT: no second copy. WHY: a sheet edited ten times doesn't carry ten copies of Arial
#[test]
fn a_whole_font_goes_into_a_file_once() {
    let scratch = Scratch::new("font-once"); // WHAT: a folder. WHY: fixtures
    let fonts = font_folder(&scratch, "fonts", &[("arial.ttf", "Arial", "ArialMT", 0x0008)]); // WHAT: Arial installed. WHY: used
    let path = scratch.0.join("subset.pdf"); // WHAT: the fixture. WHY: one
    std::fs::write(&path, subset_sheet(0, None).0).expect("the fixture writes"); // WHAT: written. WHY: read
    let (first, _, _, _) = edit_note(&scratch, &path, "BEAM 1", "BEAM 1X", None, vec![fonts.clone()]).expect("the first edit"); // WHAT: the first note, Arial put in. WHY: the copy
    assert_eq!(whole_fonts_named(&first, "arial"), 1, "the first edit put in no whole Arial"); // WHAT: one copy. WHY: the fixture is right
    let (second, said, _, after) = edit_note(&scratch, &first, "BEAM 2", "BEAM 2X", None, vec![fonts]).expect("the second edit"); // WHAT: the other note, also needing Arial. WHY: the copy should be used again
    assert!(after.font.to_lowercase().contains("arial"), "set in {}", after.font); // WHAT: Arial. WHY: the same family
    assert!(said.contains("Typed in Arial"), "{said}"); // WHAT: said. WHY: the same
    assert_eq!(whole_fonts_named(&second, "arial"), 1, "Arial went into the sheet twice"); // WHAT: still one copy. WHY: the guard
}
