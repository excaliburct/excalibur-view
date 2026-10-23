//! Overlay Pages, run for real on pairs of drawings that look like revisions.
//!
//! Every test here writes two issues of a sheet (or a set) by hand, lays one
//! over the other through pdfium exactly as the program does, then draws what
//! came out and counts its colours. Red is the older issue, blue the newer,
//! dark where both agree. A pair that has not really changed should come out
//! dark, and a pair with one beam moved should show that beam in red and blue
//! and nothing else.

use std::path::{Path, PathBuf}; // WHAT: file paths. WHY: every test writes and reads files.

use hyperview::docops::Operation; // WHAT: the whole-set overlay. WHY: the batch and the Document menu both run it.

/// A scratch directory that cleans up after itself.
struct Scratch(PathBuf); // WHAT: the folder's path. WHY: removed when the test ends.

// WHAT: making and using the folder. WHY: every test.
impl Scratch {
    // WHAT: a new folder. WHY: one per test.
    fn new(what: &str) -> Scratch {
        let n: u64 = std::time::SystemTime::now() // WHAT: the clock. WHY: a different folder every run.
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::from(d.subsec_nanos()))
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("hyperview-overlay-{what}-{n:08x}")); // WHAT: the folder. WHY: one per test, in the temp folder.
        std::fs::create_dir_all(&path).expect("a scratch directory"); // WHAT: makes it. WHY: the files go in it.
        Scratch(path) // WHAT: hands it back. WHY: dropped at the end of the test.
    }
    // WHAT: a path in it. WHY: fixtures.
    fn at(&self, name: &str) -> PathBuf {
        self.0.join(name) // WHAT: a file in the folder. WHY: every fixture is named.
    }
}

// WHAT: cleanup. WHY: nothing left behind.
impl Drop for Scratch {
    // WHAT: removes it. WHY: the same.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0); // WHAT: deletes the folder. WHY: tests leave nothing behind.
    }
}

/// The folder pdfium's library is in, for a test binary that is not beside one.
// WHAT: where pdfium is. WHY: the engine needs it.
fn library() -> Option<PathBuf> {
    // WHAT: each place it might be. WHY: tried in turn.
    for guess in [
        "third_party/pdfium/linux-x64/lib",
        "../third_party/pdfium/linux-x64/lib",
        "../../third_party/pdfium/linux-x64/lib",
    ] {
        let path = PathBuf::from(guess); // WHAT: one place it might be. WHY: tests run from the crate or the workspace.

        // WHAT: it is there. WHY: used.
        if path.join("libpdfium.so").exists() {
            return Some(path); // WHAT: found it. WHY: the engine loads it from there.
        }
    }
    None // WHAT: let the engine find its own. WHY: the embedded copy, as the program does.
}

/// One page of a fixture: its boxes, its turn and what is drawn on it.
#[derive(Clone)]
// WHAT: a page to write. WHY: fixtures.
struct Page {
    media: [f64; 4], // WHAT: the paper, in points. WHY: sheets come on different paper.
    crop: Option<[f64; 4]>, // WHAT: a crop box, if any. WHY: a cropped revision shows a window of its paper.
    rotate: i32,            // WHAT: the page's /Rotate. WHY: a sheet can be stored turned.
    content: String,        // WHAT: the drawing's content stream. WHY: what is printed on it.
    image: Option<(u32, u32, Vec<u8>)>, // WHAT: a grey picture drawn as /Im0. WHY: a scanned sheet is one picture.
    annots: Vec<[f64; 4]>, // WHAT: square markups, by rectangle. WHY: somebody's takeoff on the sheet.
}

// WHAT: page shorthands. WHY: readable tests.
impl Page {
    /// A plain 36 by 24 inch sheet with this drawn on it.
    // WHAT: a plain ARCH D page. WHY: the common case.
    fn arch_d(content: String) -> Page {
        Page {
            media: [0.0, 0.0, 2592.0, 1728.0],
            crop: None,
            rotate: 0,
            content,
            image: None,
            annots: Vec::new(),
        } // WHAT: the sheet. WHY: ARCH D is the commonest structural sheet.
    }
}

/// Writes pages into a PDF by hand, so nothing here depends on a file somebody has to supply.
// WHAT: writes a PDF. WHY: fixtures without supplied files.
fn write_pdf(to: &Path, pages: &[Page]) {
    let mut out: Vec<u8> = b"%PDF-1.7\n".to_vec(); // WHAT: the header. WHY: every PDF starts with it.
    let mut offsets: Vec<usize> = Vec::new(); // WHAT: where each object starts. WHY: the cross-reference table lists them.
    let mut objects: Vec<Vec<u8>> = Vec::new(); // WHAT: each object's bytes, numbered from 1. WHY: written in order at the end.
    objects.push(b"<</Type/Catalog/Pages 2 0 R>>".to_vec()); // WHAT: object 1, the catalog. WHY: the root of the file.
    objects.push(Vec::new()); // WHAT: object 2, filled in once the pages are numbered. WHY: it lists them.
    objects.push(b"<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>".to_vec()); // WHAT: object 3, the font. WHY: sheet numbers and notes are text.
    let mut kids = String::new(); // WHAT: the page references. WHY: the page tree's /Kids.

    // WHAT: each page. WHY: written.
    for page in pages {
        let stream_number = objects.len() + 1; // WHAT: the content stream's number. WHY: the page points at it.
        objects.push(
            format!(
                "<</Length {}>>\nstream\n{}\nendstream",
                page.content.len() + 1,
                page.content // WHAT: the drawing. WHY: the stream's body.
            )
            .into_bytes(),
        ); // WHAT: the content stream. WHY: the drawing.
        let mut resources = String::from("/Font<</F1 3 0 R>>"); // WHAT: the page's resources. WHY: the text needs its font.

        // WHAT: a scanned picture. WHY: written as an image.
        if let Some((w, h, grey)) = &page.image {
            let image_number = objects.len() + 1; // WHAT: the picture's number. WHY: the resources point at it.
            let mut body = format!("<</Type/XObject/Subtype/Image/Width {w}/Height {h}/ColorSpace/DeviceGray/BitsPerComponent 8/Length {}>>\nstream\n", grey.len()).into_bytes(); // WHAT: the picture's dictionary. WHY: uncompressed grey, the plainest a scanner writes.
            body.extend_from_slice(grey); // WHAT: the pixels. WHY: the scan itself.
            body.extend_from_slice(b"\nendstream"); // WHAT: the end of the stream. WHY: the format.
            objects.push(body); // WHAT: the picture. WHY: drawn by the content stream.

            // WHAT: names it /Im0. WHY: the content stream draws it by that name.
            resources.push_str(&format!("/XObject<</Im0 {image_number} 0 R>>"));
        }
        let mut annots = String::new(); // WHAT: the page's markups. WHY: listed in /Annots.

        // WHAT: each markup. WHY: written.
        for rect in &page.annots {
            let ap_number = objects.len() + 1; // WHAT: the markup's appearance. WHY: what a reader draws.
            let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]); // WHAT: its size. WHY: the appearance is drawn in its own box.
            let ap = format!("0 0 1 RG 8 w 4 4 {} {} re S", w - 8.0, h - 8.0); // WHAT: a thick blue box. WHY: a takeoff markup as Revu draws one.
            objects.push(format!("<</Type/XObject/Subtype/Form/BBox[0 0 {w} {h}]/Length {}>>\nstream\n{ap}\nendstream", ap.len()).into_bytes()); // WHAT: the appearance stream. WHY: drawn when the page is.
            let annot_number = objects.len() + 1; // WHAT: the markup's number. WHY: the page lists it.
            objects.push(format!("<</Type/Annot/Subtype/Square/Rect[{} {} {} {}]/C[0 0 1]/Subj(Takeoff)/AP<</N {ap_number} 0 R>>>>", rect[0], rect[1], rect[2], rect[3]).into_bytes()); // WHAT: the markup. WHY: a square, the commonest takeoff markup.
            annots.push_str(&format!("{annot_number} 0 R ")); // WHAT: lists it. WHY: the page's /Annots.
        }
        let page_number = objects.len() + 1; // WHAT: the page's own number. WHY: the page tree lists it.
        let crop = page
            .crop
            .map(|c| format!("/CropBox[{} {} {} {}]", c[0], c[1], c[2], c[3]))
            .unwrap_or_default(); // WHAT: the crop box, if any. WHY: a cropped revision.
        let m = page.media; // WHAT: the paper. WHY: short to write.
        objects.push(format!("<</Type/Page/Parent 2 0 R/MediaBox[{} {} {} {}]{crop}/Rotate {}/Resources<<{resources}>>/Contents {stream_number} 0 R/Annots[{annots}]>>", m[0], m[1], m[2], m[3], page.rotate).into_bytes()); // WHAT: the page. WHY: its boxes, turn, resources and content.
        kids.push_str(&format!("{page_number} 0 R ")); // WHAT: lists it. WHY: the page tree's /Kids.
    }
    objects[1] = format!("<</Type/Pages/Kids[{kids}]/Count {}>>", pages.len()).into_bytes(); // WHAT: object 2, the page tree. WHY: now every page has its number.

    // WHAT: each object. WHY: written in order.
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len()); // WHAT: where it starts. WHY: the cross-reference table.
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes()); // WHAT: its header. WHY: the format.
        out.extend_from_slice(body); // WHAT: its body. WHY: the object.
        out.extend_from_slice(b"\nendobj\n"); // WHAT: its end. WHY: the format.
    }
    let xref = out.len(); // WHAT: where the table starts. WHY: startxref points at it.
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    ); // WHAT: the table's header. WHY: the format.

    // WHAT: each offset. WHY: the table.
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes()); // WHAT: one entry. WHY: the format.
    }
    out.extend_from_slice(
        format!(
            "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    ); // WHAT: the trailer. WHY: the format.
    std::fs::write(to, out).expect("a fixture"); // WHAT: writes it. WHY: the engine reads it from disk.
}

/// A structural framing plan on a 36 by 24 sheet, in that sheet's points.
///
/// A border, a grid, beams between the grid lines, notes down the side and a
/// title block whose number the sheet reader finds. `beam_at` is where the
/// one beam that moves between issues sits, across the sheet.
// WHAT: a framing plan. WHY: a realistic sheet.
fn plan(number: &str, beam_at: f64) -> String {
    let mut s = String::from("0 0 0 RG 3 w 36 36 2520 1656 re S\n"); // WHAT: the border. WHY: every sheet has one.
    s.push_str("2 w 2100 36 456 264 re S\n"); // WHAT: the title block. WHY: bottom right, where a sheet's number is.

    // WHAT: grid lines across. WHY: a grid.
    for i in 0..8 {
        let x = 300.0 + f64::from(i) * 220.0; // WHAT: one grid line across. WHY: a plan's grid.
        s.push_str(&format!("1 w {x} 420 m {x} 1600 l S\n")); // WHAT: draws it. WHY: thin, as grids are.
    }
    // WHAT: grid lines up. WHY: the same.
    for j in 0..5 {
        let y = 450.0 + f64::from(j) * 270.0; // WHAT: one grid line up. WHY: the same.
        s.push_str(&format!("1 w 260 {y} m 1900 {y} l S\n")); // WHAT: draws it. WHY: the same.
    }
    // WHAT: beams. WHY: heavy lines.
    for j in 0..5 {
        let y = 450.0 + f64::from(j) * 270.0; // WHAT: a beam line. WHY: beams run along the grid.
        s.push_str(&format!("6 w 300 {y} m 1840 {y} l S\n")); // WHAT: draws it thick. WHY: beams are drawn heavy.
    }
    s.push_str(&format!("6 w {beam_at} 450 m {beam_at} 1530 l S\n")); // WHAT: the beam that moves. WHY: the one thing that changed.

    // WHAT: note rows. WHY: body text.
    for row in 0..24 {
        s.push_str(&format!(
            "BT /F1 11 Tf 1960 {} Td (NOTE {row} SEE SPECS) Tj ET\n",
            1600 - row * 40
        )); // WHAT: the notes. WHY: the sheet reader wants body text to tell the number from.
    }
    s.push_str("BT /F1 14 Tf 2140 210 Td (FRAMING PLAN) Tj ET\n"); // WHAT: the title. WHY: above the number, as a title block has it.
    s.push_str(&format!("BT /F1 48 Tf 2140 90 Td ({number}) Tj ET\n")); // WHAT: the sheet number. WHY: big and in the corner, so it is read as the number.
    s // WHAT: the plan. WHY: handed back.
}

/// The same plan, placed on paper by a transform (a b c d e f).
// WHAT: the plan moved, scaled or turned. WHY: revision pairs.
fn placed(number: &str, beam_at: f64, m: [f64; 6]) -> String {
    format!(
        "q {} {} {} {} {} {} cm\n{}Q\n",
        m[0],
        m[1],
        m[2],
        m[3],
        m[4],
        m[5],
        plan(number, beam_at)
    ) // WHAT: the plan inside a transform. WHY: scaled, shifted and turned copies of one drawing.
}

/// How the colours of an overlay came out.
#[derive(Debug, Default, Clone, Copy)]
// WHAT: a colour tally. WHY: the answer.
struct Colours {
    older: usize, // WHAT: red pixels. WHY: ink only in the older issue.
    newer: usize, // WHAT: blue pixels. WHY: ink only in the newer issue.
    both: usize,  // WHAT: dark pixels. WHY: ink in both.
}

// WHAT: what the tally says. WHY: the same.
impl Colours {
    /// The share of the ink that is coloured: what the overlay says changed.
    // WHAT: the coloured share. WHY: how much changed.
    fn changed(&self) -> f64 {
        let ink = self.older + self.newer + self.both; // WHAT: every inked pixel. WHY: the whole of the line-work.

        // WHAT: no ink. WHY: nothing changed.
        if ink == 0 {
            return 0.0; // WHAT: no ink is no change. WHY: no dividing by nothing.
        }
        (self.older + self.newer) as f64 / ink as f64 // WHAT: the coloured share. WHY: a revision that did not change reads near nought.
    }
}

/// Counts the colours in a picture: red, blue and dark.
// WHAT: counts colours. WHY: the answer.
fn count(image: &image::RgbaImage, area: Option<[u32; 4]>) -> Colours {
    let mut c = Colours::default(); // WHAT: the tally. WHY: counted below.
    let [x0, y0, x1, y1] = area.unwrap_or([0, 0, image.width(), image.height()]); // WHAT: where to count. WHY: some tests look at one part of the sheet.

    // WHAT: each row. WHY: counted.
    for y in y0..y1.min(image.height()) {
        // WHAT: each pixel. WHY: counted.
        for x in x0..x1.min(image.width()) {
            let p = image.get_pixel(x, y).0; // WHAT: one pixel. WHY: classified below.
            let (r, g, b) = (i32::from(p[0]), i32::from(p[1]), i32::from(p[2])); // WHAT: its channels. WHY: compared with each other.
            let hi = r.max(g).max(b); // WHAT: its brightest channel. WHY: paper is bright in all three.
            let lo = r.min(g).min(b); // WHAT: its darkest channel. WHY: grey has all three close.

            // WHAT: paper. WHY: skipped.
            if lo > 215 {
                continue; // WHAT: paper. WHY: not ink.
            }
            // WHAT: red. WHY: older.
            if r > b + 60 && r > g + 40 {
                c.older += 1; // WHAT: red. WHY: the older issue's colour.
            } else if b > r + 60 {
                // WHAT: blue. WHY: newer.
                c.newer += 1; // WHAT: blue. WHY: the newer issue's colour.
            } else if hi - lo < 40 && hi < 150 {
                // WHAT: dark grey. WHY: both.
                c.both += 1; // WHAT: dark grey. WHY: both issues' colour.
            }
        }
    }
    c // WHAT: the tally. WHY: handed back.
}

/// Lays `older` over `newer` as a whole set and hands back the file it wrote.
// WHAT: a whole-set overlay. WHY: the Document menu's path.
fn overlay(older: &Path, newer: &Path, to: &Path) -> hyperview::docops::Done {
    let op = Operation::Overlay {
        newer: newer.to_path_buf(),
        older: older.to_path_buf(),
        to: to.to_path_buf(),
        darkness: 0.65,
    }; // WHAT: the job. WHY: the same one the Document and Batch menus send.
    hyperview::render::do_one(library(), &op).expect("the overlay should run") // WHAT: runs it. WHY: through pdfium, for real.
}

/// One sheet of a file as a picture, `across` pixels wide or `down` high.
// WHAT: a sheet as a picture. WHY: counted.
fn picture(path: &Path, page: u32, across: u32, down: u32, scratch: &Scratch) -> image::RgbaImage {
    let png = scratch.at(&format!("look-{page}-{across}.png")); // WHAT: where to draw it. WHY: the engine writes a picture file.
    hyperview::render::picture_of(library(), path, page, across, down, &png)
        .expect("the sheet should draw"); // WHAT: draws it. WHY: the same drawing the viewer shows.
    image::open(&png).expect("a picture").to_rgba8() // WHAT: reads it back. WHY: counted pixel by pixel.
}

/// Saves a picture into the folder named by HYPERVIEW_OVERLAY_SHOTS, if set.
// WHAT: saves a copy when asked. WHY: a person checks by eye.
fn keep_a_copy(look: &image::RgbaImage, scratch: &Scratch) {
    // WHAT: a folder was named. WHY: saved there.
    if let Ok(folder) = std::env::var("HYPERVIEW_OVERLAY_SHOTS") {
        let name = scratch
            .0
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(); // WHAT: the test's folder name. WHY: says which test it came from.
        let _ = look.save(Path::new(&folder).join(format!("{name}.png"))); // WHAT: writes it. WHY: looked at afterwards; a failure to save is not the test's business.
    }
}

/// A pair laid over each other, drawn and counted.
// WHAT: a pair laid over and counted. WHY: most tests.
fn colours_of(older: &[Page], newer: &[Page], scratch: &Scratch) -> (Colours, String) {
    let (a, b, to) = (
        scratch.at("older.pdf"),
        scratch.at("newer.pdf"),
        scratch.at("overlay.pdf"),
    ); // WHAT: the three files. WHY: two issues in, one overlay out.
    write_pdf(&a, older); // WHAT: the older issue. WHY: on disk for the engine.
    write_pdf(&b, newer); // WHAT: the newer issue. WHY: the same.
    let done = overlay(&a, &b, &to); // WHAT: lays them over. WHY: the thing under test.
    let look = picture(&to, 0, 1800, 1800, scratch); // WHAT: the first overlaid sheet as a picture. WHY: its colours are the answer.
    keep_a_copy(&look, scratch); // WHAT: saves it to look at, when asked. WHY: a person checks the colours by eye too.
    (count(&look, None), done.said) // WHAT: the colours and what it said. WHY: both are checked.
}

#[test]
// WHAT: the test. WHY: a moved beam shows, the rest does not.
fn one_beam_moved_shows_in_red_and_blue_and_nothing_else_does() {
    let scratch = Scratch::new("moved"); // WHAT: a folder. WHY: the fixtures go in it.
    let (c, said) = colours_of(
        &[Page::arch_d(plan("S-201", 700.0))],
        &[Page::arch_d(plan("S-201", 1300.0))],
        &scratch,
    ); // WHAT: the beam moves 600 points. WHY: the plainest revision there is.
    assert!(
        c.older > 300 && c.newer > 300,
        "the moved beam should show in both colours: {c:?} {said}"
    ); // WHAT: red and blue are there. WHY: where it was and where it went.
    assert!(
        c.changed() < 0.12,
        "only the beam changed, so most of the ink should be dark: {c:?} {:.3}",
        c.changed()
    ); // WHAT: the rest is dark. WHY: nothing else moved.
}

#[test]
// WHAT: the test. WHY: a re-plot is not a change.
fn a_sheet_replotted_a_little_over_comes_out_dark() {
    let scratch = Scratch::new("shifted"); // WHAT: a folder. WHY: the fixtures.
    let newer = Page::arch_d(placed("S-201", 700.0, [1.0, 0.0, 0.0, 1.0, 14.0, -9.0])); // WHAT: the same drawing, 14 points right and 9 down. WHY: a different plotter.
    let (c, said) = colours_of(&[Page::arch_d(plan("S-201", 700.0))], &[newer], &scratch); // WHAT: lays them over. WHY: the test.
    assert!(
        c.changed() < 0.04,
        "a re-plot has not changed: {c:?} {:.3} {said}",
        c.changed()
    ); // WHAT: nearly no colour. WHY: lined up, nothing moved.
}

#[test]
// WHAT: the test. WHY: a half-size print lines up.
fn a_half_size_print_lines_up_with_the_full_size_sheet() {
    let scratch = Scratch::new("halfsize"); // WHAT: a folder. WHY: the fixtures.
    let mut newer = Page::arch_d(placed("S-201", 700.0, [0.5, 0.0, 0.0, 0.5, 0.0, 0.0])); // WHAT: the same drawing at half size. WHY: an 18 by 12 half-size set.
    newer.media = [0.0, 0.0, 1296.0, 864.0]; // WHAT: on half-size paper. WHY: the same.
    let (c, said) = colours_of(&[Page::arch_d(plan("S-201", 700.0))], &[newer], &scratch); // WHAT: lays them over. WHY: the test.
    assert!(
        c.changed() < 0.04,
        "a half-size print is the same drawing: {c:?} {:.3} {said}",
        c.changed()
    ); // WHAT: nearly no colour. WHY: same drawing, smaller paper.
}

#[test]
// WHAT: the test. WHY: bigger paper lines up.
fn the_same_drawing_on_bigger_paper_lines_up() {
    let scratch = Scratch::new("bigger"); // WHAT: a folder. WHY: the fixtures.
    let mut newer = Page::arch_d(plan("S-201", 700.0)); // WHAT: the same drawing at the same scale. WHY: plotted on a bigger sheet.
    newer.media = [0.0, 0.0, 3024.0, 2160.0]; // WHAT: 42 by 30 paper. WHY: ARCH E1, the next size up.
    let (c, said) = colours_of(&[Page::arch_d(plan("S-201", 700.0))], &[newer], &scratch); // WHAT: lays them over. WHY: the test.
    assert!(
        c.changed() < 0.04,
        "the paper grew and the drawing did not: {c:?} {:.3} {said}",
        c.changed()
    ); // WHAT: nearly no colour. WHY: same drawing, same scale.
}

#[test]
// WHAT: the test. WHY: a crop lines up.
fn a_cropped_revision_lines_up_with_the_whole_sheet() {
    let scratch = Scratch::new("cropped"); // WHAT: a folder. WHY: the fixtures.
    let mut newer = Page::arch_d(plan("S-201", 700.0)); // WHAT: the same drawing. WHY: only the crop differs.
    newer.crop = Some([150.0, 120.0, 2592.0, 1728.0]); // WHAT: 150 points off the left, 120 off the bottom. WHY: a revision cropped tighter.
    let (c, said) = colours_of(&[Page::arch_d(plan("S-201", 700.0))], &[newer], &scratch); // WHAT: lays them over. WHY: the test.
    assert!(
        c.changed() < 0.04,
        "a crop is a window, not a change: {c:?} {:.3} {said}",
        c.changed()
    ); // WHAT: nearly no colour. WHY: same drawing.
}

#[test]
// WHAT: the test. WHY: a sideways sheet lines up.
fn a_sheet_issued_turned_a_quarter_lines_up() {
    let scratch = Scratch::new("turned"); // WHAT: a folder. WHY: the fixtures.
    let mut newer = Page::arch_d(placed("S-201", 700.0, [0.0, 1.0, -1.0, 0.0, 1728.0, 0.0])); // WHAT: the drawing turned a quarter on portrait paper. WHY: a sheet scanned or plotted sideways.
    newer.media = [0.0, 0.0, 1728.0, 2592.0]; // WHAT: portrait paper. WHY: the same.
    let (c, said) = colours_of(&[Page::arch_d(plan("S-201", 700.0))], &[newer], &scratch); // WHAT: lays them over. WHY: the test.
    assert!(
        c.changed() < 0.05,
        "a turned sheet is the same drawing: {c:?} {:.3} {said}",
        c.changed()
    ); // WHAT: nearly no colour. WHY: same drawing.
}

#[test]
// WHAT: the test. WHY: a /Rotate flag lines up.
fn a_sheet_stored_with_a_rotate_flag_lines_up() {
    let scratch = Scratch::new("rotateflag"); // WHAT: a folder. WHY: the fixtures.
    let mut newer = Page::arch_d(placed("S-201", 700.0, [0.0, 1.0, -1.0, 0.0, 1728.0, 0.0])); // WHAT: the drawing written turned. WHY: the /Rotate below turns it back.
    newer.media = [0.0, 0.0, 1728.0, 2592.0]; // WHAT: portrait paper. WHY: the same.
    newer.rotate = 90; // WHAT: shown a quarter turned. WHY: how many plotters store a landscape sheet.
    let (c, said) = colours_of(&[Page::arch_d(plan("S-201", 700.0))], &[newer], &scratch); // WHAT: lays them over. WHY: the test.
    assert!(
        c.changed() < 0.04,
        "a /Rotate flag shows the same drawing: {c:?} {:.3} {said}",
        c.changed()
    ); // WHAT: nearly no colour. WHY: same drawing.
    let laid = hyperview::render::overlay_two(
        library(),
        &scratch.at("older.pdf"),
        0,
        &scratch.at("newer.pdf"),
        0,
        0.65,
        &hyperview::overlay::Lining::Auto,
    )
    .expect("the overlay should run"); // WHAT: the same pair, one sheet. WHY: to read what it says.
    assert!(
        !laid.said.contains("turned"),
        "shown the same way up, so nothing was turned: {}",
        laid.said
    ); // WHAT: no turn. WHY: /Rotate already shows it the right way up.
}

#[test]
// WHAT: the test. WHY: a skewed scan lines up.
fn a_scan_a_little_skewed_lines_up() {
    let scratch = Scratch::new("skewed"); // WHAT: a folder. WHY: the fixtures.
    let turn = 0.6_f64.to_radians(); // WHAT: six tenths of a degree. WHY: what a sheet feeder does.
    let (s, c) = (turn.sin(), turn.cos()); // WHAT: the turn's sine and cosine. WHY: the transform.
    let newer = Page::arch_d(placed(
        "S-201",
        700.0,
        [
            c,
            s,
            -s,
            c,
            1296.0 - c * 1296.0 + s * 864.0,
            864.0 - s * 1296.0 - c * 864.0,
        ],
    )); // WHAT: turned about the middle. WHY: a skewed feed.
    let (colours, said) = colours_of(&[Page::arch_d(plan("S-201", 700.0))], &[newer], &scratch); // WHAT: lays them over. WHY: the test.
    assert!(
        colours.changed() < 0.06,
        "a skew is not a change: {colours:?} {:.3} {said}",
        colours.changed()
    ); // WHAT: nearly no colour. WHY: same drawing.
}

/// The plan, drawn and turned into a grey scan: paper not white, a little noise.
// WHAT: a scanned plan. WHY: scan against vector.
fn scanned(number: &str, beam_at: f64, scratch: &Scratch) -> Page {
    let vector = scratch.at("to-scan.pdf"); // WHAT: the plan as vectors first. WHY: the scan is a picture of it.
    write_pdf(&vector, &[Page::arch_d(plan(number, beam_at))]); // WHAT: writes it. WHY: drawn below.
    let look = picture(&vector, 0, 2400, 1600, scratch); // WHAT: draws it at 66 dots an inch. WHY: a cheap scan.
    let (w, h) = (look.width(), look.height()); // WHAT: its size. WHY: the picture's dictionary.
    let mut seed: u32 = 12345; // WHAT: a fixed seed. WHY: the same noise every run.
    let mut grey = Vec::with_capacity((w * h) as usize); // WHAT: the scan's pixels. WHY: grey, one byte each.

    // WHAT: each pixel. WHY: greyed and noised.
    for p in look.pixels() {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345); // WHAT: the next random number. WHY: scanner noise.
        let noise = i32::try_from((seed >> 16) % 21).unwrap_or(0) - 10; // WHAT: plus or minus ten. WHY: a scan is never flat.
        let ink = i32::from(p.0[0]); // WHAT: how dark the drawing is here. WHY: white paper reads 255.
        let paper = 160; // WHAT: the scan's paper. WHY: a scanned print is grey, darker than a fixed line-darkness cut takes paper to be.
        let v = (ink * paper / 255 + noise).clamp(0, 255); // WHAT: the drawing on grey paper, with noise. WHY: a scan.
        grey.push(u8::try_from(v).unwrap_or(0)); // WHAT: the byte. WHY: the scan's pixel.
    }
    let mut page = Page::arch_d("q 2592 0 0 1728 0 0 cm /Im0 Do Q".to_string()); // WHAT: a page that is one picture. WHY: what a scanner writes.
    page.image = Some((w, h, grey)); // WHAT: the picture. WHY: the scan.
    page // WHAT: the page. WHY: handed back.
}

#[test]
// WHAT: the test. WHY: a scan's paper is not ink.
fn a_scan_laid_over_the_vector_sheet_is_not_all_one_colour() {
    let scratch = Scratch::new("scan"); // WHAT: a folder. WHY: the fixtures.
    let newer = scanned("S-201", 700.0, &scratch); // WHAT: the newer issue scanned. WHY: it came back from the field on paper.
    let (c, said) = colours_of(&[Page::arch_d(plan("S-201", 700.0))], &[newer], &scratch); // WHAT: lays them over. WHY: the test.
    let ink = c.older + c.newer + c.both; // WHAT: everything inked. WHY: a grey page must not count as ink all over.
    assert!(
        ink < 1800 * 1200 / 5,
        "the scan's grey paper is not ink: {c:?} {said}"
    ); // WHAT: most of the sheet is paper. WHY: it is.
    assert!(
        c.changed() < 0.10,
        "a scan of the same sheet has not changed: {c:?} {:.3} {said}",
        c.changed()
    ); // WHAT: mostly dark. WHY: same drawing.
}

#[test]
// WHAT: the test. WHY: markups are not changes.
fn takeoff_markups_are_not_shown_as_a_change_to_the_drawing() {
    let scratch = Scratch::new("markups"); // WHAT: a folder. WHY: the fixtures.
    let mut newer = Page::arch_d(plan("S-201", 700.0)); // WHAT: the same drawing. WHY: only the markups differ.
    newer.annots = vec![
        [400.0, 600.0, 900.0, 900.0],
        [1200.0, 1000.0, 1700.0, 1400.0],
    ]; // WHAT: two takeoff boxes. WHY: an estimator's markups on the newer issue.
    let (c, said) = colours_of(&[Page::arch_d(plan("S-201", 700.0))], &[newer], &scratch); // WHAT: lays them over. WHY: the test.
    assert!(
        c.changed() < 0.03,
        "markups are not the drawing: {c:?} {:.3} {said}",
        c.changed()
    ); // WHAT: nearly no colour. WHY: the drawing did not change.
}

#[test]
// WHAT: the test. WHY: the overlay is not stretched.
fn the_overlaid_sheet_is_not_stretched_off_the_drawing() {
    let scratch = Scratch::new("stretch"); // WHAT: a folder. WHY: the fixtures.
    let (a, b, to) = (
        scratch.at("older.pdf"),
        scratch.at("newer.pdf"),
        scratch.at("overlay.pdf"),
    ); // WHAT: the files. WHY: in, in, out.
    write_pdf(&a, &[Page::arch_d(plan("S-201", 700.0))]); // WHAT: the older issue. WHY: the fixture.
    write_pdf(&b, &[Page::arch_d(plan("S-201", 700.0))]); // WHAT: the same, as the newer. WHY: the colours do not matter here, the places do.
    overlay(&a, &b, &to); // WHAT: lays them over. WHY: the test.
    let sheet = picture(&b, 0, 1800, 1200, &scratch); // WHAT: the newer sheet as it is. WHY: where its lines are.
    let laid = picture(&to, 0, 1800, 1200, &scratch); // WHAT: the overlay, the same size. WHY: its lines must be in the same places.
    assert_eq!(
        (sheet.width(), sheet.height()),
        (laid.width(), laid.height()),
        "the overlay is the same paper as the sheet"
    ); // WHAT: same proportions. WHY: the same paper.
       // The bottom beam line, 450 points up: find it down the middle of each.
    let row_of = |img: &image::RgbaImage| {
        (0..img.height()).rev().find(|y| {
            img.get_pixel(img.width() / 3, *y).0[0] < 160
                && img.get_pixel(img.width() / 3, *y).0[2] < 200
        })
    }; // WHAT: the lowest dark row a third across. WHY: the bottom beam, below which is only the border.
    let (want, got) = (
        row_of(&sheet).expect("a beam"),
        row_of(&laid).expect("a beam"),
    ); // WHAT: where it is in each. WHY: compared.
    assert!(
        want.abs_diff(got) <= 3,
        "the beam is at row {want} on the sheet and {got} on the overlay"
    ); // WHAT: the same row. WHY: a markup drawn on the overlay must land on the same steel.
}

#[test]
// WHAT: the test. WHY: sheets pair by number.
fn a_sheet_added_in_the_middle_of_a_set_does_not_put_every_sheet_after_it_out() {
    let scratch = Scratch::new("inserted"); // WHAT: a folder. WHY: the fixtures.
    let older = [
        Page::arch_d(plan("S-101", 700.0)),
        Page::arch_d(plan("S-102", 900.0)),
        Page::arch_d(plan("S-103", 1100.0)),
    ]; // WHAT: a three-sheet issue. WHY: the older set.
    let newer = [
        Page::arch_d(plan("S-101", 700.0)),
        Page::arch_d(plan("S-101A", 1500.0)),
        Page::arch_d(plan("S-102", 900.0)),
        Page::arch_d(plan("S-103", 1100.0)),
    ]; // WHAT: the same with S-101A put in. WHY: a revision added a sheet.
    let (a, b, to) = (
        scratch.at("older.pdf"),
        scratch.at("newer.pdf"),
        scratch.at("overlay.pdf"),
    ); // WHAT: the files. WHY: in, in, out.
    write_pdf(&a, &older); // WHAT: writes the older set. WHY: the fixture.
    write_pdf(&b, &newer); // WHAT: writes the newer set. WHY: the same.
    let done = overlay(&a, &b, &to); // WHAT: lays them over. WHY: the test.
    assert_eq!(
        hyperview::render::pages_in(library(), &to).expect("the overlay opens"),
        4,
        "one overlaid sheet per newer sheet: {}",
        done.said
    ); // WHAT: every newer sheet has its page. WHY: page N of the overlay is sheet N of the newer set.

    // WHAT: each later sheet. WHY: paired right.
    for (page, number) in [(2u32, "S-102"), (3u32, "S-103")] {
        let c = count(&picture(&to, page, 1800, 1800, &scratch), None); // WHAT: that sheet's overlay. WHY: matched by number, it has not changed.
        assert!(
            c.changed() < 0.04,
            "{number} should be laid over {number}, not the sheet before it: {c:?} {}",
            done.said
        ); // WHAT: nearly no colour. WHY: same sheet.
    }
    assert!(
        done.said.contains("S-101A"),
        "the sheet with no older issue is named: {}",
        done.said
    ); // WHAT: it says which. WHY: nobody wonders where its older issue went.
}

#[test]
// WHAT: the test. WHY: a sheet with no partner keeps its page.
fn a_sheet_that_will_not_overlay_is_said_and_keeps_its_place() {
    let scratch = Scratch::new("broken"); // WHAT: a folder. WHY: the fixtures.
    let older = [Page::arch_d(plan("S-101", 700.0))]; // WHAT: a one-sheet older issue. WHY: the newer has two.
    let newer = [
        Page::arch_d(plan("S-101", 700.0)),
        Page::arch_d(plan("S-102", 900.0)),
    ]; // WHAT: two sheets, the second new. WHY: a set that grew.
    let (a, b, to) = (
        scratch.at("older.pdf"),
        scratch.at("newer.pdf"),
        scratch.at("overlay.pdf"),
    ); // WHAT: the files. WHY: in, in, out.
    write_pdf(&a, &older); // WHAT: writes it. WHY: the fixture.
    write_pdf(&b, &newer); // WHAT: writes it. WHY: the same.
    let done = overlay(&a, &b, &to); // WHAT: lays them over. WHY: the test.
    assert_eq!(
        hyperview::render::pages_in(library(), &to).expect("the overlay opens"),
        2,
        "{}",
        done.said
    ); // WHAT: both sheets are there. WHY: the new sheet is shown, not dropped.
    let c = count(&picture(&to, 1, 1800, 1800, &scratch), None); // WHAT: the new sheet's page. WHY: it has no older issue.
    assert!(
        c.newer > 1000 && c.older == 0,
        "a sheet with no older issue is all new: {c:?}"
    ); // WHAT: all blue. WHY: everything on it arrived with this issue.
}

#[test]
// WHAT: the test. WHY: unrelated sheets are said.
fn two_sheets_that_do_not_line_up_are_shown_as_they_are() {
    let scratch = Scratch::new("unrelated"); // WHAT: a folder. WHY: the fixtures.
    let other = "0 0 0 RG 6 w 200 200 m 2400 1500 l S 300 1500 m 2300 200 l S\n".to_string(); // WHAT: a different drawing. WHY: not two issues of one sheet.
    let (c, said) = colours_of(
        &[Page::arch_d(plan("S-201", 700.0))],
        &[Page::arch_d(other)],
        &scratch,
    ); // WHAT: lays them over. WHY: the test.
    assert!(
        said.to_lowercase().contains("could not be lined up"),
        "{said}"
    ); // WHAT: it says so. WHY: nobody reads a mess as a revision.
    assert!(c.older > 0 && c.newer > 0, "{c:?}"); // WHAT: both are still drawn. WHY: shown as they are.
}

#[test]
// WHAT: the test. WHY: big sheets are quick enough.
fn a_big_sheet_overlays_in_reasonable_time_and_stays_readable() {
    let scratch = Scratch::new("big"); // WHAT: a folder. WHY: the fixtures.
    let mut busy = String::from("0 0 0 RG 1 w\n"); // WHAT: a busy 48 by 36 sheet. WHY: the biggest sheets are the slow ones.

    // WHAT: fifteen hundred marks. WHY: a busy sheet.
    for i in 0..1500 {
        let x = 100.0 + f64::from(i % 50) * 64.0; // WHAT: a column of the pattern. WHY: line-work all over.
        let y = 100.0 + f64::from(i / 50) * 80.0; // WHAT: its row. WHY: the same.
        busy.push_str(&format!(
            "{x} {y} m {} {} l S {x} {y} 20 12 re S\n",
            x + 40.0,
            y + 30.0
        )); // WHAT: a line and a box. WHY: bolts and members.
    }
    let mut older = Page::arch_d(busy.clone()); // WHAT: the older issue. WHY: the fixture.
    older.media = [0.0, 0.0, 3456.0, 2592.0]; // WHAT: 48 by 36 paper. WHY: ARCH E.
    let mut newer = older.clone(); // WHAT: the newer issue. WHY: the same sheet.
    newer.content.push_str("4 w 1700 1300 m 2400 1300 l S\n"); // WHAT: one new member. WHY: what changed.
    let started = std::time::Instant::now(); // WHAT: the clock. WHY: how long it takes.
    let (c, said) = colours_of(&[older], &[newer], &scratch); // WHAT: lays them over. WHY: the test.
    let took = started.elapsed(); // WHAT: how long. WHY: checked below.
    eprintln!("a 48 by 36 sheet took {took:?}"); // WHAT: says it. WHY: the number goes in the report.
    assert!(took < std::time::Duration::from_secs(120), "{took:?}"); // WHAT: under two minutes even unoptimised and under load. WHY: somebody is waiting at the dialog.
    assert!(c.newer > 200, "the new member shows: {c:?} {said}"); // WHAT: blue is there. WHY: it is the change.
    assert!(
        c.changed() < 0.08,
        "nothing else changed: {c:?} {:.3}",
        c.changed()
    ); // WHAT: the rest is dark. WHY: the same.
}

/// One sheet of each laid over through the single-sheet overlay, as the window runs it.
// WHAT: a one-sheet overlay. WHY: the window's path.
fn overlay_one(
    older: &Page,
    newer: &Page,
    lining: &hyperview::overlay::Lining,
    scratch: &Scratch,
) -> hyperview::render::Overlaid {
    write_pdf(&scratch.at("older.pdf"), std::slice::from_ref(older)); // WHAT: the older issue. WHY: on disk for the engine.
    write_pdf(&scratch.at("newer.pdf"), std::slice::from_ref(newer)); // WHAT: the newer issue. WHY: the same.
    hyperview::render::overlay_two(
        library(),
        &scratch.at("older.pdf"),
        0,
        &scratch.at("newer.pdf"),
        0,
        0.65,
        lining,
    )
    .expect("the overlay should run") // WHAT: lays them over. WHY: the window's own path.
}

/// The colours of an overlay picture straight from the engine.
// WHAT: counts an engine picture. WHY: the answer.
fn colours_in(laid: &hyperview::render::Overlaid) -> Colours {
    let [w, h] = laid.image.size; // WHAT: its size. WHY: copied into an image to count.
    let img = image::RgbaImage::from_fn(
        u32::try_from(w).unwrap_or(0),
        u32::try_from(h).unwrap_or(0),
        |x, y| image::Rgba(laid.image.pixels[y as usize * w + x as usize].to_array()),
    ); // WHAT: the picture as an image. WHY: counted the same way as the files.
    count(&img, None) // WHAT: its colours. WHY: the answer.
}

#[test]
// WHAT: the test. WHY: two points line up what search cannot.
fn a_sheet_moved_too_far_to_find_lines_up_by_two_points() {
    let scratch = Scratch::new("twopoints"); // WHAT: a folder. WHY: the fixtures.
    let older = Page::arch_d(plan("S-201", 700.0)); // WHAT: the older issue. WHY: the reference.
    let mut newer = Page::arch_d(placed("S-201", 700.0, [1.0, 0.0, 0.0, 1.0, 700.0, 300.0])); // WHAT: the same drawing moved 700 across and 300 up. WHY: re-laid on a bigger sheet with a new border.
    newer.media = [0.0, 0.0, 3600.0, 2400.0]; // WHAT: 50 by 33 paper. WHY: room for it.
    let auto = overlay_one(&older, &newer, &hyperview::overlay::Lining::Auto, &scratch); // WHAT: lined up by the drawing. WHY: too far for the search.
    assert!(
        !auto.lined_up && auto.said.contains("could not be lined up"),
        "{}",
        auto.said
    ); // WHAT: says it could not. WHY: never a wrong line-up dressed as right.
       // Two grid crossings, in each sheet's points from the top left: (300, 1728-1530) and (1840, 1728-450) on the older,
       // the same moved 700 across and 300 up on the newer, whose paper is 2400 high.
    let picked = hyperview::overlay::Lining::Points {
        older: [[300.0, 198.0], [1840.0, 1278.0]],
        newer: [
            [1003.0, 2400.0 - 1830.0 - 2.0],
            [2538.0, 2400.0 - 750.0 + 1.0],
        ],
    }; // WHAT: clicked a few points out. WHY: a click is never exact.
    let by_hand = overlay_one(&older, &newer, &picked, &scratch); // WHAT: lined up by the points. WHY: the test.
    let c = colours_in(&by_hand); // WHAT: its colours. WHY: should be dark.
    assert!(
        c.changed() < 0.05,
        "two points line it up: {c:?} {:.3} {}",
        c.changed(),
        by_hand.said
    ); // WHAT: nearly no colour. WHY: the same drawing.
    assert!(by_hand.said.contains("two points"), "{}", by_hand.said); // WHAT: says how. WHY: so the colours are trusted for the right reason.
}

#[test]
// WHAT: the test. WHY: as they are is uncorrected.
fn as_they_are_lays_them_where_the_files_put_them() {
    let scratch = Scratch::new("astheyare"); // WHAT: a folder. WHY: the fixtures.
    let older = Page::arch_d(plan("S-201", 700.0)); // WHAT: the older issue. WHY: the reference.
    let newer = Page::arch_d(placed("S-201", 700.0, [1.0, 0.0, 0.0, 1.0, 14.0, -9.0])); // WHAT: replotted a little over. WHY: a fit would correct it; as they are must not.
    let laid = overlay_one(
        &older,
        &newer,
        &hyperview::overlay::Lining::AsTheyAre,
        &scratch,
    ); // WHAT: not lined up. WHY: asked for.
    assert!(laid.said.contains("Shown as they are"), "{}", laid.said); // WHAT: says so. WHY: the colours are not change.
    assert!(
        colours_in(&laid).changed() > 0.3,
        "left uncorrected, the shift shows"
    ); // WHAT: lots of colour. WHY: nothing was moved.
}

#[test]
// WHAT: the test. WHY: a failed fit moves nothing.
fn sheets_that_do_not_line_up_are_not_moved_by_a_fit_that_did_not_work() {
    let scratch = Scratch::new("notmoved"); // WHAT: a folder. WHY: the fixtures.
    let older = Page::arch_d(plan("S-201", 700.0)); // WHAT: a plan. WHY: one sheet.
    let newer =
        Page::arch_d("0 0 0 RG 6 w 200 200 m 2400 1500 l S 300 1500 m 2300 200 l S\n".to_string()); // WHAT: something else. WHY: does not line up.
    let laid = overlay_one(&older, &newer, &hyperview::overlay::Lining::Auto, &scratch); // WHAT: lays them over. WHY: the test.
    assert!(!laid.lined_up, "{}", laid.said); // WHAT: not lined up. WHY: different sheets.
    let [w, h] = laid.image.size; // WHAT: the overlay's size. WHY: the border is looked for.
    let k = w as f64 / 2592.0; // WHAT: pixels per point. WHY: where 36 points is.
    let row = h / 2; // WHAT: halfway down. WHY: crosses the older border's left side.
    let border = (0..w / 10).find(|x| {
        let p = laid.image.pixels[row * w + x];
        i32::from(p.r()) > i32::from(p.b()) + 60
    }); // WHAT: the first red pixel from the left. WHY: the older border.
    let border = border.expect("the older border is drawn"); // WHAT: found. WHY: shown as it is.
    assert!(
        (border as f64 - 36.0 * k).abs() <= 3.0,
        "the border is where the file has it, at {:.0}, not moved to {border}",
        36.0 * k
    ); // WHAT: unmoved. WHY: "as they are" must be as they are.
}

#[test]
// WHAT: the test. WHY: Save as PDF writes a sheet with a legend.
fn one_overlaid_sheet_saves_as_a_drawing_with_a_legend() {
    let scratch = Scratch::new("saveone"); // WHAT: a folder. WHY: the fixtures.
    let (a, b, to) = (
        scratch.at("Rev 1.pdf"),
        scratch.at("Rev 2.pdf"),
        scratch.at("S-201 overlay.pdf"),
    ); // WHAT: the files. WHY: in, in, out.
    write_pdf(
        &a,
        &[
            Page::arch_d(plan("S-101", 700.0)),
            Page::arch_d(plan("S-201", 700.0)),
        ],
    ); // WHAT: two sheets in the older. WHY: sheet 2 is laid under.
    let mut bigger = Page::arch_d(plan("S-201", 1300.0)); // WHAT: the newer S-201, beam moved. WHY: a revision.
    bigger.media = [0.0, 0.0, 3024.0, 2160.0]; // WHAT: on bigger paper. WHY: the overlay takes the newer sheet's paper.
    write_pdf(&b, &[bigger]); // WHAT: the newer issue. WHY: the same.
    let op = Operation::OverlaySheet {
        newer: b.clone(),
        newer_page: 0,
        older: a.clone(),
        older_page: 1,
        darkness: 0.65,
        lining: hyperview::overlay::Lining::Auto,
        to: to.clone(),
    }; // WHAT: the window's Save as PDF. WHY: the test.
    let done = hyperview::render::do_one(library(), &op).expect("it should save"); // WHAT: runs it. WHY: through pdfium.
    assert_eq!(done.wrote, vec![to.clone()], "{}", done.said); // WHAT: the file it wrote. WHY: opened next.
    assert_eq!(
        hyperview::render::pages_in(library(), &to).expect("it opens"),
        1
    ); // WHAT: one sheet. WHY: one sheet was laid over.
    let doc = pdf::Document::open(&to).expect("it reads"); // WHAT: the file. WHY: its paper size.
    let page = doc.page(0).expect("a sheet"); // WHAT: its one page. WHY: measured.
    assert_eq!(
        doc.page_box(&page),
        [0.0, 0.0, 3024.0, 2160.0],
        "the newer sheet's paper"
    ); // WHAT: the same size. WHY: markups and prints line up with the newer sheet.
    let words = hyperview::render::words_on(library(), &to, 0).expect("its words"); // WHAT: the text on it. WHY: the legend.
    assert!(
        words.contains("Red: Rev 1.pdf") && words.contains("Blue: Rev 2.pdf"),
        "{words}"
    ); // WHAT: says which colour is which file. WHY: a print has no window beside it.
    let c = count(&picture(&to, 0, 1800, 1800, &scratch), None); // WHAT: its colours. WHY: the beam moved.
    assert!(
        c.older > 200 && c.newer > 200 && c.changed() < 0.15,
        "{c:?}"
    ); // WHAT: the move shows and nothing else. WHY: lined up across paper sizes.
    assert!(
        std::fs::read(&a)
            .map(|bytes| bytes.starts_with(b"%PDF"))
            .unwrap_or(false),
        "the older issue is untouched"
    ); // WHAT: still a PDF. WHY: an overlay never writes over an issue.
}

#[test]
// WHAT: the test. WHY: comparing lines up first.
fn comparing_a_turned_half_size_print_finds_only_what_changed() {
    let scratch = Scratch::new("comparefit"); // WHAT: a folder. WHY: the fixtures.
    let mut newer = Page::arch_d(placed("S-201", 1300.0, [0.0, 0.5, -0.5, 0.0, 864.0, 0.0])); // WHAT: half size and turned a quarter, beam moved. WHY: the hardest pair a comparison gets.
    newer.media = [0.0, 0.0, 864.0, 1296.0]; // WHAT: half-size portrait paper. WHY: the same.
    newer.annots = vec![[100.0, 100.0, 300.0, 300.0]]; // WHAT: a takeoff box on it. WHY: markups are not changes.
    write_pdf(
        &scratch.at("older.pdf"),
        &[Page::arch_d(plan("S-201", 700.0))],
    ); // WHAT: the older issue. WHY: on disk.
    write_pdf(&scratch.at("newer.pdf"), &[newer]); // WHAT: the newer. WHY: the same.
    let found = hyperview::render::compare_two(
        library(),
        &scratch.at("older.pdf"),
        0,
        &scratch.at("newer.pdf"),
        0,
        0.65,
    )
    .expect("the comparison should run"); // WHAT: compares them. WHY: the test.
    assert!(!found.could_not_align, "{}", found.said); // WHAT: lined up. WHY: the same sheet.
    assert!(
        !found.changes.is_empty() && found.changes.len() <= 3,
        "the beam, gone and arrived, and nothing else: {:?} {}",
        found.changes,
        found.said
    ); // WHAT: a change or two. WHY: one beam moved.

    // WHAT: each change. WHY: on the newer paper.
    for (_, area) in &found.changes {
        assert!(
            area[2] <= 864.0 + 1.0 && area[3] <= 1296.0 + 1.0,
            "on the newer sheet's own paper: {area:?}"
        ); // WHAT: inside the newer sheet. WHY: the clouds go on it.
    }
}
