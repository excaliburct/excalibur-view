//! A scale set on a sheet is still there after the autosave and after the
//! drawing is opened again, whatever shape the PDF came in.
//!
//! A customer on 0.6.22 set 1/4" = 1'-0" on an architect's mechanical set and
//! saw "Scale Not Set" a few seconds later: the autosave wrote the scale, the
//! pending copy was cleared, and the scale was then read back from the file.
//! Each drawing below is built the way a real producer writes one (`AutoCAD`'s
//! viewports, Bluebeam's compressed saves, a turned sheet, a sync folder
//! rewriting the file), and each must keep its scale through save + reopen.

use std::fmt::Write as _;
use std::path::PathBuf;

use hyperview::sheet::Doc;

/// 1/4" = 1'-0": 72 points on the sheet is four feet.
fn quarter_inch() -> annot::measure::Measure {
    annot::measure::imperial(48.0, "1/4\" = 1'-0\"", 16) // WHAT: the customer's scale. WHY: the one that reverted
}

/// An AutoCAD-style `/Measure` for a viewport, at `ratio` with `c` feet a point.
fn acad_measure(ratio: &str, c: f64) -> String {
    format!("<</Type/Measure/Subtype/RL/R({ratio})/X[<</Type/NumberFormat/U(ft)/C {c}/D 100/FD false>>]/D[<</Type/NumberFormat/U(ft)/C 1/D 100>>]/A[<</Type/NumberFormat/U(sq ft)/C 1/D 100>>]>>") // WHAT: a ruler measure as AutoCAD writes one, every part inline. WHY: the common case off a CAD export
}

/// A 36 x 24 sheet, as `AutoCAD` lays out an Arch D sheet.
const SHEET: &str = "[0 0 2592 1728]"; // WHAT: 36 x 24 inches in points. WHY: a real drawing sheet size

/// A classic PDF of `bodies` (object 1 first), with a cross-reference table.
fn classic(bodies: &[String]) -> Vec<u8> {
    let mut file = b"%PDF-1.6\n%\xe2\xe3\xcf\xd3\n".to_vec(); // WHAT: the header and a binary comment. WHY: as producers write it
    let mut offsets = Vec::new(); // WHAT: where each object starts. WHY: the table lists them
    for (i, body) in bodies.iter().enumerate() {
        offsets.push(file.len()); // WHAT: this object's offset. WHY: for the table
        file.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes()); // WHAT: the object. WHY: numbered from 1
    }
    let table = file.len(); // WHAT: where the table starts. WHY: startxref points at it
    file.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes()); // WHAT: the table head. WHY: object 0 is always free
    for offset in &offsets {
        file.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes()); // WHAT: one row. WHY: the object's place
    }
    file.extend_from_slice(format!("trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{table}\n%%EOF\n", bodies.len() + 1).as_bytes()); // WHAT: the trailer. WHY: the file's entry point
    file
}

/// A compressed PDF, as Bluebeam or Acrobat save one: every object except
/// streams packed into one object stream, found through a cross-reference
/// stream. `loose` are the 1-based numbers kept outside (streams).
fn compressed(bodies: &[String], loose: &[usize]) -> Vec<u8> {
    let mut file = b"%PDF-1.6\n%\xe2\xe3\xcf\xd3\n".to_vec(); // WHAT: the header. WHY: as above
    let n = bodies.len(); // WHAT: how many objects. WHY: the object stream and xref stream come after
    let objstm = n + 1; // WHAT: the object stream's number. WHY: right after the rest
    let xrefstm = n + 2; // WHAT: the cross-reference stream's number. WHY: last
    let mut rows: Vec<(u8, u32, u16)> = vec![(0, 0, 65535); xrefstm + 1]; // WHAT: one row an object, all free to begin with. WHY: filled in below
    for &number in loose {
        rows[number] = (1, u32::try_from(file.len()).unwrap(), 0); // WHAT: a loose object's offset. WHY: type 1
        file.extend_from_slice(format!("{number} 0 obj\n{}\nendobj\n", bodies[number - 1]).as_bytes()); // WHAT: written loose. WHY: streams can't be packed
    }
    let mut head = String::new(); // WHAT: the object stream's "number offset" pairs. WHY: how a reader finds each one
    let mut packed = String::new(); // WHAT: the packed objects themselves. WHY: the stream's body
    let mut index = 0u16; // WHAT: place within the stream. WHY: type 2 rows name it
    for (i, body) in bodies.iter().enumerate() {
        let number = i + 1; // WHAT: the object number. WHY: from 1
        if loose.contains(&number) {
            continue; // WHAT: already written. WHY: loose
        }
        let _ = write!(head, "{number} {} ", packed.len()); // WHAT: its pair. WHY: the head
        packed.push_str(body); // WHAT: its body. WHY: packed
        packed.push('\n'); // WHAT: a separator. WHY: objects must not run together
        rows[number] = (2, u32::try_from(objstm).unwrap(), index); // WHAT: in the object stream at this place. WHY: type 2
        index += 1; // WHAT: the next place. WHY: in order
    }
    let data = format!("{head}{packed}"); // WHAT: the stream's data. WHY: head then bodies
    rows[objstm] = (1, u32::try_from(file.len()).unwrap(), 0); // WHAT: the object stream's own offset. WHY: type 1
    file.extend_from_slice(format!("{objstm} 0 obj\n<</Type/ObjStm/N {index}/First {}/Length {}>>\nstream\n{data}\nendstream\nendobj\n", head.len(), data.len()).as_bytes()); // WHAT: the object stream, unfiltered. WHY: valid, and readable in a failure message
    let start = file.len(); // WHAT: the xref stream's offset. WHY: startxref points at it
    rows[xrefstm] = (1, u32::try_from(start).unwrap(), 0); // WHAT: its own row. WHY: it lists itself
    let mut raw = Vec::new(); // WHAT: the rows as bytes. WHY: /W [1 4 2]
    for (kind, field, generation) in &rows {
        raw.push(*kind); // WHAT: the type. WHY: 1 byte
        raw.extend_from_slice(&field.to_be_bytes()); // WHAT: offset or stream number. WHY: 4 bytes
        raw.extend_from_slice(&generation.to_be_bytes()); // WHAT: generation or index. WHY: 2 bytes
    }
    file.extend_from_slice(format!("{xrefstm} 0 obj\n<</Type/XRef/Size {}/W[1 4 2]/Root 1 0 R/Length {}>>\nstream\n", xrefstm + 1, raw.len()).as_bytes()); // WHAT: the xref stream's dictionary. WHY: no table in this file
    file.extend_from_slice(&raw); // WHAT: the rows. WHY: the stream's data
    file.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{start}\n%%EOF\n").as_bytes()); // WHAT: the end. WHY: points at the xref stream
    file
}

/// The usual three objects with `page_extra` added to the page dictionary,
/// then `more` objects from number 4 on.
fn bodies(page_extra: &str, more: &[String]) -> Vec<String> {
    let mut all = vec![
        "<</Type/Catalog/Pages 2 0 R>>".to_string(), // WHAT: the catalog. WHY: object 1
        "<</Type/Pages/Kids[3 0 R]/Count 1>>".to_string(), // WHAT: the page tree. WHY: object 2
        format!("<</Type/Page/Parent 2 0 R/MediaBox {SHEET}{page_extra}>>"), // WHAT: the sheet. WHY: object 3
    ];
    all.extend(more.iter().cloned()); // WHAT: anything else. WHY: viewports, measures
    all
}

/// Writes `bytes` to a fresh file of the build's own, named for the case.
fn on_disk(case: &str, bytes: &[u8]) -> PathBuf {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()); // WHAT: a fresh name. WHY: tests run side by side
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("scale-{case}-{n}-{}", std::process::id())); // WHAT: under the build's own folder. WHY: never a shared temporary folder
    std::fs::create_dir_all(&dir).unwrap(); // WHAT: made. WHY: the drawing goes in it
    let path = dir.join("M-101 Plumbing.pdf"); // WHAT: the drawing. WHY: named like the customer's
    std::fs::write(&path, bytes).unwrap(); // WHAT: written. WHY: opened from disk, as the program does
    path
}

/// Sets 1/4" on the first sheet, saves the way the autosave does, and checks
/// the scale both in the open drawing and after opening it again. Returns the
/// reopened drawing for any further checks.
fn survives(case: &str, bytes: &[u8]) -> Doc {
    let path = on_disk(case, bytes); // WHAT: the drawing on disk. WHY: the real open path
    let mut doc = Doc::open(path.clone()).unwrap_or_else(|e| panic!("{case}: open: {e}")); // WHAT: opened. WHY: as a person does
    doc.set_scale(0, Some(quarter_inch())); // WHAT: 1/4" set. WHY: what the customer did
    assert_eq!(doc.scale_of(0).map(|m| m.ratio), Some("1/4\" = 1'-0\"".to_string()), "{case}: shown before the save"); // WHAT: shown at once. WHY: pending
    doc.save("Customer").unwrap_or_else(|e| panic!("{case}: save: {e}")); // WHAT: the autosave. WHY: what came a few seconds later
    let after = doc.scale_of(0).unwrap_or_else(|| panic!("{case}: Scale Not Set straight after the save")); // WHAT: read from the file now. WHY: the pending copy is cleared by the save
    assert_eq!(after.ratio, "1/4\" = 1'-0\"", "{case}: the scale after the save"); // WHAT: the same scale. WHY: never another region's
    assert_eq!(after.length(72.0), "4'-0\"", "{case}: measures at 1/4\""); // WHAT: an inch is four feet. WHY: the measurements read right
    let again = Doc::open(path).unwrap_or_else(|e| panic!("{case}: reopen: {e}")); // WHAT: opened again. WHY: the file talking, not memory
    let reopened = again.scale_of(0).unwrap_or_else(|| panic!("{case}: Scale Not Set after reopening")); // WHAT: read back. WHY: the test that matters
    assert_eq!(reopened.ratio, "1/4\" = 1'-0\"", "{case}: the scale after reopening"); // WHAT: the same. WHY: kept
    assert_eq!(reopened.length(72.0), "4'-0\"", "{case}: measures after reopening"); // WHAT: the same. WHY: kept
    again
}

/// The scale at a point of the reopened sheet, as a length of 72 points.
fn inch_at(doc: &Doc, at: [f64; 2]) -> String {
    let sheet = doc.file.page(0).expect("the sheet"); // WHAT: the page. WHY: details are read from it
    annot::viewport::scale_at(&doc.file, &sheet, at).expect("a scale there").length(72.0) // WHAT: one inch measured there. WHY: tells the scales apart
}

#[test]
fn a_plain_sheet_keeps_its_scale() {
    survives("plain", &classic(&bodies("", &[]))); // WHAT: no /VP at all. WHY: the control
}

#[test]
fn an_autocad_sheet_with_its_own_viewports_keeps_the_new_scale_and_its_details() {
    // AutoCAD writes a viewport for each paper-space viewport: the main plan
    // inside the title-block margins (so never quite the whole sheet), and a
    // detail with its own scale. Both carry a /Measure.
    let vp = format!(
        "/VP[<</Type/Viewport/BBox[72 72 2160 1656]/Name(Plan)/Measure {}>> <</Type/Viewport/BBox[2200 900 2520 1656]/Name(Detail 3)/Measure {}>>]",
        acad_measure("1/8\" = 1'-0\"", 0.111_111),
        acad_measure("1\" = 1'-0\"", 0.013_888_9)
    ); // WHAT: a plan at 1/8" inside the margins and a 1" detail. WHY: an architect's set as exported
    let again = survives("autocad", &classic(&bodies(&vp, &[]))); // WHAT: set 1/4", save, reopen. WHY: the sheet-wide one is written last and must still be the one read
    assert_eq!(inch_at(&again, [2300.0, 1000.0]), "1 ft", "the detail keeps its own 1\" scale"); // WHAT: inside the detail. WHY: never overwritten
    assert_eq!(inch_at(&again, [1000.0, 800.0]), "4'-0\"", "the plan measures at the 1/4\" just set, not AutoCAD's 1/8\""); // WHAT: inside the plan viewport. WHY: the plan is the sheet; a measurement there must use the scale the person set
    assert_eq!(annot::viewport::regions(&again.file, &again.file.page(0).unwrap()).len(), 2, "the sheet's scale and the detail, the plan's 1/8\" replaced"); // WHAT: two regions. WHY: never three disagreeing
}

#[test]
fn a_viewport_list_kept_as_its_own_object_keeps_the_scale() {
    let vp = format!("[<</Type/Viewport/BBox[2200 900 2520 1656]/Name(Detail 3)/Measure {}>>]", acad_measure("1\" = 1'-0\"", 0.013_888_9)); // WHAT: the /VP array, as object 4. WHY: an indirect /VP
    let again = survives("indirect-vp", &classic(&bodies("/VP 4 0 R", &[vp]))); // WHAT: set, save, reopen. WHY: the list must be read through its reference
    assert_eq!(inch_at(&again, [2300.0, 1000.0]), "1 ft", "the detail keeps its own scale"); // WHAT: inside the detail. WHY: kept
}

#[test]
fn viewports_and_measures_kept_as_their_own_objects_keep_the_scale() {
    let more = vec![
        "<</Type/Viewport/BBox[2200 900 2520 1656]/Name(Detail 3)/Measure 5 0 R>>".to_string(), // WHAT: the viewport, as object 4. WHY: indirect entries
        "<</Type/Measure/Subtype/RL/R(1\" = 1'-0\")/X[6 0 R]/D[7 0 R]/A[8 0 R]>>".to_string(), // WHAT: its measure, as object 5, its formats by reference. WHY: how some exporters write them
        "<</Type/NumberFormat/U(ft)/C 0.0138889/D 100>>".to_string(), // WHAT: the X format. WHY: by reference
        "<</Type/NumberFormat/U(ft)/C 1/D 100>>".to_string(), // WHAT: the D format. WHY: likewise
        "<</Type/NumberFormat/U(sq ft)/C 1/D 100>>".to_string(), // WHAT: the A format. WHY: likewise
    ];
    let again = survives("indirect-entries", &classic(&bodies("/VP[4 0 R]", &more))); // WHAT: set, save, reopen. WHY: references all the way down
    assert_eq!(inch_at(&again, [2300.0, 1000.0]), "1 ft", "the detail, with its formats by reference, keeps its own scale"); // WHAT: inside the detail. WHY: read through its references
}

#[test]
fn viewports_whose_measure_does_not_read_are_left_alone_and_the_scale_holds() {
    let vp = "/VP[<</Type/Viewport/BBox[72 72 2160 1656]/Name(Plan)/Measure<</Type/Measure/Subtype/GEO/GPTS[0 0 1 1]>>>> <</Type/Viewport/BBox[2200 900 2520 1656]/Measure<</Type/Measure/Subtype/RL/R(odd)>>>>]"; // WHAT: a geospatial measure and one with no X. WHY: neither is a ruler scale
    survives("odd-measures", &classic(&bodies(vp, &[]))); // WHAT: set, save, reopen. WHY: an unreadable viewport must not hide the new scale
}

#[test]
fn a_turned_sheet_and_one_with_its_own_user_unit_keep_the_scale() {
    survives("rotated", &classic(&bodies("/Rotate 90", &[]))); // WHAT: a sheet stored on its side. WHY: common in scanned and plotted sets
    survives("user-unit", &classic(&bodies("/UserUnit 2", &[]))); // WHAT: a sheet in bigger units. WHY: some large-format producers
    survives("cropped", &classic(&bodies("/CropBox[18 18 2574 1710]/Rotate 270", &[]))); // WHAT: cropped and turned. WHY: the sheet box isn't the media box
}

#[test]
fn a_compressed_save_from_bluebeam_keeps_the_scale() {
    survives("objstm", &compressed(&bodies("", &[]), &[])); // WHAT: every object packed, an xref stream. WHY: Bluebeam and Acrobat save like this
    let vp = format!("/VP[<</Type/Viewport/BBox[2200 900 2520 1656]/Name(Detail 3)/Measure {}>>]", acad_measure("1\" = 1'-0\"", 0.013_888_9)); // WHAT: a detail. WHY: the page in the object stream carries viewports too
    let again = survives("objstm-vp", &compressed(&bodies(&vp, &[]), &[])); // WHAT: set, save, reopen. WHY: the page is rewritten out of the object stream
    assert_eq!(inch_at(&again, [2300.0, 1000.0]), "1 ft", "the detail keeps its scale"); // WHAT: inside the detail. WHY: kept
}

#[test]
fn a_scale_survives_a_second_save_and_a_markup_added_after() {
    let path = on_disk("twice", &compressed(&bodies("", &[]), &[])); // WHAT: a compressed drawing. WHY: two incremental saves on an xref stream
    let mut doc = Doc::open(path.clone()).unwrap(); // WHAT: opened. WHY: as a person does
    doc.set_scale(0, Some(quarter_inch())); // WHAT: 1/4". WHY: the customer's
    doc.save("Customer").unwrap(); // WHAT: the first autosave. WHY: the scale written
    let draft = hyperview::sheet::Draft { tool: hyperview::app::Tool::Length, points: vec![[100.0, 100.0], [820.0, 100.0]], ..hyperview::sheet::Draft::default() }; // WHAT: a ten-inch line. WHY: a pipe run taken off after the scale
    doc.place(0, draft).expect("placed"); // WHAT: on the sheet. WHY: the next autosave writes it
    doc.save("Customer").unwrap(); // WHAT: the second autosave. WHY: must not lose the scale
    let again = Doc::open(path).unwrap(); // WHAT: reopened. WHY: the file talking
    assert_eq!(again.scale_of(0).map(|m| m.ratio), Some("1/4\" = 1'-0\"".to_string()), "the scale after two saves"); // WHAT: kept. WHY: the bug
    assert!(again.marks.iter().any(|m| m.markup.contents() == "40'-0\""), "the run reads 40'-0\" at 1/4\""); // WHAT: the measurement at the scale. WHY: shows the same, not differently
}

#[test]
fn a_file_rewritten_by_a_sync_folder_before_the_autosave_keeps_the_scale() {
    let path = on_disk("synced", &classic(&bodies("", &[]))); // WHAT: a drawing in a synced folder. WHY: OneDrive, Dropbox, Egnyte
    let mut doc = Doc::open(path.clone()).unwrap(); // WHAT: opened. WHY: as a person does
    doc.set_scale(0, Some(quarter_inch())); // WHAT: 1/4". WHY: the customer's
    std::thread::sleep(std::time::Duration::from_millis(1100)); // WHAT: a moment passes. WHY: the file's stamp must change
    std::fs::write(&path, compressed(&bodies("", &[]), &[])).unwrap(); // WHAT: the same drawing, rewritten whole and compressed by the sync client. WHY: catch_up runs at the save
    doc.save("Customer").unwrap(); // WHAT: the autosave. WHY: brings theirs in, then writes ours
    assert_eq!(doc.scale_of(0).map(|m| m.ratio), Some("1/4\" = 1'-0\"".to_string()), "the scale after catching up"); // WHAT: kept. WHY: the sync must not cost it
    let again = Doc::open(path).unwrap(); // WHAT: reopened. WHY: the file talking
    assert_eq!(again.scale_of(0).map(|m| m.ratio), Some("1/4\" = 1'-0\"".to_string()), "the scale after reopening"); // WHAT: kept. WHY: likewise
}

#[test]
fn an_architects_set_locked_with_an_owner_password_keeps_the_scale() {
    let vp = format!("/VP[<</Type/Viewport/BBox[2200 900 2520 1656]/Name(Detail 3)/Measure {}>>]", acad_measure("1\" = 1'-0\"", 0.013_888_9)); // WHAT: a detail. WHY: its strings are locked too
    let plain = pdf::Document::from_bytes(classic(&bodies(&vp, &[]))); // WHAT: the set before it was locked. WHY: locked below
    let locked = pdf::crypt::lock("", "architect", pdf::crypt::Allowed::from_bits(-4), [7u8; 32], [9u8; 32]); // WHAT: AES-256, no password to open, an owner password. WHY: how architects send sets out
    let again = survives("encrypted", &pdf::crypt::write_locked(&plain, &locked, [3u8; 16])); // WHAT: set, save, reopen. WHY: the update is sealed and read back unsealed
    assert_eq!(inch_at(&again, [2300.0, 1000.0]), "1 ft", "the detail keeps its scale"); // WHAT: inside the detail. WHY: kept
}
