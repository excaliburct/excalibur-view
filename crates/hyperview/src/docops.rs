//! Document operations: what Revu's Document menu does.
//!
//! Combining, splitting, extracting, inserting, deleting, rotating, cropping,
//! labelling, stamping headers and footers, slip-sheeting a revision, and
//! flattening markups into the page.
//!
//! Two rules run through every one of them.
//!
//! **Nothing is done in place.** Every operation writes a new file and leaves
//! the one it read alone. A drawing set is the file of record for a bid, and a
//! program that rewrites one because somebody chose the wrong menu item has
//! destroyed the only copy of what was actually issued. Where a name would
//! collide, a number is added rather than anything being overwritten.
//!
//! **Markups travel or they are reported.** The markups are real PDF
//! annotations on those pages, so pages copied between files carry them. Where
//! an operation cannot carry them — and there is exactly one, flattening, which
//! turns them into paint on purpose — it is said plainly before it runs.

use std::path::{Path, PathBuf};

/// One thing to do to one or more drawings.
#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    /// Several files into one, in the order given.
    Combine {
        sources: Vec<PathBuf>,
        to: PathBuf,
    },
    /// One file into several, every `every` sheets.
    Split {
        source: PathBuf,
        into: PathBuf,
        every: usize,
    },
    /// Chosen sheets into a file of their own.
    Extract {
        source: PathBuf,
        pages: Vec<u32>,
        to: PathBuf,
        /// Also take them out of a copy of the original, rather than only
        /// copying them out.
        and_remove: bool,
    },
    /// Another file's sheets into this one, at a position.
    Insert {
        source: PathBuf,
        insert: PathBuf,
        at: usize,
        to: PathBuf,
    },
    Delete {
        source: PathBuf,
        pages: Vec<u32>,
        to: PathBuf,
    },
    /// Turn sheets. Ninety degree steps, which is all a PDF page has.
    Rotate {
        source: PathBuf,
        pages: Vec<u32>,
        quarter_turns: i32,
        to: PathBuf,
    },
    /// Trim the visible area. The content is not thrown away — a crop box is a
    /// window onto the page, and widening it later brings back what was outside
    /// it. That is worth knowing before somebody crops a drawing to a detail.
    Crop {
        source: PathBuf,
        pages: Vec<u32>,
        /// Left, bottom, right, top, in points, in page space.
        box_: [f32; 4],
        to: PathBuf,
    },
    /// Replace sheets with newer ones, matched by sheet number.
    SlipSheet {
        source: PathBuf,
        revisions: PathBuf,
        to: PathBuf,
        /// Keep the sheets that were replaced, at the back, so a bid can still
        /// show what was superseded.
        keep_superseded: bool,
    },
    /// Turn markups into part of the page.
    Flatten {
        source: PathBuf,
        pages: Vec<u32>,
        to: PathBuf,
    },
    /// Stamp text in the corners of every chosen sheet.
    Stamp {
        source: PathBuf,
        pages: Vec<u32>,
        stamps: Vec<crate::stamps::Stamp>,
        to: PathBuf,
    },
    /// Make a set small enough to email, by rendering each sheet.
    ///
    /// This is the one operation that is genuinely lossy, and it is lossy in a
    /// way that matters here: a rendered sheet has no text layer, so search
    /// stops working and sheet numbers stop being read. Said out loud before it
    /// runs, and the original is still the original.
    Shrink {
        source: PathBuf,
        to: PathBuf,
        /// Dots per inch to render at. 150 is readable on screen; 300 prints.
        dpi: u32,
    },
    /// Read the words off scanned sheets and put them back invisibly, so the
    /// set can be searched and its sheet numbers read.
    Ocr {
        source: PathBuf,
        pages: Vec<u32>,
        to: PathBuf,
        dpi: u32,
        /// Leave alone any sheet that already has text on it.
        only_scans: bool,
    },
    /// Compare every sheet against the matching sheet of an older issue, and
    /// cloud what changed on a copy of the newer one.
    Compare {
        newer: PathBuf,
        older: PathBuf,
        to: PathBuf,
        darkness: f32,
    },
    /// Put the engineer's seal on chosen sheets.
    Seal {
        source: PathBuf,
        /// A PNG or JPEG of the seal, and usually the signature with it.
        picture: PathBuf,
        pages: Vec<u32>,
        placement: crate::seal::Placement,
        to: PathBuf,
    },
    /// Rebuild a file whose cross-reference table no longer matches its
    /// contents, by finding every object in it and writing a fresh one.
    Repair {
        source: PathBuf,
        to: PathBuf,
    },
    /// Write each sheet's own number into the file as its page label, so
    /// every other reader shows S-201 where Hyperview does.
    PageLabels {
        source: PathBuf,
        /// Sheet index and the label it should carry.
        labels: Vec<(u32, String)>,
        to: PathBuf,
    },
    /// Lift markups Hyperview flattened back out into markups again.
    Unflatten {
        source: PathBuf,
        to: PathBuf,
    },
    /// Take out what is under every redaction mark, for good.
    ApplyRedactions {
        source: PathBuf,
        to: PathBuf,
        /// Turn each affected sheet into a picture rather than removing the
        /// objects under the box. Slower and heavier, and the only way to be
        /// certain nothing of the words is left behind.
        as_pictures: bool,
        dpi: u32,
    },
    /// Lay one issue of a set over another, sheet for sheet, in two colours.
    Overlay {
        newer: PathBuf,
        older: PathBuf,
        to: PathBuf,
        darkness: f32,
    },
    /// Lay one sheet over another and write the overlay as a one-page file.
    OverlaySheet { // WHAT: one sheet's overlay written as a file. WHY: Save as PDF in the overlay window.
        newer: PathBuf,                    // WHAT: the newer issue. WHY: its sheet is the overlay's paper.
        newer_page: u32,                   // WHAT: which of its sheets. WHY: the same.
        older: PathBuf,                    // WHAT: the older issue. WHY: laid over it.
        older_page: u32,                   // WHAT: which of its sheets. WHY: the same.
        darkness: f32,                     // WHAT: how dark is line-work. WHY: a faint scan needs it higher.
        lining: crate::overlay::Lining,    // WHAT: how they are lined up. WHY: found, by two points, or as they are.
        to: PathBuf,                       // WHAT: where to write it. WHY: a new file; neither issue is touched.
    },
    /// Send a set to the plotter.
    Print {
        source: PathBuf,
    },
    /// One takeoff report covering several sets.
    Summary {
        sources: Vec<PathBuf>,
        to: PathBuf,
    },
    /// A new drawing with nothing on it.
    NewBlank {
        to: PathBuf,
        sheets: usize,
        /// The paper, in points.
        size: [f32; 2],
    },
    /// A drawing made from pictures: one sheet per picture, each sized to the
    /// picture it holds.
    FromPictures {
        pictures: Vec<PathBuf>,
        to: PathBuf,
        /// Dots per inch to treat each picture as, which decides how big its
        /// sheet is.
        dpi: u32,
    },
    /// Lock a set with a password, or take the lock off.
    Secure {
        source: PathBuf,
        to: PathBuf,
        /// Empty means anybody may open it without being asked.
        open_password: String,
        /// Empty means the open password is also the owner's.
        owner_password: String,
        allowed: pdf::crypt::Allowed,
    },
    /// Write the sheets out as pictures.
    ExportPictures {
        source: PathBuf,
        pages: Vec<u32>,
        into: PathBuf,
        dpi: u32,
        /// `png` or `jpg`.
        format: String,
    },
}

impl Operation {
    /// What this will produce. Shown before anything runs, because the answer
    /// to "what is about to happen to my drawings" should not be "try it".
    pub fn describe(&self) -> String {
        match self {
            Operation::Combine { sources, to } => format!(
                "Combine {} file{} into {}.",
                sources.len(),
                if sources.len() == 1 { "" } else { "s" },
                name(to)
            ),
            Operation::Split { every, .. } => format!(
                "Split into files of {every} sheet{}.",
                if *every == 1 { "" } else { "s" }
            ),
            Operation::Extract { pages, to, and_remove, .. } => format!(
                "{} {} sheet{} into {}.",
                if *and_remove { "Move" } else { "Copy" },
                pages.len(),
                if pages.len() == 1 { "" } else { "s" },
                name(to)
            ),
            Operation::Insert { insert, at, .. } => {
                format!("Insert {} after sheet {at}.", name(insert))
            }
            Operation::Delete { pages, .. } => format!(
                "Leave out {} sheet{}.",
                pages.len(),
                if pages.len() == 1 { "" } else { "s" }
            ),
            Operation::Rotate { pages, quarter_turns, .. } => format!(
                "Turn {} sheet{} {}.",
                pages.len(),
                if pages.len() == 1 { "" } else { "s" },
                turn_in_words(*quarter_turns)
            ),
            Operation::Crop { pages, .. } => format!(
                "Crop {} sheet{}. Nothing is thrown away — a wider crop brings it back.",
                pages.len(),
                if pages.len() == 1 { "" } else { "s" }
            ),
            Operation::SlipSheet { revisions, keep_superseded, .. } => format!(
                "Slip in the sheets from {}, matched by sheet number.{}",
                name(revisions),
                if *keep_superseded {
                    " The ones they replace move to the back."
                } else {
                    " The ones they replace come out."
                }
            ),
            Operation::Flatten { pages, .. } => format!(
                "Flatten the markups on {} sheet{} into the page. \
                 They stop being markups: no list, no totals, no editing. \
                 The original file still has them.",
                pages.len(),
                if pages.len() == 1 { "" } else { "s" }
            ),
            Operation::Stamp { pages, stamps, .. } => format!(
                "Stamp {} piece{} of text on {} sheet{}.",
                stamps.len(),
                if stamps.len() == 1 { "" } else { "s" },
                pages.len(),
                if pages.len() == 1 { "" } else { "s" }
            ),
            Operation::Shrink { dpi, .. } => format!(
                "Render every sheet at {dpi} dpi to make the file smaller."
            ),
            Operation::Compare { older, .. } => format!(
                "Compare every sheet against {} and cloud what changed.",
                name(older)
            ),
            Operation::Repair { source, .. } => format!(
                "Rebuild {} from the objects actually in it. Nothing is thrown \
                 away — what cannot be found is reported.",
                name(source)
            ),
            Operation::PageLabels { labels, .. } => format!(
                "Write {} sheet number{} into the file as page labels, so every \
                 other reader shows them too.",
                labels.len(),
                if labels.len() == 1 { "" } else { "s" }
            ),
            Operation::Unflatten { source, .. } => format!(
                "Lift the markups Excalibur View flattened in {} back out. Markups \
                 flattened by another program cannot be lifted, and that is \
                 said rather than guessed at.",
                name(source)
            ),
            Operation::ApplyRedactions { as_pictures, .. } => {
                if *as_pictures {
                    "Take out everything under every redaction mark by turning \
                     those sheets into pictures. Nothing of the words is left \
                     behind, and those sheets stop being searchable."
                        .into()
                } else {
                    "Take out everything under every redaction mark. Anything \
                     that reaches outside a box goes too, and how much is \
                     reported."
                        .into()
                }
            }
            Operation::NewBlank { sheets, to, .. } => format!(
                "A new drawing of {sheets} blank sheet{}, written to {}.",
                if *sheets == 1 { "" } else { "s" },
                name(to)
            ),
            Operation::FromPictures { pictures, to, .. } => format!(
                "{} picture{} into {}, one sheet each.",
                pictures.len(),
                if pictures.len() == 1 { "" } else { "s" },
                name(to)
            ),
            Operation::Secure { open_password, allowed, .. } => {
                let mut said = if open_password.is_empty() {
                    "Lock the set so a reader is asked to honour the permissions, \
                     without asking anybody for a password to open it. "
                        .to_string()
                } else {
                    "Lock the set so it cannot be opened without the password. "
                        .to_string()
                };
                said.push_str(&allowed.in_words());
                said
            }
            Operation::Overlay { older, .. } => format!(
                "Lay every sheet over the matching sheet of {} and write the two \
                 in different colours, so what moved shows as colour and what did \
                 not shows as dark.",
                name(older)
            ),
            Operation::OverlaySheet { older, older_page, newer_page, .. } => format!("Lay sheet {} of {} over sheet {} of this drawing and write it as a new file.", older_page + 1, name(older), newer_page + 1), // WHAT: what it will do. WHY: the same plain line every operation has.
            Operation::Print { source } => {
                format!("Send {} to the plotter.", name(source))
            }
            Operation::Summary { sources, to } => format!(
                "One takeoff report covering {} set{}, written to {}.",
                sources.len(),
                if sources.len() == 1 { "" } else { "s" },
                name(to)
            ),
            Operation::ExportPictures { pages, format, dpi, .. } => format!(
                "Write {} sheet{} out as {} files at {dpi} dpi.",
                pages.len(),
                if pages.len() == 1 { "" } else { "s" },
                format.to_uppercase()
            ),
            Operation::Seal { pages, .. } => format!(
                "Put the seal on {} sheet{}.",
                pages.len(),
                if pages.len() == 1 { "" } else { "s" }
            ),
            Operation::Ocr { pages, only_scans, .. } => format!(
                "Read the words off {} sheet{}{}. The drawing will look exactly the \
                 same — the words go in invisibly, underneath.",
                pages.len(),
                if pages.len() == 1 { "" } else { "s" },
                if *only_scans {
                    ", skipping any that already have text"
                } else {
                    ""
                }
            ),
        }
    }

    /// Whether this one needs saying twice. Flattening is the only operation
    /// here that loses something, and it loses the thing this program is for.
    pub fn needs_a_warning(&self) -> Option<&'static str> {
        match self {
            Operation::Flatten { .. } => Some(
                "Flattened markups are paint. They will not appear in the markups list, \
                 they will not be counted in a takeoff, and nobody can edit or delete them \
                 afterwards. This writes a new file — the one you have open keeps its \
                 markups.",
            ),
            Operation::Seal { .. } => Some(
                "This puts a picture of the seal on the sheets. It is not a cryptographic \
                 digital signature, and Excalibur View does not add one — if a submittal \
                 requires a digitally signed PDF, seal it here and sign it with whatever \
                 your engineer already uses.",
            ),
            Operation::Shrink { .. } => Some(
                "Every sheet becomes a picture of itself. There is no text left to search, \
                 sheet numbers can no longer be read off the drawings, and markups become \
                 part of the page. Use it for emailing a set to somebody, not for the copy \
                 you work from. The original is untouched.",
            ),
            Operation::Secure { open_password, .. } if !open_password.is_empty() => Some(
                "There is no way back into this file without the password. Excalibur View \
                 does not keep a copy of it and nobody can recover it — not this \
                 program, not Adobe, not anybody. Write it down somewhere before you \
                 send the set. The original is untouched.",
            ),
            Operation::ApplyRedactions { as_pictures, .. } => Some(if *as_pictures {
                "Every sheet with a redaction mark on it becomes a picture. That is \
                 the only way to be certain nothing of the words is left in the file \
                 — and it means those sheets stop being searchable, and their sheet \
                 numbers stop being read off the drawing. The original is untouched."
            } else {
                "What is under a redaction mark comes out of the file for good, and \
                 so does anything that reaches outside the box. There is no undoing \
                 it in the file that is written. The original is untouched."
            }),
            _ => None,
        }
    }

    /// Where the result goes.
    pub fn output(&self) -> Option<&Path> {
        match self {
            Operation::Combine { to, .. }
            | Operation::Extract { to, .. }
            | Operation::Insert { to, .. }
            | Operation::Delete { to, .. }
            | Operation::Rotate { to, .. }
            | Operation::Crop { to, .. }
            | Operation::SlipSheet { to, .. }
            | Operation::Flatten { to, .. }
            | Operation::Stamp { to, .. }
            | Operation::Shrink { to, .. }
            | Operation::Ocr { to, .. }
            | Operation::Seal { to, .. }
            | Operation::Compare { to, .. }
            | Operation::Repair { to, .. }
            | Operation::PageLabels { to, .. }
            | Operation::Unflatten { to, .. }
            | Operation::ApplyRedactions { to, .. }
            | Operation::Overlay { to, .. }
            | Operation::OverlaySheet { to, .. } // WHAT: one sheet's overlay writes a file too. WHY: so it is listed with the others that do.
            | Operation::Secure { to, .. }
            | Operation::NewBlank { to, .. }
            | Operation::FromPictures { to, .. }
            | Operation::Summary { to, .. } => Some(to),
            Operation::Split { .. }
            | Operation::ExportPictures { .. }
            | Operation::Print { .. } => None,
        }
    }
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn turn_in_words(quarter_turns: i32) -> &'static str {
    match quarter_turns.rem_euclid(4) {
        1 => "a quarter turn clockwise",
        2 => "upside down",
        3 => "a quarter turn anticlockwise",
        _ => "not at all",
    }
}

/// What came of one.
#[derive(Clone, Debug)]
pub struct Done {
    pub wrote: Vec<PathBuf>,
    /// One line somebody can read, and put in an email.
    pub said: String,
}

// ---- not overwriting anybody's work ---------------------------------------

/// A path that is not already a file.
///
/// The alternative is asking somebody "overwrite?" at the end of an operation
/// they have already decided to run, which is a question nobody reads
/// carefully, on the one occasion where getting it wrong destroys a drawing
/// set. So this never asks, and never overwrites.
/// Puts one bookmark per source file into a combined set, named after the
/// file and opening at its first sheet, so a stapled set can still be found
/// round in. `files` is each source's name and how many sheets it brought.
/// Written as an update on the end of the file; nothing already in it moves.
pub fn bookmark_each_file(path: &Path, files: &[(String, usize)]) -> Result<(), String> {
    let file = pdf::Document::open(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let pages = file.pages();
    let starts = run_starts(&files.iter().map(|(_, count)| *count).collect::<Vec<_>>());
    let mut update = pdf::write::Update::new(&file);
    let refs: Vec<pdf::Ref> = files.iter().map(|_| update.add(pdf::Object::Null)).collect();
    let outlines_ref = update.add(pdf::Object::Null);
    for (at, (title, _)) in files.iter().enumerate() {
        let Some(page_ref) = starts.get(at).and_then(|start| pages.get(*start)).copied() else {
            continue;
        };
        let mut entry = pdf::Dict::new();
        entry.set("Title", pdf::Object::text(title));
        entry.set("Parent", pdf::Object::Ref(outlines_ref));
        if at > 0 {
            entry.set("Prev", pdf::Object::Ref(refs[at - 1]));
        }
        if at + 1 < refs.len() {
            entry.set("Next", pdf::Object::Ref(refs[at + 1]));
        }
        entry.set(
            "Dest",
            pdf::Object::Array(vec![pdf::Object::Ref(page_ref), pdf::Object::name("Fit")]),
        );
        update.replace(refs[at], pdf::Object::Dict(entry));
    }
    let mut outlines = pdf::Dict::new();
    outlines.set("Type", pdf::Object::name("Outlines"));
    if let (Some(first), Some(last)) = (refs.first(), refs.last()) {
        outlines.set("First", pdf::Object::Ref(*first));
        outlines.set("Last", pdf::Object::Ref(*last));
    }
    outlines.set("Count", pdf::Object::Int(refs.len() as i64));
    update.replace(outlines_ref, pdf::Object::Dict(outlines));
    let Some(catalog_ref) = file.xref.trailer.get("Root").and_then(|o| o.as_ref()) else {
        return Err("the combined file has no catalog".into());
    };
    let mut catalog = file.catalog();
    catalog.set("Outlines", pdf::Object::Ref(outlines_ref));
    catalog.set("PageMode", pdf::Object::name("UseOutlines"));
    update.replace(catalog_ref, pdf::Object::Dict(catalog));
    std::fs::write(path, update.apply(&file)).map_err(|e| format!("could not write {}: {e}", path.display()))
}

pub fn free_name(wanted: &Path) -> PathBuf {
    if !wanted.exists() {
        return wanted.to_path_buf();
    }
    let parent = wanted.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = wanted
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "drawing".into());
    let ext = wanted
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 2..10_000 {
        let candidate = parent.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // Ten thousand of them. Something is wrong, but still do not overwrite.
    parent.join(format!("{stem} ({}){ext}", now_stamp()))
}

fn now_stamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A sensible name for what an operation produces, beside the file it came
/// from — because that is where somebody will look for it.
pub fn beside(source: &Path, suffix: &str) -> PathBuf {
    let parent = source.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "drawing".into());
    free_name(&parent.join(format!("{stem} {suffix}.pdf")))
}

// ---- which sheets ---------------------------------------------------------

/// Everything except the listed sheets, in order. What a delete leaves behind.
pub fn all_but(count: usize, leave_out: &[u32]) -> Vec<u32> {
    (0..count as u32)
        .filter(|p| !leave_out.contains(p))
        .collect()
}

/// Where each file's sheets start, when several are put end to end.
pub fn run_starts(counts: &[usize]) -> Vec<usize> {
    let mut at = 0;
    let mut out = Vec::with_capacity(counts.len());
    for count in counts {
        out.push(at);
        at += count;
    }
    out
}

/// Splits a count into runs of at most `every`.
pub fn in_runs_of(count: usize, every: usize) -> Vec<Vec<u32>> {
    if every == 0 || count == 0 {
        return Vec::new();
    }
    (0..count)
        .step_by(every)
        .map(|from| ((from as u32)..((from + every).min(count) as u32)).collect())
        .collect()
}

// ---- slip sheeting --------------------------------------------------------

/// Which sheets a revision replaces, matched by sheet number.
///
/// This is the operation a fabricator does most and the one it is worst to get
/// wrong: a revised S-201 arrives and has to replace the S-201 already in the
/// set, not sit next to it and not replace S-2010. So matching is exact after
/// tidying, never a prefix, and anything in the revisions that matches nothing
/// is **reported rather than quietly appended** — a sheet number that does not
/// match is usually a typo in a sheet number, and appending it hides that.
pub struct SlipPlan {
    /// (page in the original, page in the revisions) — what replaces what.
    pub replacing: Vec<(u32, u32)>,
    /// Sheets in the revisions that matched nothing in the original.
    pub unmatched: Vec<(u32, String)>,
    /// Sheets in the original nothing replaced. Not a problem — most of a set.
    pub untouched: usize,
}

impl SlipPlan {
    pub fn says(&self) -> String {
        let mut said = format!(
            "{} sheet{} replaced",
            self.replacing.len(),
            if self.replacing.len() == 1 { "" } else { "s" }
        );
        if !self.unmatched.is_empty() {
            let names: Vec<&str> = self
                .unmatched
                .iter()
                .map(|(_, number)| number.as_str())
                .collect();
            said.push_str(&format!(
                ". {} in the revisions matched no sheet in the set: {}. \
                 Check those sheet numbers — nothing was added for them.",
                self.unmatched.len(),
                names.join(", ")
            ));
        } else {
            said.push('.');
        }
        said
    }
}

/// Tidies a sheet number for matching: case and the spaces and dashes people
/// put in differently on different issues of the same drawing.
pub fn tidy_number(number: &str) -> String {
    number
        .trim()
        .to_uppercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

pub fn plan_slip(original: &[String], revisions: &[String]) -> SlipPlan {
    let tidy: Vec<String> = original.iter().map(|n| tidy_number(n)).collect();
    let mut replacing = Vec::new();
    let mut unmatched = Vec::new();
    let mut replaced = vec![false; original.len()];

    for (from, number) in revisions.iter().enumerate() {
        let wanted = tidy_number(number);
        if wanted.is_empty() {
            unmatched.push((from as u32, "(no sheet number)".to_string()));
            continue;
        }
        // The last match, not the first: a set that already contains two S-201s
        // has the later one as the current one.
        match tidy.iter().rposition(|n| *n == wanted) {
            Some(at) => {
                replaced[at] = true;
                replacing.push((at as u32, from as u32));
            }
            None => unmatched.push((from as u32, number.trim().to_string())),
        }
    }
    replacing.sort_by_key(|(at, _)| *at);
    SlipPlan {
        untouched: replaced.iter().filter(|r| !**r).count(),
        replacing,
        unmatched,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_ever_written_over() {
        let dir = std::env::temp_dir().join(format!("hyperview-ops-{}", now_stamp()));
        std::fs::create_dir_all(&dir).unwrap();
        let taken = dir.join("Set.pdf");
        std::fs::write(&taken, b"already here").unwrap();

        let fresh = free_name(&taken);
        assert_ne!(fresh, taken);
        assert!(fresh.to_string_lossy().contains("(2)"));
        // And the one that was there is untouched.
        assert_eq!(std::fs::read(&taken).unwrap(), b"already here");

        std::fs::write(&fresh, b"second").unwrap();
        let third = free_name(&taken);
        assert!(third.to_string_lossy().contains("(3)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_free_name_keeps_the_extension_where_windows_expects_it() {
        // "Set (2).pdf", not "Set.pdf (2)", because the second is not a PDF as
        // far as Windows is concerned.
        let dir = std::env::temp_dir().join(format!("hyperview-ext-{}", now_stamp()));
        std::fs::create_dir_all(&dir).unwrap();
        let taken = dir.join("Structural Stamped.pdf");
        std::fs::write(&taken, b"x").unwrap();
        let fresh = free_name(&taken);
        assert_eq!(fresh.extension().unwrap(), "pdf");
        assert!(fresh.file_stem().unwrap().to_string_lossy().ends_with("(2)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_delete_leaves_everything_else_in_order() {
        assert_eq!(all_but(6, &[1, 3]), vec![0, 2, 4, 5]);
        assert_eq!(all_but(3, &[]), vec![0, 1, 2]);
        assert_eq!(all_but(3, &[0, 1, 2]), Vec::<u32>::new());
        // A sheet listed twice is still only one sheet left out.
        assert_eq!(all_but(3, &[1, 1]), vec![0, 2]);
    }

    #[test]
    fn splitting_makes_runs_and_the_last_one_is_short() {
        assert_eq!(in_runs_of(5, 2), vec![vec![0, 1], vec![2, 3], vec![4]]);
        assert_eq!(in_runs_of(4, 2), vec![vec![0, 1], vec![2, 3]]);
        assert_eq!(in_runs_of(3, 10), vec![vec![0, 1, 2]]);
        // Nonsense does not panic and does not make a file.
        assert!(in_runs_of(5, 0).is_empty());
        assert!(in_runs_of(0, 3).is_empty());
    }

    #[test]
    fn combining_knows_where_each_set_starts() {
        assert_eq!(run_starts(&[12, 4, 30]), vec![0, 12, 16]);
        assert_eq!(run_starts(&[]), Vec::<usize>::new());
    }

    #[test]
    fn a_revision_replaces_the_sheet_with_the_same_number() {
        let set = vec!["S-100".into(), "S-201".into(), "S-300".into()];
        let revisions = vec!["S-201".into()];
        let plan = plan_slip(&set, &revisions);
        assert_eq!(plan.replacing, vec![(1, 0)]);
        assert!(plan.unmatched.is_empty());
        assert_eq!(plan.untouched, 2);
    }

    #[test]
    fn a_sheet_number_typed_differently_on_the_revision_still_matches() {
        // The same drawing, issued twice, by two people.
        let set = vec!["S-201".into()];
        let revisions = vec![" s-201 ".into()];
        assert_eq!(plan_slip(&set, &revisions).replacing, vec![(0, 0)]);
    }

    #[test]
    fn a_revision_that_matches_nothing_is_reported_and_not_quietly_added() {
        // Because "S-2011" in a revision set is almost always a typo for
        // "S-201", and appending it gives a bid two sheets where there is one.
        let set = vec!["S-100".into(), "S-201".into()];
        let revisions = vec!["S-2011".into()];
        let plan = plan_slip(&set, &revisions);
        assert!(plan.replacing.is_empty());
        assert_eq!(plan.unmatched.len(), 1);
        assert!(plan.says().contains("S-2011"));
        assert!(plan.says().contains("nothing was added"));
    }

    #[test]
    fn a_near_miss_is_not_treated_as_a_match() {
        // S-201 must not match S-2010, in either direction.
        let set = vec!["S-2010".into()];
        assert!(plan_slip(&set, &vec!["S-201".into()]).replacing.is_empty());
        let set = vec!["S-201".into()];
        assert!(plan_slip(&set, &vec!["S-2010".into()]).replacing.is_empty());
    }

    #[test]
    fn a_set_with_the_same_sheet_twice_replaces_the_later_one() {
        let set = vec!["S-201".into(), "S-300".into(), "S-201".into()];
        let plan = plan_slip(&set, &vec!["S-201".into()]);
        assert_eq!(plan.replacing, vec![(2, 0)]);
    }

    #[test]
    fn a_sheet_with_no_number_is_reported_rather_than_matched_to_anything() {
        let set = vec!["S-100".into(), "".into()];
        let plan = plan_slip(&set, &vec!["".into()]);
        assert!(plan.replacing.is_empty());
        assert_eq!(plan.unmatched.len(), 1);
    }

    #[test]
    fn flattening_is_the_one_that_warns() {
        let flatten = Operation::Flatten {
            source: PathBuf::from("a.pdf"),
            pages: vec![0],
            to: PathBuf::from("b.pdf"),
        };
        let warning = flatten.needs_a_warning().expect("this one warns");
        assert!(warning.contains("not be counted"));
        // And nothing else does, because nothing else loses anything.
        let rotate = Operation::Rotate {
            source: PathBuf::from("a.pdf"),
            pages: vec![0],
            quarter_turns: 1,
            to: PathBuf::from("b.pdf"),
        };
        assert!(rotate.needs_a_warning().is_none());
    }

    #[test]
    fn every_operation_says_what_it_will_do_before_it_does_it() {
        let ops = [
            Operation::Combine {
                sources: vec!["a.pdf".into(), "b.pdf".into()],
                to: "both.pdf".into(),
            },
            Operation::Split { source: "a.pdf".into(), into: ".".into(), every: 1 },
            Operation::Extract {
                source: "a.pdf".into(),
                pages: vec![0, 1],
                to: "some.pdf".into(),
                and_remove: false,
            },
            Operation::Delete { source: "a.pdf".into(), pages: vec![0], to: "b.pdf".into() },
            Operation::Rotate {
                source: "a.pdf".into(),
                pages: vec![0],
                quarter_turns: 1,
                to: "b.pdf".into(),
            },
        ];
        for op in ops {
            let said = op.describe();
            assert!(!said.is_empty());
            // A sentence, not a label.
            assert!(said.ends_with('.'), "{said}");
        }
    }

    #[test]
    fn a_turn_is_described_the_way_somebody_would_say_it() {
        assert_eq!(turn_in_words(1), "a quarter turn clockwise");
        assert_eq!(turn_in_words(2), "upside down");
        assert_eq!(turn_in_words(3), "a quarter turn anticlockwise");
        assert_eq!(turn_in_words(-1), "a quarter turn anticlockwise");
        assert_eq!(turn_in_words(4), "not at all");
    }
}
