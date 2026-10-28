//! The Document menu, run for real against a real drawing set.
//!
//! Every one of these opens actual files through pdfium, does the operation,
//! and then opens what came out and counts what is in it. Testing that the
//! message was sent proves nothing: the failures that matter here are a set
//! that comes out with the wrong sheets in it, or — much worse — an original
//! that got written over.

use std::path::{Path, PathBuf};

use hyperview::docops::{self, Operation};

/// A scratch directory that cleans up after itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(what: &str) -> Scratch {
        let n: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("hyperview-{what}-{n:08x}"));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Scratch(path)
    }
    fn at(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The folder pdfium's library is in, for a test binary that is not beside one.
/// The loader adds the platform's own file name, so this is a directory.
fn library() -> Option<PathBuf> {
    for guess in [
        "third_party/pdfium/linux-x64/lib",
        "../third_party/pdfium/linux-x64/lib",
        "../../third_party/pdfium/linux-x64/lib",
    ] {
        let path = PathBuf::from(guess);
        if path.join("libpdfium.so").exists() {
            return Some(path);
        }
    }
    None
}

/// A drawing set with `sheets` numbered sheets, written by hand so the tests do
/// not depend on a file somebody has to supply.
fn a_set(to: &Path, sheets: usize, numbers: &[&str]) {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets: Vec<usize> = Vec::new();
    let add = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &str| {
        offsets.push(out.len());
        out.extend_from_slice(body.as_bytes());
    };

    let kids: String = (0..sheets)
        .map(|i| format!("{} 0 R ", 3 + i * 2))
        .collect();
    add(&mut out, &mut offsets, "1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n");
    add(
        &mut out,
        &mut offsets,
        &format!("2 0 obj\n<</Type/Pages/Kids[{kids}]/Count {sheets}>>\nendobj\n"),
    );
    for i in 0..sheets {
        let number = numbers.get(i).copied().unwrap_or("");
        // A sheet, laid out the way a sheet is: body text all over it, a title
        // block in the bottom right, the title above the number, and the number
        // itself far larger than anything near it. The reader finds a sheet
        // number by being *bigger than the page's body text and in the corner*,
        // so a test page with nothing on it but the number would be read as
        // having none — correctly, and uselessly for a test.
        let mut stream = String::new();
        for row in 0..24 {
            stream.push_str(&format!(
                "BT /F1 11 Tf 200 {} Td (GENERAL NOTE {row} — SEE SPECIFICATIONS) Tj ET\n",
                1900 - row * 60
            ));
        }
        stream.push_str("BT /F1 14 Tf 2600 210 Td (FRAMING PLAN) Tj ET\n");
        stream.push_str(&format!("BT /F1 48 Tf 2600 90 Td ({number}) Tj ET\n"));
        add(
            &mut out,
            &mut offsets,
            &format!(
                "{} 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]\
                 /Resources<</Font<</F1 <</Type/Font/Subtype/Type1/BaseFont/Helvetica>> >> >>\
                 /Contents {} 0 R>>\nendobj\n",
                3 + i * 2,
                4 + i * 2
            ),
        );
        add(
            &mut out,
            &mut offsets,
            &format!(
                "{} 0 obj\n<</Length {}>>\nstream\n{stream}endstream\nendobj\n",
                4 + i * 2,
                stream.len()
            ),
        );
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    std::fs::write(to, out).expect("a drawing set");
}

/// How many sheets are in a file, asked of pdfium rather than guessed.
fn sheets_in(path: &Path) -> usize {
    hyperview::render::pages_in(library(), path).expect("that file should open")
}

fn run(op: &Operation) -> docops::Done {
    hyperview::render::do_one(library(), op).expect("the operation should work")
}

#[test]
fn combining_two_sets_gives_one_with_both_lots_of_sheets() {
    let scratch = Scratch::new("combine");
    let one = scratch.at("One.pdf");
    let two = scratch.at("Two.pdf");
    a_set(&one, 3, &["S-100", "S-101", "S-102"]);
    a_set(&two, 2, &["S-200", "S-201"]);
    let both = scratch.at("Both.pdf");

    let done = run(&Operation::Combine {
        sources: vec![one.clone(), two.clone()],
        to: both.clone(),
    });
    assert_eq!(done.wrote, vec![both.clone()]);
    assert_eq!(sheets_in(&both), 5);
    // And neither original changed.
    assert_eq!(sheets_in(&one), 3);
    assert_eq!(sheets_in(&two), 2);
}

#[test]
fn extracting_takes_the_sheets_asked_for_and_leaves_the_set_alone() {
    let scratch = Scratch::new("extract");
    let set = scratch.at("Set.pdf");
    a_set(&set, 6, &["S-100", "S-101", "S-102", "S-200", "S-201", "S-300"]);
    let out = scratch.at("Some.pdf");

    run(&Operation::Extract {
        source: set.clone(),
        pages: vec![1, 3, 4],
        to: out.clone(),
        and_remove: false,
    });
    assert_eq!(sheets_in(&out), 3);
    assert_eq!(sheets_in(&set), 6, "the set must not have shrunk");
}

#[test]
fn extracting_with_the_remainder_writes_two_files_and_still_leaves_the_original() {
    let scratch = Scratch::new("extractrest");
    let set = scratch.at("Set.pdf");
    a_set(&set, 5, &["S-100", "S-101", "S-102", "S-103", "S-104"]);
    let out = scratch.at("Two.pdf");

    let done = run(&Operation::Extract {
        source: set.clone(),
        pages: vec![0, 1],
        to: out.clone(),
        and_remove: true,
    });
    assert_eq!(done.wrote.len(), 2, "both halves");
    assert_eq!(sheets_in(&done.wrote[0]), 2);
    assert_eq!(sheets_in(&done.wrote[1]), 3);
    // The point of the whole design: "move" still does not touch the source.
    assert_eq!(sheets_in(&set), 5);
    assert!(done.said.contains("untouched"));
}

#[test]
fn deleting_leaves_a_copy_without_those_sheets() {
    let scratch = Scratch::new("delete");
    let set = scratch.at("Set.pdf");
    a_set(&set, 4, &["S-100", "S-101", "S-102", "S-103"]);
    let out = scratch.at("Trimmed.pdf");

    run(&Operation::Delete {
        source: set.clone(),
        pages: vec![1, 2],
        to: out.clone(),
    });
    assert_eq!(sheets_in(&out), 2);
    assert_eq!(sheets_in(&set), 4);
}

#[test]
fn deleting_every_sheet_is_refused_rather_than_making_an_empty_file() {
    let scratch = Scratch::new("deleteall");
    let set = scratch.at("Set.pdf");
    a_set(&set, 2, &["S-100", "S-101"]);
    let refused = hyperview::render::do_one(
        library(),
        &Operation::Delete {
            source: set.clone(),
            pages: vec![0, 1],
            to: scratch.at("Nothing.pdf"),
        },
    );
    assert!(refused.is_err());
    assert!(!scratch.at("Nothing.pdf").exists());
}

#[test]
fn inserting_puts_the_other_file_where_it_was_asked_to() {
    let scratch = Scratch::new("insert");
    let set = scratch.at("Set.pdf");
    let extra = scratch.at("Extra.pdf");
    a_set(&set, 4, &["S-100", "S-101", "S-102", "S-103"]);
    a_set(&extra, 2, &["X-1", "X-2"]);
    let out = scratch.at("With.pdf");

    run(&Operation::Insert {
        source: set.clone(),
        insert: extra.clone(),
        at: 2,
        to: out.clone(),
    });
    assert_eq!(sheets_in(&out), 6);
    assert_eq!(sheets_in(&set), 4);
}

#[test]
fn splitting_makes_one_file_per_run_and_they_add_up() {
    let scratch = Scratch::new("split");
    let set = scratch.at("Set.pdf");
    a_set(&set, 5, &["S-100", "S-101", "S-102", "S-103", "S-104"]);

    let done = run(&Operation::Split {
        source: set.clone(),
        into: scratch.0.clone(),
        every: 2,
    });
    assert_eq!(done.wrote.len(), 3, "2 + 2 + 1");
    let total: usize = done.wrote.iter().map(|p| sheets_in(p)).sum();
    assert_eq!(total, 5, "no sheet lost and none duplicated");
    assert_eq!(sheets_in(&set), 5);
}

#[test]
fn rotating_writes_a_turned_copy_and_leaves_the_original_the_right_way_up() {
    let scratch = Scratch::new("rotate");
    let set = scratch.at("Set.pdf");
    a_set(&set, 3, &["S-100", "S-101", "S-102"]);
    let out = scratch.at("Turned.pdf");

    let done = run(&Operation::Rotate {
        source: set.clone(),
        pages: vec![0, 2],
        quarter_turns: 1,
        to: out.clone(),
    });
    assert!(done.said.contains("2 sheets turned"));
    assert_eq!(sheets_in(&out), 3, "turning does not remove sheets");
    assert_eq!(sheets_in(&set), 3);
}

#[test]
fn a_revision_slips_in_over_the_sheet_with_the_same_number() {
    let scratch = Scratch::new("slip");
    let set = scratch.at("Set.pdf");
    let revisions = scratch.at("Rev 2.pdf");
    a_set(&set, 4, &["S-100", "S-201", "S-202", "S-300"]);
    a_set(&revisions, 1, &["S-202"]);
    let out = scratch.at("Slipped.pdf");

    let done = run(&Operation::SlipSheet {
        source: set.clone(),
        revisions: revisions.clone(),
        to: out.clone(),
        keep_superseded: false,
    });
    // Same number of sheets: one swapped, not one added.
    assert_eq!(sheets_in(&out), 4, "{}", done.said);
    assert_eq!(sheets_in(&set), 4);
    // And it really matched, rather than quietly doing nothing and leaving a
    // count that happens to be right.
    assert!(done.said.starts_with("1 sheet replaced"), "{}", done.said);
}

#[test]
fn keeping_the_superseded_sheet_puts_it_at_the_back() {
    let scratch = Scratch::new("slipkeep");
    let set = scratch.at("Set.pdf");
    let revisions = scratch.at("Rev 2.pdf");
    a_set(&set, 3, &["S-100", "S-201", "S-300"]);
    a_set(&revisions, 1, &["S-201"]);
    let out = scratch.at("Slipped.pdf");

    run(&Operation::SlipSheet {
        source: set.clone(),
        revisions: revisions.clone(),
        to: out.clone(),
        keep_superseded: true,
    });
    assert_eq!(sheets_in(&out), 4, "three, plus the one that was replaced");
}

#[test]
fn a_revision_that_matches_nothing_is_reported_and_not_appended() {
    let scratch = Scratch::new("slipmiss");
    let set = scratch.at("Set.pdf");
    let revisions = scratch.at("Rev 2.pdf");
    a_set(&set, 2, &["S-100", "S-201"]);
    a_set(&revisions, 1, &["S-999"]);
    let out = scratch.at("Slipped.pdf");

    let done = run(&Operation::SlipSheet {
        source: set.clone(),
        revisions: revisions.clone(),
        to: out.clone(),
        keep_superseded: false,
    });
    // Two sheets, not three: an unmatched revision is a typo to look at, not a
    // sheet to bolt on the end of somebody's bid set.
    assert_eq!(sheets_in(&out), 2);
    assert!(done.said.contains("matched no sheet"), "{}", done.said);
}

#[test]
fn a_name_that_is_taken_gets_a_number_rather_than_overwriting_a_drawing() {
    let scratch = Scratch::new("collide");
    let set = scratch.at("Set.pdf");
    a_set(&set, 2, &["S-100", "S-101"]);

    let taken = scratch.at("Out.pdf");
    std::fs::write(&taken, b"somebody's work").expect("the file in the way");

    let fresh = docops::free_name(&taken);
    run(&Operation::Extract {
        source: set.clone(),
        pages: vec![0],
        to: fresh.clone(),
        and_remove: false,
    });
    assert_ne!(fresh, taken);
    assert_eq!(
        std::fs::read(&taken).unwrap(),
        b"somebody's work",
        "the file that was already there must be exactly as it was"
    );
}

#[test]
fn asking_for_sheets_that_are_not_there_is_refused_rather_than_guessed_at() {
    let scratch = Scratch::new("outofrange");
    let set = scratch.at("Set.pdf");
    a_set(&set, 2, &["S-100", "S-101"]);
    let refused = hyperview::render::do_one(
        library(),
        &Operation::Extract {
            source: set,
            pages: vec![50, 60],
            to: scratch.at("Nothing.pdf"),
            and_remove: false,
        },
    );
    assert!(refused.is_err());
}

#[test]
fn something_that_is_not_a_pdf_says_so_in_words() {
    let scratch = Scratch::new("notapdf");
    let fake = scratch.at("Set.pdf");
    std::fs::write(&fake, b"this is not a pdf at all").unwrap();
    let refused = hyperview::render::do_one(
        library(),
        &Operation::Delete {
            source: fake,
            pages: vec![0],
            to: scratch.at("Out.pdf"),
        },
    );
    let why = refused.expect_err("that is not a PDF");
    assert!(why.contains("not a PDF") || why.contains("damaged"), "{why}");
}

// ---- comparing two issues of a sheet ---------------------------------------

/// A sheet with a title block and whatever extra line-work is asked for, so two
/// issues of "the same sheet" can be made that differ in a known way.
fn a_sheet_with(to: &Path, extra: &str) {
    let body: String = (0..20)
        .map(|row| {
            format!(
                "BT /F1 11 Tf 200 {} Td (GENERAL NOTE {row} — SEE SPECIFICATIONS) Tj ET\n",
                1900 - row * 60
            )
        })
        .collect();
    // A frame, so there is line-work rather than only text.
    let frame = "4 w 150 150 m 2874 150 l 2874 2010 l 150 2010 l h S\n";
    let stream = format!("{frame}{body}BT /F1 48 Tf 2600 90 Td (S-201) Tj ET\n{extra}");

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets: Vec<usize> = Vec::new();
    let add = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &str| {
        offsets.push(out.len());
        out.extend_from_slice(body.as_bytes());
    };
    add(&mut out, &mut offsets, "1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n");
    add(&mut out, &mut offsets, "2 0 obj\n<</Type/Pages/Kids[3 0 R]/Count 1>>\nendobj\n");
    add(
        &mut out,
        &mut offsets,
        "3 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]\
         /Resources<</Font<</F1 <</Type/Font/Subtype/Type1/BaseFont/Helvetica>> >> >>\
         /Contents 4 0 R>>\nendobj\n",
    );
    add(
        &mut out,
        &mut offsets,
        &format!("4 0 obj\n<</Length {}>>\nstream\n{stream}endstream\nendobj\n", stream.len()),
    );
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    std::fs::write(to, out).expect("a sheet");
}

fn compared(older: &Path, newer: &Path) -> hyperview::render::Compared {
    hyperview::render::compare_two(library(), older, 0, newer, 0, 0.65)
        .expect("the comparison should run")
}

#[test]
fn two_issues_with_nothing_changed_report_nothing() {
    let scratch = Scratch::new("samesheet");
    let one = scratch.at("Rev 1.pdf");
    let two = scratch.at("Rev 2.pdf");
    a_sheet_with(&one, "");
    a_sheet_with(&two, "");

    let found = compared(&one, &two);
    assert!(!found.could_not_align, "{}", found.said);
    assert!(found.changes.is_empty(), "{:?}", found.changes);
    assert!(found.said.contains("Nothing changed"), "{}", found.said);
}

#[test]
fn a_detail_added_on_the_revision_is_found_and_pointed_at() {
    let scratch = Scratch::new("added");
    let one = scratch.at("Rev 1.pdf");
    let two = scratch.at("Rev 2.pdf");
    a_sheet_with(&one, "");
    // A filled box in a part of the sheet nothing else occupies.
    a_sheet_with(&two, "0 0 0 rg 900 900 300 240 re f\n");

    let found = compared(&one, &two);
    assert!(!found.could_not_align, "{}", found.said);
    assert!(!found.changes.is_empty(), "something was added: {}", found.said);
    // And the change is where the box is, in the sheet's own points.
    let near = found.changes.iter().any(|(_, area)| {
        let cx = (area[0] + area[2]) * 0.5;
        let cy = (area[1] + area[3]) * 0.5;
        // PDF y is up from the bottom; the comparison works in image space, so
        // the box drawn at y 900-1140 is about 1020 from the top of a 2160 page.
        (700.0..1400.0).contains(&cx) && (800.0..1400.0).contains(&cy)
    });
    assert!(near, "the change should be where the box is: {:?}", found.changes);
}

#[test]
fn a_comparison_of_two_different_sheets_marks_nothing() {
    // Not "the same sheet with something added" — a genuinely different
    // drawing, which is what somebody has picked when they choose the wrong
    // file. Clouding the whole page would look like an answer and be none.
    let scratch = Scratch::new("different");
    let one = scratch.at("S-201.pdf");
    let two = scratch.at("Other.pdf");
    a_sheet_with(&one, "");
    a_different_sheet(&two);

    let found = compared(&one, &two);
    assert!(found.could_not_align, "{}", found.said);
    assert!(found.changes.is_empty());
    assert!(found.said.contains("could not be lined up"));
}

/// A sheet with nothing in common with [`a_sheet_with`]: different frame,
/// different text, in different places.
fn a_different_sheet(to: &Path) {
    let body: String = (0..20)
        .map(|row| {
            format!(
                "BT /F1 40 Tf 1500 {} Td (SCHEDULE ROW {row} 00 00 00) Tj ET\n",
                300 + row * 80
            )
        })
        .collect();
    let stream = format!("12 w 900 300 m 2600 300 l 2600 1900 l 900 1900 l h S\n{body}");

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets: Vec<usize> = Vec::new();
    let add = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &str| {
        offsets.push(out.len());
        out.extend_from_slice(body.as_bytes());
    };
    add(&mut out, &mut offsets, "1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n");
    add(&mut out, &mut offsets, "2 0 obj\n<</Type/Pages/Kids[3 0 R]/Count 1>>\nendobj\n");
    add(
        &mut out,
        &mut offsets,
        "3 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]\
         /Resources<</Font<</F1 <</Type/Font/Subtype/Type1/BaseFont/Helvetica>> >> >>\
         /Contents 4 0 R>>\nendobj\n",
    );
    add(
        &mut out,
        &mut offsets,
        &format!("4 0 obj\n<</Length {}>>\nstream\n{stream}endstream\nendobj\n", stream.len()),
    );
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    std::fs::write(to, out).expect("a sheet");
}

#[test]
fn a_comparison_never_reports_a_quantity() {
    let scratch = Scratch::new("noquantity");
    let one = scratch.at("Rev 1.pdf");
    let two = scratch.at("Rev 2.pdf");
    a_sheet_with(&one, "");
    a_sheet_with(&two, "0 0 0 rg 900 900 300 240 re f\n");

    let said = compared(&one, &two).said;
    for measurement in ["sq ft", "square feet", "lb", "tons", "linear"] {
        assert!(!said.contains(measurement), "{said}");
    }
}

// ---- stamping and shrinking ------------------------------------------------

#[test]
fn a_footer_lands_on_every_sheet_and_the_original_keeps_none() {
    let scratch = Scratch::new("stamp");
    let set = scratch.at("Set.pdf");
    a_set(&set, 3, &["S-100", "S-101", "S-102"]);
    let out = scratch.at("Stamped.pdf");

    let done = run(&Operation::Stamp {
        source: set.clone(),
        pages: vec![0, 1, 2],
        stamps: vec![hyperview::stamps::Stamp {
            spot: hyperview::stamps::Spot::BottomRight,
            text: "EXAMPLE STEEL 24-118 · sheet <n> of <total>".into(),
            ..Default::default()
        }],
        to: out.clone(),
    });
    assert!(done.said.contains("3 sheets stamped"), "{}", done.said);
    assert_eq!(sheets_in(&out), 3);
    assert_eq!(sheets_in(&set), 3);

    // The stamped file really has the text on it, and the original does not.
    let stamped_text = hyperview::render::compare_two(library(), &set, 0, &out, 0, 0.65)
        .expect("comparable");
    assert!(
        !stamped_text.changes.is_empty(),
        "a stamp should show up as a difference: {}",
        stamped_text.said
    );
}

#[test]
fn stamping_nothing_is_refused_rather_than_writing_an_identical_copy() {
    let scratch = Scratch::new("stampnothing");
    let set = scratch.at("Set.pdf");
    a_set(&set, 2, &["S-100", "S-101"]);
    let refused = hyperview::render::do_one(
        library(),
        &Operation::Stamp {
            source: set,
            pages: vec![0, 1],
            stamps: vec![hyperview::stamps::Stamp {
                text: "   ".into(),
                ..Default::default()
            }],
            to: scratch.at("Out.pdf"),
        },
    );
    assert!(refused.is_err());
}

#[test]
fn shrinking_makes_a_file_of_pictures_and_says_what_that_cost() {
    let scratch = Scratch::new("shrink");
    let set = scratch.at("Set.pdf");
    a_set(&set, 2, &["S-100", "S-101"]);
    let out = scratch.at("Small.pdf");

    let done = run(&Operation::Shrink {
        source: set.clone(),
        to: out.clone(),
        dpi: 100,
    });
    assert_eq!(sheets_in(&out), 2);
    assert_eq!(sheets_in(&set), 2);
    // It says the thing somebody needs to know before sending it on.
    assert!(done.said.contains("no text left to search"), "{}", done.said);
}

#[test]
fn shrinking_warns_before_it_runs_and_the_others_do_not() {
    let shrink = Operation::Shrink {
        source: "a.pdf".into(),
        to: "b.pdf".into(),
        dpi: 150,
    };
    let warning = shrink.needs_a_warning().expect("this one warns");
    assert!(warning.contains("no text left to search"));
    let stamp = Operation::Stamp {
        source: "a.pdf".into(),
        pages: vec![0],
        stamps: Vec::new(),
        to: "b.pdf".into(),
    };
    assert!(stamp.needs_a_warning().is_none());
}

// ---- batches ---------------------------------------------------------------

#[test]
fn one_bad_file_in_a_batch_does_not_stop_the_rest() {
    // The whole reason a batch is not a loop. Forty files where the fourteenth
    // is unreadable should give thirty-nine results and one named failure.
    let scratch = Scratch::new("batchbad");
    let good_one = scratch.at("A.pdf");
    let bad = scratch.at("B.pdf");
    let good_two = scratch.at("C.pdf");
    a_set(&good_one, 2, &["S-100", "S-101"]);
    std::fs::write(&bad, b"not a pdf at all").unwrap();
    a_set(&good_two, 3, &["S-200", "S-201", "S-202"]);

    let report = hyperview::render::batch_over(
        library(),
        &hyperview::batch::Job::Rotate {
            quarter_turns: 1,
            into: Some(scratch.0.clone()),
        },
        &[good_one.clone(), bad.clone(), good_two.clone()],
    )
    .expect("the batch should run");

    assert_eq!(report.lines.len(), 3, "every file gets a line");
    assert_eq!(report.done(), 2);
    assert_eq!(report.failed(), 1);
    // The failure names the file and says why.
    let text = report.as_text();
    assert!(text.contains("B.pdf"));
    assert!(text.contains("NOT DONE"));
    assert!(text.contains("not a PDF"), "{text}");
    // And the two good ones really were written.
    assert_eq!(report.wrote().len(), 2);
    for wrote in report.wrote() {
        assert!(wrote.exists(), "{}", wrote.display());
    }
}

#[test]
fn a_batch_combine_puts_them_all_in_one_file_in_order() {
    let scratch = Scratch::new("batchcombine");
    let one = scratch.at("A.pdf");
    let two = scratch.at("B.pdf");
    a_set(&one, 2, &["S-100", "S-101"]);
    a_set(&two, 3, &["S-200", "S-201", "S-202"]);
    let all = scratch.at("All.pdf");

    let report = hyperview::render::batch_over(
        library(),
        &hyperview::batch::Job::CombineAll { to: all.clone() },
        &[one.clone(), two.clone()],
    )
    .expect("the batch should run");
    assert_eq!(report.failed(), 0, "{}", report.as_text());
    assert_eq!(sheets_in(&all), 5);
    assert_eq!(sheets_in(&one), 2, "originals untouched");
    assert_eq!(sheets_in(&two), 3);

    // One bookmark per file, named after it, opening at its first sheet.
    let combined = pdf::Document::open(&all).unwrap();
    let marks = hyperview::morepanels::bookmarks_of(&combined);
    let titles: Vec<&str> = marks.iter().map(|b| b.title.as_str()).collect();
    assert_eq!(titles, ["A", "B"]);
    assert_eq!(marks[1].page, Some(2), "B starts on the third sheet");
}

#[test]
fn combining_files_added_a_few_at_a_time_from_two_folders_works_with_no_name_typed() {
    // WHAT: what Combine does on a Linux desktop, end to end. WHY: there the files arrive through Add files, a folder, the open drawings or a drop, never from Explorer, and nobody had typed where the result goes.
    let scratch = Scratch::new("combinelinux"); // WHAT: a folder of its own. WHY: cleaned up after.
    let first = scratch.at("Job 6742"); // WHAT: one folder, with a space in its name. WHY: as real job folders are.
    let second = scratch.at("revisions"); // WHAT: another folder. WHY: files from two places in one set.
    std::fs::create_dir_all(&first).unwrap(); // WHAT: made. WHY: the sets go in them.
    std::fs::create_dir_all(&second).unwrap(); // WHAT: made. WHY: as above.
    let a = first.join("S-100 FRAMING.PDF"); // WHAT: an upper-case extension. WHY: how plotters and scanners name files, and what Linux's file window used to hide.
    let b = second.join("s-200 rev 1.pdf"); // WHAT: an ordinary one. WHY: the two kinds side by side.
    a_set(&a, 2, &["S-100", "S-101"]); // WHAT: two sheets. WHY: something to count.
    a_set(&b, 1, &["S-200"]); // WHAT: one sheet. WHY: as above.

    let mut batching = hyperview::batchui::Batching::new(hyperview::batchui::Which::Combine); // WHAT: the Combine window, fresh. WHY: as somebody opens it.
    assert!(batching.job().is_none(), "nothing to combine yet"); // WHAT: the button greyed. WHY: no files.
    hyperview::batchui::add_to_list(&mut batching, vec![a.clone()]); // WHAT: the first file added on its own. WHY: a file window that picks one at a time.
    hyperview::batchui::add_to_list(&mut batching, vec![b.clone(), a.clone()]); // WHAT: then the second, with the first again. WHY: added a few at a time, each once.
    assert_eq!(batching.files, vec![a.clone(), b.clone()], "each file once, in the order added"); // WHAT: the list. WHY: the order is the combined set's order.
    let job = batching.job().expect("Combine can start without anybody typing where it goes"); // WHAT: the button live. WHY: it used to stay grey until a path was typed.
    let hyperview::batch::Job::CombineAll { to } = &job else { panic!("a combine") }; // WHAT: where it goes. WHY: checked next.
    assert_eq!(to.parent(), Some(first.as_path()), "beside the first file"); // WHAT: Combined.pdf in the first file's folder. WHY: where somebody looks for it.

    let report = hyperview::render::batch_over(library(), &job, &batching.files).expect("the batch should run"); // WHAT: combined for real. WHY: through pdfium, as the window does it.
    assert_eq!(report.failed(), 0, "{}", report.as_text()); // WHAT: nothing failed. WHY: Combine works on Linux.
    assert_eq!(sheets_in(to), 3, "every sheet from both folders"); // WHAT: two and one. WHY: nothing lost.
}

#[test]
fn a_batch_writes_beside_each_file_when_no_folder_is_given() {
    let scratch = Scratch::new("batchbeside");
    let one = scratch.at("A.pdf");
    a_set(&one, 2, &["S-100", "S-101"]);

    let report = hyperview::render::batch_over(
        library(),
        &hyperview::batch::Job::Flatten { into: None },
        &[one.clone()],
    )
    .expect("the batch should run");
    assert_eq!(report.done(), 1, "{}", report.as_text());
    let wrote = report.wrote();
    assert_eq!(wrote.len(), 1);
    assert_eq!(wrote[0].parent(), one.parent(), "beside the original");
    assert_ne!(*wrote[0], one, "never over the original");
}

// ---- reading a scanned sheet -----------------------------------------------

/// A sheet with no text layer at all — a picture of a drawing, which is what a
/// scan is. Drawn as thick strokes forming readable characters so an OCR engine
/// has something real to read.
fn a_scanned_sheet(to: &Path, word: &str) {
    // Drawn, not typed: the point is that there is no text object on the page.
    // Each character is built from filled rectangles, big and blocky, which is
    // what a scan of large title-block lettering looks like to a recogniser.
    let mut ink = String::from("0 0 0 rg\n");
    let mut x = 300.0f32;
    for c in word.chars() {
        for (rx, ry, rw, rh) in strokes_for(c) {
            ink.push_str(&format!(
                "{} {} {} {} re f\n",
                x + rx * 100.0,
                300.0 + ry * 100.0,
                rw * 100.0,
                rh * 100.0
            ));
        }
        x += 130.0;
    }

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets: Vec<usize> = Vec::new();
    let add = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &str| {
        offsets.push(out.len());
        out.extend_from_slice(body.as_bytes());
    };
    add(&mut out, &mut offsets, "1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n");
    add(&mut out, &mut offsets, "2 0 obj\n<</Type/Pages/Kids[3 0 R]/Count 1>>\nendobj\n");
    add(
        &mut out,
        &mut offsets,
        "3 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 1700 1100]/Contents 4 0 R>>\nendobj\n",
    );
    add(
        &mut out,
        &mut offsets,
        &format!("4 0 obj\n<</Length {}>>\nstream\n{ink}endstream\nendobj\n", ink.len()),
    );
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    std::fs::write(to, out).expect("a scanned sheet");
}

/// Blocky strokes for the few characters these tests use, as fractions of a
/// character cell.
fn strokes_for(c: char) -> Vec<(f32, f32, f32, f32)> {
    let bar = 0.18;
    match c {
        'I' => vec![(0.4, 0.0, bar, 1.0)],
        'L' => vec![(0.1, 0.0, bar, 1.0), (0.1, 0.0, 0.7, bar)],
        'T' => vec![(0.4, 0.0, bar, 1.0), (0.1, 1.0 - bar, 0.8, bar)],
        'H' => vec![
            (0.1, 0.0, bar, 1.0),
            (0.7, 0.0, bar, 1.0),
            (0.1, 0.45, 0.78, bar),
        ],
        'E' => vec![
            (0.1, 0.0, bar, 1.0),
            (0.1, 0.0, 0.7, bar),
            (0.1, 0.45, 0.6, bar),
            (0.1, 1.0 - bar, 0.7, bar),
        ],
        _ => vec![(0.1, 0.0, 0.7, 1.0)],
    }
}

#[test]
fn a_scan_has_no_text_until_it_is_read() {
    let scratch = Scratch::new("ocrbefore");
    let scan = scratch.at("Scan.pdf");
    a_scanned_sheet(&scan, "TILE");
    // Compared with itself it lines up perfectly, which proves the file is a
    // real, renderable page rather than a broken one.
    let found =
        hyperview::render::compare_two(library(), &scan, 0, &scan, 0, 0.65).expect("comparable");
    assert!(!found.could_not_align);
    assert!(found.changes.is_empty());
}

#[test]
fn reading_a_scan_leaves_the_drawing_looking_exactly_the_same() {
    // The property that makes OCR safe to run on somebody's drawings: the text
    // it adds is invisible, so the page prints exactly as it did.
    if hyperview::ocr::engine_here().is_none() {
        eprintln!("no OCR engine on this machine; skipping");
        return;
    }
    let scratch = Scratch::new("ocrsame");
    let scan = scratch.at("Scan.pdf");
    a_scanned_sheet(&scan, "TILE");
    let out = scratch.at("Searchable.pdf");

    hyperview::render::ocr_over(library(), &scan, &[0], &out, 200, true)
        .expect("the reading should run");

    assert_eq!(sheets_in(&out), 1);
    // Rendered, the two are the same page — nothing was drawn onto it.
    let found = hyperview::render::compare_two(library(), &scan, 0, &out, 0, 0.65)
        .expect("comparable");
    assert!(!found.could_not_align, "{}", found.said);
    assert!(
        found.changes.is_empty(),
        "an invisible text layer must not change the picture: {:?}",
        found.changes
    );
}

#[test]
fn a_sheet_that_already_has_text_is_left_alone_by_default() {
    // Running OCR over a drawing that was never scanned puts a second, worse
    // copy of every word under the good one, and search then finds things
    // twice.
    if hyperview::ocr::engine_here().is_none() {
        eprintln!("no OCR engine on this machine; skipping");
        return;
    }
    let scratch = Scratch::new("ocrskip");
    let set = scratch.at("Set.pdf");
    a_set(&set, 1, &["S-201"]);
    let out = scratch.at("Read.pdf");

    let done = hyperview::render::ocr_over(library(), &set, &[0], &out, 150, true)
        .expect("the reading should run");
    assert!(done.said.contains("already had text"), "{}", done.said);
}

#[test]
fn asking_for_a_reading_with_no_engine_says_which_engine_is_missing() {
    // Not "OCR failed". The person needs to know what to install, or that on
    // Windows there is nothing to install.
    let said = hyperview::ocr::NO_ENGINE;
    assert!(said.contains("Windows"));
    assert!(said.contains("Tesseract"));
}

// ---- sealing ---------------------------------------------------------------

/// A small PNG, written by hand, standing in for a scanned seal.
fn a_seal_picture(to: &Path) {
    // Reuse the icon generator's PNG writer by way of a tiny RGBA buffer: a
    // 64x64 dark disc on transparency, which is the shape of a seal.
    let (w, h) = (64u32, 64u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 - 32.0, y as f32 - 32.0);
            let inside = dx * dx + dy * dy < 30.0 * 30.0;
            if inside {
                rgba.extend_from_slice(&[20, 30, 90, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    let encoded = image::RgbaImage::from_raw(w, h, rgba).expect("a picture");
    encoded.save(to).expect("a seal picture");
}

#[test]
fn the_seal_lands_on_the_sheets_asked_for_and_nowhere_else() {
    let scratch = Scratch::new("seal");
    let set = scratch.at("Set.pdf");
    a_set(&set, 3, &["S-100", "S-101", "S-102"]);
    let picture = scratch.at("seal.png");
    a_seal_picture(&picture);
    let out = scratch.at("Sealed.pdf");

    let done = run(&Operation::Seal {
        source: set.clone(),
        picture: picture.clone(),
        pages: vec![0, 2],
        placement: hyperview::seal::Placement::default(),
        to: out.clone(),
    });
    assert!(done.said.contains("2 sheets"), "{}", done.said);
    assert_eq!(sheets_in(&out), 3);
    assert_eq!(sheets_in(&set), 3);

    // Sheet 1 changed and sheet 2 did not — which is what "the sheets asked
    // for and nowhere else" means when it is checked rather than asserted.
    let first = hyperview::render::compare_two(library(), &set, 0, &out, 0, 0.65)
        .expect("comparable");
    assert!(!first.changes.is_empty(), "the seal should show on sheet 1");
    let second = hyperview::render::compare_two(library(), &set, 1, &out, 1, 0.65)
        .expect("comparable");
    assert!(
        second.changes.is_empty(),
        "sheet 2 was not asked for: {:?}",
        second.changes
    );
}

#[test]
fn sealing_says_out_loud_that_it_is_not_a_digital_signature() {
    let done_said = Operation::Seal {
        source: "a.pdf".into(),
        picture: "seal.png".into(),
        pages: vec![0],
        placement: hyperview::seal::Placement::default(),
        to: "b.pdf".into(),
    };
    let warning = done_said.needs_a_warning().expect("this one warns");
    assert!(warning.contains("not a cryptographic"), "{warning}");
}

#[test]
fn a_seal_that_is_not_a_picture_is_refused_by_name() {
    let scratch = Scratch::new("badseal");
    let set = scratch.at("Set.pdf");
    a_set(&set, 1, &["S-100"]);
    let picture = scratch.at("seal.png");
    std::fs::write(&picture, b"this is not a png").unwrap();

    let refused = hyperview::render::do_one(
        library(),
        &Operation::Seal {
            source: set,
            picture,
            pages: vec![0],
            placement: hyperview::seal::Placement::default(),
            to: scratch.at("Out.pdf"),
        },
    );
    let why = refused.expect_err("that is not a picture");
    assert!(why.contains("seal.png"), "{why}");
}

// ---- the summary report ----------------------------------------------------

#[test]
fn a_summary_report_is_a_pdf_anybody_can_open() {
    let scratch = Scratch::new("report");
    let report = hyperview::report::Report {
        title: "Takeoff summary".into(),
        drawing: "2026-08-13 Structural Stamped.pdf".into(),
        when: "2026-09-18".into(),
        who: "Sam".into(),
        short_by: String::new(),
        headings: vec!["#".into(), "Subject".into(), "Length".into(), "Weight".into()],
        widths: vec![0.08, 0.52, 0.2, 0.2],
        pages: hyperview::report::paginate(
            (0..120)
                .map(|n| hyperview::report::Line {
                    cells: vec![
                        (n + 1).to_string(),
                        format!("W12x26 BEAM {n}"),
                        "24'-0\"".into(),
                        "624 lb".into(),
                    ],
                    heading: false,
                })
                .collect(),
        ),
    };
    let to = scratch.at("Summary.pdf");
    let done =
        hyperview::render::write_report(library(), &report, &to).expect("the report should write");
    assert!(to.exists());
    // Several pages, and pdfium can open what came out — which is the whole
    // test: a report nobody else can open is not a report.
    assert!(report.pages.len() > 1);
    assert_eq!(sheets_in(&to), report.pages.len());
    assert!(done.said.contains("page"), "{}", done.said);
}

#[test]
fn a_report_with_nothing_in_it_is_still_a_file() {
    let scratch = Scratch::new("emptyreport");
    let report = hyperview::report::Report {
        title: "Takeoff summary".into(),
        drawing: "Empty.pdf".into(),
        when: "2026-09-18".into(),
        who: "Sam".into(),
        short_by: String::new(),
        headings: vec!["#".into()],
        widths: vec![1.0],
        pages: hyperview::report::paginate(Vec::new()),
    };
    let to = scratch.at("Summary.pdf");
    hyperview::render::write_report(library(), &report, &to).expect("still a report");
    assert_eq!(sheets_in(&to), 1);
}

#[test]
fn a_short_takeoff_says_so_on_the_report_itself() {
    // Not in a log, not in the status bar somebody has already closed — on the
    // page, above the table, in the file that goes in the bid folder.
    let scratch = Scratch::new("shortreport");
    let report = hyperview::report::Report {
        title: "Takeoff summary".into(),
        drawing: "Structural.pdf".into(),
        when: "2026-09-18".into(),
        who: "Sam".into(),
        short_by: "SHORT: 4 measurements are not in these totals, because their sheets \
                   have no scale — S-201, S-202."
            .into(),
        headings: vec!["#".into(), "Subject".into()],
        widths: vec![0.2, 0.8],
        pages: hyperview::report::paginate(vec![hyperview::report::Line {
            cells: vec!["1".into(), "W12x26".into()],
            heading: false,
        }]),
    };
    let to = scratch.at("Short.pdf");
    hyperview::render::write_report(library(), &report, &to).expect("a report");

    // The warning is really on the page: a report with it and one without are
    // different pictures.
    let without = hyperview::report::Report {
        short_by: String::new(),
        ..report.clone()
    };
    let plain = scratch.at("Plain.pdf");
    hyperview::render::write_report(library(), &without, &plain).expect("a report");
    let found = hyperview::render::compare_two(library(), &plain, 0, &to, 0, 0.65)
        .expect("comparable");
    assert!(
        !found.changes.is_empty() || found.could_not_align,
        "the warning should be visible on the page"
    );
}

// ---- comparing a whole set, and a whole folder of them ----------------------

/// A set of several sheets, each with whatever extra line-work is asked for.
///
/// One file rather than several, because comparing a whole set is the point:
/// the sheets have to sit in one document in a known order so a test can say
/// which of them came back clouded.
fn a_set_with(to: &Path, extras: &[&str]) {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets: Vec<usize> = Vec::new();
    let add = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: String| {
        offsets.push(out.len());
        out.extend_from_slice(body.as_bytes());
    };

    // 1 catalog, 2 pages, then a page and a contents object per sheet.
    let first_page = 3u32;
    let kids: Vec<String> = (0..extras.len())
        .map(|i| format!("{} 0 R", first_page + (i as u32) * 2))
        .collect();
    add(&mut out, &mut offsets, "1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n".to_string());
    add(
        &mut out,
        &mut offsets,
        format!(
            "2 0 obj\n<</Type/Pages/Kids[{}]/Count {}>>\nendobj\n",
            kids.join(" "),
            extras.len()
        ),
    );
    for (i, extra) in extras.iter().enumerate() {
        let page = first_page + (i as u32) * 2;
        let contents = page + 1;
        let body: String = (0..20)
            .map(|row| {
                format!(
                    "BT /F1 11 Tf 200 {} Td (GENERAL NOTE {row} — SEE SPECIFICATIONS) Tj ET\n",
                    1900 - row * 60
                )
            })
            .collect();
        let frame = "4 w 150 150 m 2874 150 l 2874 2010 l 150 2010 l h S\n";
        let stream = format!(
            "{frame}{body}BT /F1 48 Tf 2600 90 Td (S-{}) Tj ET\n{extra}",
            201 + i
        );
        add(
            &mut out,
            &mut offsets,
            format!(
                "{page} 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]\
                 /Resources<</Font<</F1 <</Type/Font/Subtype/Type1/BaseFont/Helvetica>> >> >>\
                 /Contents {contents} 0 R>>\nendobj\n"
            ),
        );
        add(
            &mut out,
            &mut offsets,
            format!(
                "{contents} 0 obj\n<</Length {}>>\nstream\n{stream}endstream\nendobj\n",
                stream.len()
            ),
        );
    }

    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    std::fs::write(to, out).expect("a set");
}

/// How many annotations are on each sheet of a file, in order.
fn annotations_per_sheet(path: &Path) -> Vec<usize> {
    let doc = pdf::Document::open(path).expect("the file should open");
    (0..doc.page_count())
        .map(|i| {
            doc.page(i)
                .map(|page| doc.annotations(&page).len())
                .unwrap_or(0)
        })
        .collect()
}

#[test]
fn comparing_a_whole_set_clouds_only_the_sheets_that_changed() {
    let scratch = Scratch::new("batchcompare");
    let older_folder = scratch.at("last month");
    let newer_folder = scratch.at("this month");
    let results = scratch.at("clouded");
    for folder in [&older_folder, &newer_folder, &results] {
        std::fs::create_dir_all(folder).expect("a folder");
    }

    // The same three sheets in both issues, with one detail added to the
    // middle sheet in the newer one.
    let name = "Building A Structural.pdf";
    a_set_with(&older_folder.join(name), &["", "", ""]);
    a_set_with(
        &newer_folder.join(name),
        &["", "0 0 0 rg 900 900 300 240 re f\n", ""],
    );

    let work = hyperview::batch::Job::Compare {
        older: older_folder.clone(),
        darkness: 0.65,
        into: Some(results.clone()),
    };
    let report =
        hyperview::render::batch_over(library(), &work, &[newer_folder.join(name)])
            .expect("the batch should run");

    assert_eq!(report.lines.len(), 1, "{}", report.as_text());
    let wrote = match &report.lines[0].outcome {
        hyperview::batch::Outcome::Done { wrote, .. } => wrote.clone(),
        other => panic!("the comparison should have run: {other:?}\n{}", report.as_text()),
    };
    assert_eq!(wrote.len(), 1);
    let clouded = &wrote[0];
    assert!(clouded.starts_with(&results), "it should land in the folder asked for");

    // Three sheets still, and marks on the middle one only.
    let per_sheet = annotations_per_sheet(clouded);
    assert_eq!(per_sheet.len(), 3, "the whole set should come through");
    assert_eq!(per_sheet[0], 0, "sheet 1 did not change");
    assert!(per_sheet[1] > 0, "sheet 2 changed and should be clouded");
    assert_eq!(per_sheet[2], 0, "sheet 3 did not change");

    // And the originals are untouched.
    assert_eq!(annotations_per_sheet(&newer_folder.join(name)), vec![0, 0, 0]);
    assert_eq!(annotations_per_sheet(&older_folder.join(name)), vec![0, 0, 0]);
}

#[test]
fn a_clouded_difference_is_a_real_cloud_that_any_reader_can_draw() {
    // A Square annotation with a cloudy border effect is what Revu writes and
    // what every other reader already knows how to draw. A box without /BE is
    // a box, and nobody reading the set would know it meant a revision.
    let scratch = Scratch::new("cloudshape");
    let older_folder = scratch.at("old");
    let newer_folder = scratch.at("new");
    for folder in [&older_folder, &newer_folder] {
        std::fs::create_dir_all(folder).expect("a folder");
    }
    let name = "S-201.pdf";
    a_set_with(&older_folder.join(name), &[""]);
    a_set_with(&newer_folder.join(name), &["0 0 0 rg 900 900 300 240 re f\n"]);

    let work = hyperview::batch::Job::Compare {
        older: older_folder.clone(),
        darkness: 0.65,
        into: None,
    };
    let report =
        hyperview::render::batch_over(library(), &work, &[newer_folder.join(name)])
            .expect("the batch should run");
    let wrote = match &report.lines[0].outcome {
        hyperview::batch::Outcome::Done { wrote, .. } => wrote.clone(),
        other => panic!("{other:?}\n{}", report.as_text()),
    };

    let doc = pdf::Document::open(&wrote[0]).expect("the clouded file should open");
    let page = doc.page(0).expect("a sheet");
    let list = doc.annotations(&page);
    assert!(!list.is_empty());
    let mut clouds = 0;
    for reference in &list {
        let object = doc.get(*reference);
        let dict = object.as_dict().cloned().expect("an annotation");
        assert_eq!(
            dict.get("Subtype").and_then(|o| o.as_name()).map(|n| n.as_str().to_string()),
            Some("Square".to_string())
        );
        let effect = doc.follow(dict.get("BE").unwrap_or(&pdf::Object::Null));
        let effect = effect.as_dict().expect("a border effect");
        assert_eq!(
            effect.get("S").and_then(|o| o.as_name()).map(|n| n.as_str().to_string()),
            Some("C".to_string()),
            "the border effect has to be cloudy"
        );
        clouds += 1;
    }
    assert!(clouds > 0);
}

#[test]
fn a_set_with_no_older_issue_of_its_name_is_reported_rather_than_compared() {
    // Comparing a set against a near match is how a bid ends up reviewing the
    // wrong revision. Saying so plainly is the only right answer.
    let scratch = Scratch::new("nopair");
    let older_folder = scratch.at("old");
    let newer_folder = scratch.at("new");
    for folder in [&older_folder, &newer_folder] {
        std::fs::create_dir_all(folder).expect("a folder");
    }
    a_set_with(&older_folder.join("Building B.pdf"), &[""]);
    a_set_with(&newer_folder.join("Building A.pdf"), &[""]);

    let work = hyperview::batch::Job::Compare {
        older: older_folder.clone(),
        darkness: 0.65,
        into: None,
    };
    let report = hyperview::render::batch_over(
        library(),
        &work,
        &[newer_folder.join("Building A.pdf")],
    )
    .expect("the batch should run");

    match &report.lines[0].outcome {
        hyperview::batch::Outcome::Failed(why) => {
            assert!(why.contains("no file with this name"), "{why}");
            assert!(why.contains("near match is not used"), "{why}");
        }
        other => panic!("it should not have been compared against anything: {other:?}"),
    }
    // And nothing was written beside it.
    let beside: Vec<_> = std::fs::read_dir(&newer_folder)
        .expect("the folder")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(beside, vec!["Building A.pdf".to_string()]);
}

// ---- pictures on a sheet ---------------------------------------------------

/// A one page drawing with nothing on it, to put things on.
fn a_blank_sheet(to: &Path) {
    a_set_with(to, &[""]);
}

#[test]
fn a_picture_put_on_a_sheet_is_really_there_when_it_is_rendered() {
    // An annotation carrying a picture is only worth anything if every reader
    // draws it. This puts one down, saves the file, and looks at the pixels
    // pdfium gets back.
    let scratch = Scratch::new("picture");
    let sheet = scratch.at("S-201.pdf");
    a_blank_sheet(&sheet);

    // A solid red square, which is easy to find again.
    let red = image::RgbaImage::from_pixel(64, 64, image::Rgba([220, 30, 30, 255]));
    let picture = hyperview::picture::from_image(&red);

    let doc = pdf::Document::open(&sheet).expect("the sheet should open");
    let mut placer = annot::place::Placer::new(&doc).by("Test");
    let mut markup = annot::Markup::new(annot::Subtype::Stamp);
    markup.set_box([1000.0, 1000.0, 1600.0, 1600.0]);
    markup.picture = Some(picture);
    assert!(placer.add(0, &mut markup).is_some());
    let updated = placer.finish().apply(&doc);
    std::fs::write(&sheet, updated).expect("write");

    let png = scratch.at("look.png");
    hyperview::render::picture_of(library(), &sheet, 0, 600, 600, &png)
        .expect("the sheet should render");
    let drawn = image::open(&png).expect("the picture should open").to_rgb8();

    // The page is 3024 by 2160 and the square sits at 1000..1600 in both
    // directions from the bottom left, so in a rendered picture it is about
    // a third across and, y being upside down, a bit above the middle.
    let mut red_pixels = 0;
    for pixel in drawn.pixels() {
        if pixel[0] > 150 && pixel[1] < 100 && pixel[2] < 100 {
            red_pixels += 1;
        }
    }
    assert!(
        red_pixels > 100,
        "the picture should be visible: {red_pixels} red pixels"
    );
}

#[test]
fn a_picture_with_transparency_does_not_arrive_on_a_black_square() {
    // A PDF has no room for a fourth channel in the picture itself. Without a
    // soft mask, everything see-through comes out black — which is exactly how
    // a scanned seal ends up as a black rectangle on a drawing.
    let scratch = Scratch::new("softmask");
    let sheet = scratch.at("S-201.pdf");
    a_blank_sheet(&sheet);

    let mut spot = image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 0]));
    for y in 20..44 {
        for x in 20..44 {
            spot.put_pixel(x, y, image::Rgba([30, 60, 200, 255]));
        }
    }
    let picture = hyperview::picture::from_image(&spot);
    assert!(picture.mask.is_some(), "it has transparency to carry");

    let doc = pdf::Document::open(&sheet).expect("open");
    let mut placer = annot::place::Placer::new(&doc).by("Test");
    let mut markup = annot::Markup::new(annot::Subtype::Stamp);
    markup.set_box([600.0, 600.0, 2400.0, 1800.0]);
    markup.picture = Some(picture);
    placer.add(0, &mut markup);
    std::fs::write(&sheet, placer.finish().apply(&doc)).expect("write");

    let png = scratch.at("look.png");
    hyperview::render::picture_of(library(), &sheet, 0, 600, 600, &png).expect("render");
    let drawn = image::open(&png).expect("open").to_rgb8();

    let mut black = 0;
    let mut blue = 0;
    for pixel in drawn.pixels() {
        if pixel[0] < 40 && pixel[1] < 40 && pixel[2] < 40 {
            black += 1;
        }
        if pixel[2] > 140 && pixel[0] < 110 {
            blue += 1;
        }
    }
    assert!(blue > 50, "the spot should be there: {blue}");
    assert!(
        black < blue / 4,
        "the see-through part should not be black: {black} black to {blue} blue"
    );
}

#[test]
fn a_cloud_saved_into_a_file_still_reads_as_a_cloud() {
    // The whole point of a revision cloud is that the general contractor sees
    // a cloud. An appearance stream wins over the border effect in every
    // reader, so if the appearance is a rectangle, a rectangle is what they
    // get.
    let scratch = Scratch::new("savedcloud");
    let sheet = scratch.at("S-201.pdf");
    a_blank_sheet(&sheet);

    let doc = pdf::Document::open(&sheet).expect("open");
    let mut placer = annot::place::Placer::new(&doc).by("Test");
    let mut markup = annot::Markup::new(annot::Subtype::Square);
    markup.set_colour([1.0, 0.0, 0.0]).set_width(6.0);
    markup.set_box([600.0, 600.0, 2400.0, 1600.0]);
    let mut effect = pdf::Dict::new();
    effect.set("S", pdf::Object::name("C"));
    effect.set("I", pdf::Object::Real(2.0));
    markup.dict.set("BE", pdf::Object::Dict(effect));
    placer.add(0, &mut markup);
    std::fs::write(&sheet, placer.finish().apply(&doc)).expect("write");

    let png = scratch.at("look.png");
    hyperview::render::picture_of(library(), &sheet, 0, 900, 900, &png).expect("render");
    let drawn = image::open(&png).expect("open").to_rgb8();

    // A cloud's top edge wanders up and down; a rectangle's does not. Look at
    // where the topmost red pixel is in each column across the middle of the
    // shape and count how many different heights there are.
    let (w, h) = (drawn.width(), drawn.height());
    let mut tops: Vec<Option<u32>> = Vec::new();
    for x in 0..w {
        let mut top = None;
        for y in 0..h {
            let p = drawn.get_pixel(x, y);
            if p[0] > 150 && p[1] < 110 && p[2] < 110 {
                top = Some(y);
                break;
            }
        }
        tops.push(top);
    }
    let found: Vec<u32> = tops.into_iter().flatten().collect();
    assert!(!found.is_empty(), "the cloud should be drawn at all");
    let highest = *found.iter().min().unwrap();
    let lowest = *found.iter().max().unwrap();
    assert!(
        lowest - highest > 8,
        "a cloud's edge goes up and down; this one is flat ({highest} to {lowest})"
    );
}

// ---- repair, labels, unflattening, redaction and export --------------------

#[test]
fn a_set_with_a_wrecked_table_of_contents_opens_again_after_a_repair() {
    // A set that opens with sheets missing is how a bid goes out short. The
    // repair reads the file itself rather than believing its table.
    let scratch = Scratch::new("repair");
    let sheet = scratch.at("Building A.pdf");
    a_set_with(&sheet, &["", "", ""]);

    // Wreck the offsets the way a program that got them wrong would.
    let mut bytes = std::fs::read(&sheet).expect("read");
    let at = bytes
        .windows(4)
        .position(|w| w == b"xref")
        .expect("there is a table");
    for byte in bytes[at..].iter_mut() {
        if byte.is_ascii_digit() {
            *byte = b'7';
        }
    }
    std::fs::write(&sheet, &bytes).expect("write");

    let fixed = scratch.at("Building A repaired.pdf");
    let done = run(&Operation::Repair {
        source: sheet.clone(),
        to: fixed.clone(),
    });
    assert!(done.said.contains("rebuilt"), "{}", done.said);
    assert_eq!(sheets_in(&fixed), 3, "{}", done.said);
    // And the wrecked original is left exactly as it was.
    assert_eq!(std::fs::read(&sheet).expect("read"), bytes);
}

#[test]
fn page_labels_are_written_where_every_reader_can_see_them() {
    let scratch = Scratch::new("labels");
    let sheet = scratch.at("Building A.pdf");
    a_set_with(&sheet, &["", "", ""]);
    let labelled = scratch.at("Building A labelled.pdf");

    let done = run(&Operation::PageLabels {
        source: sheet.clone(),
        labels: vec![
            (0, "S-201".into()),
            (1, "S-202".into()),
            (2, "S-203".into()),
        ],
        to: labelled.clone(),
    });
    assert!(done.said.contains("page labels"), "{}", done.said);

    let doc = pdf::Document::open(&labelled).expect("open");
    let labels = doc.follow(doc.catalog().get("PageLabels").expect("there are labels"));
    let labels = labels.as_dict().expect("a number tree");
    let nums = doc.follow(labels.get("Nums").expect("Nums"));
    let nums = nums.as_array().expect("an array");
    assert_eq!(nums.len(), 6, "a page and a label for each sheet");
    let first = doc.follow(&nums[1]);
    let first = first.as_dict().expect("a label");
    assert_eq!(
        first.get("P").and_then(|o| o.as_text()),
        Some("S-201".to_string())
    );
}

#[test]
fn asking_for_labels_with_no_numbers_says_so_rather_than_writing_nothing() {
    let scratch = Scratch::new("nolabels");
    let sheet = scratch.at("Building A.pdf");
    a_set_with(&sheet, &[""]);
    let out = scratch.at("out.pdf");
    let why = hyperview::render::do_one(
        library(),
        &Operation::PageLabels {
            source: sheet,
            labels: Vec::new(),
            to: out.clone(),
        },
    )
    .expect_err("it should refuse");
    assert!(why.contains("nothing to write in"), "{why}");
    assert!(!out.exists(), "and nothing should have been written");
}

#[test]
fn markups_flattened_by_hyperview_can_be_lifted_back_out() {
    // Flattening turns a markup into paint. Paint cannot be read back as a
    // markup by looking at it — so the markups themselves go into the
    // flattened file, and this is what makes Unflatten possible at all.
    let scratch = Scratch::new("unflatten");
    let sheet = scratch.at("S-201.pdf");
    a_set_with(&sheet, &[""]);

    // Put two markups on it.
    let doc = pdf::Document::open(&sheet).expect("open");
    let mut placer = annot::place::Placer::new(&doc).by("Sam");
    for (n, area) in [(1, [400.0, 400.0, 900.0, 700.0]), (2, [1200.0, 900.0, 1700.0, 1200.0])] {
        let mut markup = annot::Markup::new(annot::Subtype::Square);
        markup.set_colour([1.0, 0.0, 0.0]).set_width(3.0);
        markup.set_box(area);
        markup.set_subject(&format!("W12x26 number {n}"));
        placer.add(0, &mut markup);
    }
    std::fs::write(&sheet, placer.finish().apply(&doc)).expect("write");
    assert_eq!(annotations_per_sheet(&sheet), vec![2]);

    let flat = scratch.at("S-201 flattened.pdf");
    let done = run(&Operation::Flatten {
        source: sheet.clone(),
        pages: vec![0],
        to: flat.clone(),
    });
    assert!(done.said.contains("Unflatten"), "{}", done.said);
    // Flattened: the markups are paint now.
    assert_eq!(annotations_per_sheet(&flat), vec![0]);

    let back = scratch.at("S-201 back.pdf");
    let done = run(&Operation::Unflatten {
        source: flat.clone(),
        to: back.clone(),
    });
    assert!(done.said.contains("lifted"), "{}", done.said);
    assert_eq!(annotations_per_sheet(&back), vec![2]);

    // And they are the same markups, not two empty boxes.
    let doc = pdf::Document::open(&back).expect("open");
    let page = doc.page(0).expect("a sheet");
    let subjects: Vec<String> = doc
        .annotations(&page)
        .iter()
        .map(|r| doc.get(*r))
        .filter_map(|o| o.as_dict().and_then(|d| d.get("Subj")).and_then(|o| o.as_text()))
        .collect();
    assert!(subjects.iter().any(|s| s.contains("W12x26")), "{subjects:?}");

    // Lifting twice does not put a second copy on.
    let twice = scratch.at("S-201 twice.pdf");
    let again = hyperview::render::do_one(
        library(),
        &Operation::Unflatten {
            source: back.clone(),
            to: twice,
        },
    );
    assert!(again.is_err(), "there is nothing left to lift");
}

#[test]
fn unflattening_a_set_hyperview_never_flattened_says_so_plainly() {
    let scratch = Scratch::new("nothingtolift");
    let sheet = scratch.at("S-201.pdf");
    a_set_with(&sheet, &[""]);
    let why = hyperview::render::do_one(
        library(),
        &Operation::Unflatten {
            source: sheet,
            to: scratch.at("out.pdf"),
        },
    )
    .expect_err("it should refuse");
    assert!(why.contains("cannot be read back"), "{why}");
}

#[test]
fn applying_a_redaction_takes_what_was_under_it_out_of_the_file() {
    // The test of a redaction is not whether the box is black. It is whether
    // the words are still in the file underneath it.
    let scratch = Scratch::new("redact");
    let sheet = scratch.at("S-201.pdf");
    a_set_with(&sheet, &["BT /F1 60 Tf 700 700 Td (SECRET BID NUMBER) Tj ET\n"]);

    // A redaction mark over the words.
    let doc = pdf::Document::open(&sheet).expect("open");
    let mut placer = annot::place::Placer::new(&doc).by("Sam");
    let mut mark = annot::Markup::new(annot::Subtype::Square);
    mark.set_subject("Redaction");
    mark.set_box([650.0, 650.0, 1900.0, 800.0]);
    placer.add(0, &mut mark);
    std::fs::write(&sheet, placer.finish().apply(&doc)).expect("write");

    let out = scratch.at("S-201 redacted.pdf");
    let done = run(&Operation::ApplyRedactions {
        source: sheet.clone(),
        to: out.clone(),
        as_pictures: false,
        dpi: 200,
    });
    assert!(done.said.contains("gone from that file"), "{}", done.said);

    // The words are not in the bytes of the new file.
    let bytes = std::fs::read(&out).expect("read");
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        !text.contains("SECRET BID NUMBER"),
        "the words should be gone from the file"
    );
    // And they are still in the original, which is never touched.
    let was = String::from_utf8_lossy(&std::fs::read(&sheet).expect("read")).into_owned();
    assert!(was.contains("SECRET BID NUMBER"));
}

#[test]
fn redacting_by_rendering_leaves_nothing_of_the_words_anywhere() {
    let scratch = Scratch::new("redactpic");
    let sheet = scratch.at("S-201.pdf");
    a_set_with(
        &sheet,
        &[
            "BT /F1 60 Tf 700 700 Td (SECRET BID NUMBER) Tj ET\n",
            "",
        ],
    );
    let doc = pdf::Document::open(&sheet).expect("open");
    let mut placer = annot::place::Placer::new(&doc).by("Sam");
    let mut mark = annot::Markup::new(annot::Subtype::Square);
    mark.set_subject("Redaction");
    mark.set_box([650.0, 650.0, 1900.0, 800.0]);
    placer.add(0, &mut mark);
    std::fs::write(&sheet, placer.finish().apply(&doc)).expect("write");

    let out = scratch.at("S-201 redacted.pdf");
    let done = run(&Operation::ApplyRedactions {
        source: sheet,
        to: out.clone(),
        as_pictures: true,
        dpi: 100,
    });
    assert!(done.said.contains("pictures now"), "{}", done.said);
    assert_eq!(sheets_in(&out), 2, "both sheets should still be there");
    let text = String::from_utf8_lossy(&std::fs::read(&out).expect("read")).into_owned();
    assert!(!text.contains("SECRET BID NUMBER"));
}

#[test]
fn asking_to_redact_with_no_marks_says_what_to_do_instead() {
    let scratch = Scratch::new("noredaction");
    let sheet = scratch.at("S-201.pdf");
    a_set_with(&sheet, &[""]);
    let why = hyperview::render::do_one(
        library(),
        &Operation::ApplyRedactions {
            source: sheet,
            to: scratch.at("out.pdf"),
            as_pictures: false,
            dpi: 200,
        },
    )
    .expect_err("it should refuse");
    assert!(why.contains("Redaction tool"), "{why}");
}

#[test]
fn exporting_writes_one_picture_per_sheet_and_names_them_in_order() {
    let scratch = Scratch::new("export");
    let sheet = scratch.at("Building A.pdf");
    a_set_with(&sheet, &["", "", ""]);
    let into = scratch.at("pictures");

    let done = run(&Operation::ExportPictures {
        source: sheet.clone(),
        pages: vec![0, 2],
        into: into.clone(),
        dpi: 72,
        format: "png".into(),
    });
    assert_eq!(done.wrote.len(), 2, "{}", done.said);
    for path in &done.wrote {
        assert!(path.exists());
        let picture = image::open(path).expect("it should be a real picture");
        assert!(picture.width() > 100 && picture.height() > 100);
    }
    let names: Vec<String> = done
        .wrote
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert!(names[0].contains("001"), "{names:?}");
    assert!(names[1].contains("003"), "the third sheet keeps its number: {names:?}");
}

// ---- the last batch jobs ---------------------------------------------------

#[test]
fn a_batch_summary_covers_every_set_and_names_the_file_each_came_from() {
    let scratch = Scratch::new("batchsummary");
    let one = scratch.at("Building A.pdf");
    let two = scratch.at("Building B.pdf");
    a_set_with(&one, &["", ""]);
    a_set_with(&two, &[""]);

    // A measurement on each, with a scale on the sheet so it reads.
    for path in [&one, &two] {
        let doc = pdf::Document::open(path).expect("open");
        let mut placer = annot::place::Placer::new(&doc).by("Sam");
        placer.set_scale(0, Some(annot::measure::imperial(48.0, "1/4\" = 1'-0\"", 16)));
        let mut markup = annot::Markup::new(annot::Subtype::Line);
        markup.set("IT", pdf::Object::name("LineDimension"));
        markup.set_line([100.0, 100.0], [820.0, 100.0]);
        markup.set_subject("W12x26");
        markup.set_contents("40'-0\"");
        placer.add(0, &mut markup);
        std::fs::write(path, placer.finish().apply(&doc)).expect("write");
    }

    let to = scratch.at("Takeoff summary.pdf");
    let work = hyperview::batch::Job::Summary { to: to.clone() };
    let report = hyperview::render::batch_over(library(), &work, &[one, two])
        .expect("the batch should run");
    match &report.lines[0].outcome {
        hyperview::batch::Outcome::Done { said, .. } => {
            assert!(said.to_lowercase().contains("summary"), "{said}");
        }
        other => panic!("{other:?}\n{}", report.as_text()),
    }
    assert!(to.exists());
    assert!(sheets_in(&to) >= 1);
}

#[test]
fn a_batch_summary_says_out_loud_when_a_sheet_had_no_scale() {
    // A total that quietly swallowed six unscaled sheets is a bid somebody
    // loses money on.
    let scratch = Scratch::new("shortbatch");
    let one = scratch.at("Building A.pdf");
    a_set_with(&one, &[""]);
    let doc = pdf::Document::open(&one).expect("open");
    let mut placer = annot::place::Placer::new(&doc).by("Sam");
    let mut markup = annot::Markup::new(annot::Subtype::Line);
    markup.set("IT", pdf::Object::name("LineDimension"));
    markup.set_line([100.0, 100.0], [820.0, 100.0]);
    placer.add(0, &mut markup);
    std::fs::write(&one, placer.finish().apply(&doc)).expect("write");

    let to = scratch.at("Takeoff summary.pdf");
    let work = hyperview::batch::Job::Summary { to: to.clone() };
    hyperview::render::batch_over(library(), &work, &[one]).expect("run");

    // The warning is on the page itself, not only in the status line: a report
    // that goes in a bid folder has to carry its own warning.
    let words = hyperview::render::words_on(library(), &to, 0).expect("read the words");
    assert!(words.contains("SHORT"), "the report should say it is short: {words}");
    assert!(words.contains("never counted as zero"), "{words}");
}

#[test]
fn a_batch_crop_trims_every_sheet_and_leaves_the_originals_alone() {
    let scratch = Scratch::new("batchcrop");
    let one = scratch.at("Building A.pdf");
    a_set_with(&one, &["", ""]);
    let into = scratch.at("cropped");
    std::fs::create_dir_all(&into).expect("a folder");

    let work = hyperview::batch::Job::Crop {
        box_: [200.0, 200.0, 2000.0, 1600.0],
        into: Some(into.clone()),
    };
    let report = hyperview::render::batch_over(library(), &work, &[one.clone()])
        .expect("the batch should run");
    let wrote = match &report.lines[0].outcome {
        hyperview::batch::Outcome::Done { wrote, .. } => wrote.clone(),
        other => panic!("{other:?}\n{}", report.as_text()),
    };
    let doc = pdf::Document::open(&wrote[0]).expect("open");
    let page = doc.page(0).expect("a sheet");
    // page_box is the crop box when there is one, which is the whole point.
    assert_eq!(doc.page_box(&page), [200.0, 200.0, 2000.0, 1600.0]);

    // And the original is still whole.
    let was = pdf::Document::open(&one).expect("open");
    let page = was.page(0).expect("a sheet");
    assert_eq!(was.page_box(&page), [0.0, 0.0, 3024.0, 2160.0]);
}

#[test]
fn a_batch_overlay_lays_each_set_over_the_one_of_the_same_name() {
    let scratch = Scratch::new("batchoverlay");
    let older_folder = scratch.at("last month");
    let newer_folder = scratch.at("this month");
    for folder in [&older_folder, &newer_folder] {
        std::fs::create_dir_all(folder).expect("a folder");
    }
    let name = "Building A.pdf";
    a_set_with(&older_folder.join(name), &["", ""]);
    a_set_with(
        &newer_folder.join(name),
        &["", "0 0 0 rg 900 900 300 240 re f\n"],
    );

    let work = hyperview::batch::Job::Overlay {
        older: older_folder.clone(),
        darkness: 0.65,
        into: None,
    };
    let report = hyperview::render::batch_over(library(), &work, &[newer_folder.join(name)])
        .expect("the batch should run");
    let (wrote, said) = match &report.lines[0].outcome {
        hyperview::batch::Outcome::Done { wrote, said } => (wrote.clone(), said.clone()),
        other => panic!("{other:?}\n{}", report.as_text()),
    };
    assert_eq!(sheets_in(&wrote[0]), 2, "{said}");
    assert!(said.contains("Nothing has been measured"), "{said}");
}

#[test]
fn a_batch_overlay_with_no_matching_issue_is_reported_rather_than_guessed_at() {
    let scratch = Scratch::new("overlaynopair");
    let older_folder = scratch.at("old");
    let newer_folder = scratch.at("new");
    for folder in [&older_folder, &newer_folder] {
        std::fs::create_dir_all(folder).expect("a folder");
    }
    a_set_with(&older_folder.join("Building B.pdf"), &[""]);
    a_set_with(&newer_folder.join("Building A.pdf"), &[""]);

    let work = hyperview::batch::Job::Overlay {
        older: older_folder,
        darkness: 0.65,
        into: None,
    };
    let report = hyperview::render::batch_over(
        library(),
        &work,
        &[newer_folder.join("Building A.pdf")],
    )
    .expect("the batch should run");
    match &report.lines[0].outcome {
        hyperview::batch::Outcome::Failed(why) => {
            assert!(why.contains("near match is not used"), "{why}");
        }
        other => panic!("it should not have been laid over anything: {other:?}"),
    }
}

#[test]
fn a_batch_stamp_finds_its_corner_on_every_paper_size() {
    // A set of letter pages and a set of ARCH E1 sheets both get it in the
    // same place on the paper, which is what "bottom right" means.
    let scratch = Scratch::new("batchstamp");
    let big = scratch.at("Big.pdf");
    a_set_with(&big, &[""]);
    let picture = scratch.at("stamp.png");
    let red = image::RgbaImage::from_pixel(120, 60, image::Rgba([200, 30, 30, 255]));
    red.save(&picture).expect("a picture");

    let work = hyperview::batch::Job::ApplyStamp {
        picture,
        width: 200.0,
        spot: hyperview::stamps::Spot::BottomRight,
        into: None,
    };
    let report = hyperview::render::batch_over(library(), &work, &[big.clone()])
        .expect("the batch should run");
    let wrote = match &report.lines[0].outcome {
        hyperview::batch::Outcome::Done { wrote, .. } => wrote.clone(),
        other => panic!("{other:?}\n{}", report.as_text()),
    };
    let png = scratch.at("look.png");
    hyperview::render::picture_of(library(), &wrote[0], 0, 800, 800, &png).expect("render");
    let drawn = image::open(&png).expect("open").to_rgb8();

    // The red should be in the bottom right quarter and nowhere else.
    let (w, h) = (drawn.width(), drawn.height());
    let mut in_corner = 0;
    let mut elsewhere = 0;
    for (x, y, pixel) in drawn.enumerate_pixels() {
        if pixel[0] > 150 && pixel[1] < 90 && pixel[2] < 90 {
            if x > w / 2 && y > h / 2 {
                in_corner += 1;
            } else {
                elsewhere += 1;
            }
        }
    }
    assert!(in_corner > 50, "the stamp should be in the bottom right: {in_corner}");
    assert_eq!(elsewhere, 0, "and nowhere else");
}

// ---- locking a set ---------------------------------------------------------

#[test]
fn a_locked_set_needs_its_password_and_then_opens_whole() {
    // The test of encryption is not that a file was written. It is that the
    // password opens it, that the wrong one does not, and that every sheet is
    // still there — checked through pdfium, which is a different program's
    // opinion of whether it opens.
    let scratch = Scratch::new("locked");
    let plain = scratch.at("Building A.pdf");
    a_set_with(&plain, &["", "", ""]);
    let locked = scratch.at("Building A locked.pdf");

    let done = run(&Operation::Secure {
        source: plain.clone(),
        to: locked.clone(),
        open_password: "bolt5tons".into(),
        owner_password: String::new(),
        allowed: pdf::crypt::Allowed::default(),
    });
    assert!(done.said.contains("cannot be opened without"), "{}", done.said);
    assert!(done.said.contains("keeps no copy"), "{}", done.said);

    // Without the password it will not open at all.
    let refused = hyperview::render::pages_in(library(), &locked);
    assert!(refused.is_err(), "it should be shut without the password");
    assert!(
        refused.unwrap_err().to_lowercase().contains("password"),
        "and say so"
    );

    // The bytes of the drawing are not lying about in the file.
    let raw = std::fs::read(&locked).expect("read");
    let text = String::from_utf8_lossy(&raw);
    assert!(
        !text.contains("GENERAL NOTE"),
        "the words should not be readable in a locked file"
    );

    // And the original is untouched and still opens.
    assert_eq!(sheets_in(&plain), 3);
}

#[test]
fn locking_with_permissions_only_leaves_it_openable_by_anybody() {
    // No password to open, but the permissions travel with the file — which is
    // what a set going to a general contractor usually wants.
    let scratch = Scratch::new("permsonly");
    let plain = scratch.at("Building A.pdf");
    a_set_with(&plain, &[""]);
    let locked = scratch.at("Building A locked.pdf");

    let done = run(&Operation::Secure {
        source: plain,
        to: locked.clone(),
        open_password: String::new(),
        owner_password: "gusset".into(),
        allowed: pdf::crypt::Allowed::read_only(),
    });
    assert!(done.said.contains("without a password"), "{}", done.said);
    assert!(done.said.contains("print it"), "{}", done.said);
    assert!(
        done.said.contains("request a reader honours, not a lock"),
        "it has to say what a permission really is: {}",
        done.said
    );

    // It opens with no password at all.
    assert_eq!(
        hyperview::render::pages_in(library(), &locked).expect("it should open"),
        1
    );
}

#[test]
fn a_set_that_is_already_locked_is_not_locked_twice() {
    let scratch = Scratch::new("twice");
    let plain = scratch.at("Building A.pdf");
    a_set_with(&plain, &[""]);
    let once = scratch.at("once.pdf");
    run(&Operation::Secure {
        source: plain,
        to: once.clone(),
        open_password: "bolt".into(),
        owner_password: String::new(),
        allowed: pdf::crypt::Allowed::default(),
    });
    let why = hyperview::render::do_one(
        library(),
        &Operation::Secure {
            source: once,
            to: scratch.at("twice.pdf"),
            open_password: "other".into(),
            owner_password: String::new(),
            allowed: pdf::crypt::Allowed::default(),
        },
    )
    .expect_err("it should refuse");
    assert!(why.contains("already locked"), "{why}");
}

#[test]
fn the_markups_on_a_set_survive_being_locked() {
    let scratch = Scratch::new("lockedmarks");
    let plain = scratch.at("Building A.pdf");
    a_set_with(&plain, &[""]);
    let doc = pdf::Document::open(&plain).expect("open");
    let mut placer = annot::place::Placer::new(&doc).by("Sam");
    let mut markup = annot::Markup::new(annot::Subtype::Square);
    markup.set_box([400.0, 400.0, 900.0, 700.0]);
    markup.set_subject("W12x26");
    placer.add(0, &mut markup);
    std::fs::write(&plain, placer.finish().apply(&doc)).expect("write");

    let locked = scratch.at("locked.pdf");
    run(&Operation::Secure {
        source: plain,
        to: locked.clone(),
        open_password: String::new(),
        owner_password: String::new(),
        allowed: pdf::crypt::Allowed::default(),
    });
    assert_eq!(annotations_per_sheet(&locked), vec![1], "the markup is still on it");
}

// ---- form fields -----------------------------------------------------------

#[test]
fn a_form_field_is_a_field_and_not_just_a_box_that_looks_like_one() {
    // A widget written onto a page without an entry in the document's own
    // list of fields looks right and cannot be filled in.
    let scratch = Scratch::new("form");
    let sheet = scratch.at("Transmittal.pdf");
    a_set_with(&sheet, &[""]);

    let doc = pdf::Document::open(&sheet).expect("open");
    let mut placer = annot::place::Placer::new(&doc).by("Sam");
    for (at, (field, name)) in [
        (hyperview::forms::Field::Text, "JobNumber"),
        (hyperview::forms::Field::CheckBox, "Approved"),
        (hyperview::forms::Field::Signature, "SignHere"),
    ]
    .iter()
    .enumerate()
    {
        let y = 400.0 + at as f64 * 120.0;
        let mut markup = hyperview::forms::make(
            *field,
            name,
            [400.0, y, 1200.0, y + 60.0],
            &hyperview::pen::Pen::default(),
        );
        assert!(placer.add(0, &mut markup).is_some());
    }
    std::fs::write(&sheet, placer.finish().apply(&doc)).expect("write");

    let again = pdf::Document::open(&sheet).expect("open");
    let form = again.follow(again.catalog().get("AcroForm").expect("there is a form"));
    let form = form.as_dict().expect("a form dictionary");
    let fields = again.follow(form.get("Fields").expect("Fields"));
    let fields = fields.as_array().expect("an array");
    assert_eq!(fields.len(), 3, "every field is in the document's own list");

    // And each one reads back as the kind it was made as.
    let names: Vec<String> = fields
        .iter()
        .map(|f| again.follow(f))
        .filter_map(|o| o.as_dict().and_then(|d| d.get("T")).and_then(|o| o.as_text()))
        .collect();
    assert!(names.contains(&"JobNumber".to_string()), "{names:?}");
    assert!(names.contains(&"Approved".to_string()), "{names:?}");
}

#[test]
fn a_form_field_is_visible_when_the_sheet_is_rendered() {
    let scratch = Scratch::new("formlook");
    let sheet = scratch.at("Transmittal.pdf");
    a_set_with(&sheet, &[""]);
    let doc = pdf::Document::open(&sheet).expect("open");
    let mut placer = annot::place::Placer::new(&doc).by("Sam");
    let mut pen = hyperview::pen::Pen::default();
    pen.line = [220, 20, 20, 255];
    pen.width = 4.0;
    let mut markup = hyperview::forms::make(
        hyperview::forms::Field::Text,
        "JobNumber",
        [400.0, 400.0, 1600.0, 700.0],
        &pen,
    );
    placer.add(0, &mut markup);
    std::fs::write(&sheet, placer.finish().apply(&doc)).expect("write");

    let png = scratch.at("look.png");
    hyperview::render::picture_of(library(), &sheet, 0, 700, 700, &png).expect("render");
    let drawn = image::open(&png).expect("open").to_rgb8();
    let red = drawn
        .pixels()
        .filter(|p| p[0] > 140 && p[1] < 110 && p[2] < 110)
        .count();
    assert!(red > 40, "the field's border should be drawn: {red}");
}

// ---- two people saving the same drawing -------------------------------------

fn a_box(subject: &str) -> annot::Markup {
    let mut markup = annot::Markup::new(annot::Subtype::Square);
    markup.set_box([100.0, 100.0, 300.0, 200.0]);
    markup.set_subject(subject);
    markup.named();
    markup
}

#[test]
fn two_people_saving_one_drawing_on_the_share_both_keep_their_markups() {
    // Jordan and Sam both have S-101 open off the company share. Sam
    // saves first. When Jordan saves, what Sam put on it must still be
    // there — writing Jordan's copy over Sam's is the failure this is for.
    let scratch = Scratch::new("two-savers");
    let sheet = scratch.at("S-101.pdf");
    a_blank_sheet(&sheet);

    let mut sam = hyperview::sheet::Doc::open(sheet.clone()).expect("open");
    let mut kyler = hyperview::sheet::Doc::open(sheet.clone()).expect("open");

    sam.marks.push(hyperview::sheet::Mark::new(0, a_box("Sam's beam")));
    sam.dirty = true;
    sam.save("Sam").expect("Sam saves");

    // A different size or time on disk is how the second save knows.
    std::thread::sleep(std::time::Duration::from_millis(20));
    kyler.marks.push(hyperview::sheet::Mark::new(0, a_box("Jordan's column")));
    kyler.dirty = true;
    kyler.save("Jordan").expect("Jordan saves");

    let after = hyperview::sheet::Doc::open(sheet.clone()).expect("reopen");
    let subjects: Vec<String> = after.marks.iter().map(|m| m.markup.subject()).collect();
    assert!(subjects.contains(&"Sam's beam".to_string()), "{subjects:?}");
    assert!(subjects.contains(&"Jordan's column".to_string()), "{subjects:?}");
    assert_eq!(subjects.len(), 2, "{subjects:?}");

    // And Sam's next save keeps Jordan's.
    sam.marks.push(hyperview::sheet::Mark::new(0, a_box("Sam's brace")));
    sam.dirty = true;
    sam.save("Sam").expect("Sam saves again");
    let after = hyperview::sheet::Doc::open(sheet).expect("reopen");
    assert_eq!(after.marks.len(), 3);
}

#[test]
fn a_markup_changed_here_and_left_alone_there_comes_out_as_changed_here() {
    let scratch = Scratch::new("changed-here");
    let sheet = scratch.at("S-102.pdf");
    a_blank_sheet(&sheet);

    let mut first = hyperview::sheet::Doc::open(sheet.clone()).expect("open");
    first.marks.push(hyperview::sheet::Mark::new(0, a_box("W12x26")));
    first.dirty = true;
    first.save("Sam").expect("save");

    // Both open it; one relabels the beam, the other adds a column.
    let mut here = hyperview::sheet::Doc::open(sheet.clone()).expect("open");
    let mut there = hyperview::sheet::Doc::open(sheet.clone()).expect("open");
    there.marks.push(hyperview::sheet::Mark::new(0, a_box("HSS6x6x1/4")));
    there.dirty = true;
    there.save("Jordan").expect("save there");

    std::thread::sleep(std::time::Duration::from_millis(20));
    let at = here.marks.iter().position(|m| m.markup.subject() == "W12x26").unwrap();
    here.marks[at].markup.set_subject("W12x30");
    here.marks[at].changed = true;
    here.dirty = true;
    here.save("Sam").expect("save here");

    let after = hyperview::sheet::Doc::open(sheet).expect("reopen");
    let mut subjects: Vec<String> = after.marks.iter().map(|m| m.markup.subject()).collect();
    subjects.sort();
    assert_eq!(subjects, vec!["HSS6x6x1/4".to_string(), "W12x30".to_string()]);
}

#[test]
fn a_save_that_cannot_happen_says_why_in_words() {
    let why = hyperview::sheet::explain(
        Path::new(r"\\server\jobs\2630\S-101.pdf"),
        &std::io::Error::from(std::io::ErrorKind::PermissionDenied),
    );
    assert!(why.contains("S-101.pdf"), "{why}");
    assert!(why.contains("Nothing has been lost"), "{why}");
}

/// Each platform's own dependencies are declared against that platform.
///
/// This is not a style check. `windows` once slipped from `cfg(windows)` into
/// the macOS section when that section was added above it, and TOML being what
/// it is, nothing said a word: Windows quietly lost the crate that half its
/// code imports, and the Windows build stopped compiling. Nobody noticed,
/// because everything else was being built on a Mac that week.
#[test]
fn every_platform_gets_the_crates_its_own_code_imports() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("this crate's own manifest");
    let value: toml::Value = toml::from_str(&manifest).expect("readable TOML");
    let targets = value
        .get("target")
        .and_then(|t| t.as_table())
        .expect("per-platform dependencies");

    let named = |spec: &str, name: &str| -> bool {
        targets
            .get(spec)
            .and_then(|t| t.get("dependencies"))
            .and_then(|d| d.get(name))
            .is_some()
    };

    assert!(
        named("cfg(windows)", "windows"),
        "the `windows` crate is not a Windows dependency; install.rs, instance.rs and ocr.rs \
         all import it and the Windows build will not compile"
    );
    assert!(
        !named("cfg(target_os = \"macos\")", "windows"),
        "the `windows` crate is being pulled into Mac builds, which means it has drifted out \
         of the Windows section again"
    );
    for platform in ["cfg(windows)", "cfg(target_os = \"macos\")", "cfg(target_os = \"linux\")"] {
        assert!(named(platform, "wgpu"), "{platform} has no graphics backend");
    }
}
