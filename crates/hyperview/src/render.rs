//! The PDF engine: one background thread that owns Pdfium and answers requests
//! for tiles, page previews, thumbnails and sheet labels.
//!
//! Pdfium is not thread safe, so exactly one thread ever touches it. The UI
//! thread never blocks on it — it posts what it wants drawn and keeps painting
//! whatever it already has.

use std::path::PathBuf;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use pdfium_render::prelude::*;

/// Tiles are square in raster pixels. 512 keeps a full 42x30 sheet at 1:1 down
/// to 30 tiles while staying small enough that a stale tile is cheap to throw away.
pub const TILE: u32 = 512;

/// Zoom buckets are whole powers of two. A tile is always rendered at or above
/// the size it will be drawn at, so lines stay crisp instead of being upscaled.
pub const MIN_BUCKET: i32 = -3;
pub const MAX_BUCKET: i32 = 5;

pub fn bucket_scale(bucket: i32) -> f64 {
    2f64.powi(bucket)
}

/// Picks the bucket to render at for a given on-screen zoom.
pub fn bucket_for_zoom(zoom: f32) -> i32 {
    let z = zoom.max(1e-6) as f64;
    // The -0.08 lets a zoom a hair above a power of two keep using that bucket
    // rather than jumping to four times the work for an invisible difference.
    (z.log2() - 0.08).ceil() as i32
}

/// Where a tile sits in raster pixels at its bucket, and how big it is:
/// `(x0, y0, width, height)`.
///
/// Tiles along the right and bottom of a sheet are cut short by the page edge,
/// and their image is exactly this size — not 512 square. The renderer makes
/// them and the view draws them from this one function, so the two cannot
/// disagree about it again: when they did, the last column and row of every
/// sheet were drawn from a fraction of their image, stretched, and a letter
/// page lost its right-hand column and its bottom third.
pub fn tile_extent(size: PageSize, bucket: i32, tx: u32, ty: u32) -> Option<(f64, f64, u32, u32)> {
    let scale = bucket_scale(bucket);
    let full_w = (size.width as f64 * scale).round().max(1.0);
    let full_h = (size.height as f64 * scale).round().max(1.0);
    let x0 = tx as f64 * TILE as f64;
    let y0 = ty as f64 * TILE as f64;
    if x0 >= full_w || y0 >= full_h {
        return None;
    }
    let w = (full_w - x0).min(TILE as f64) as u32;
    let h = (full_h - y0).min(TILE as f64) as u32;
    Some((x0, y0, w, h))
}

/// How much of a tile's texture to draw, given what the tile covers and how
/// big its texture actually is. A texture made to the tile's own size is drawn
/// whole; one made 512 square for an edge tile is drawn in part.
pub fn tile_uv(covers: (u32, u32), texture: [usize; 2]) -> [f32; 2] {
    let across = texture[0].max(1) as f32;
    let down = texture[1].max(1) as f32;
    [
        (covers.0 as f32 / across).min(1.0),
        (covers.1 as f32 / down).min(1.0),
    ]
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TileKey {
    /// Which open drawing. Tabs mean several are open at once, and a tile
    /// belongs to one of them.
    pub doc: u64,
    pub page: u32,
    pub bucket: i32,
    pub tx: u32,
    pub ty: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug, Default)]
pub struct SheetLabel {
    pub number: String,
    pub title: String,
    /// The grid bubbles along this sheet's edges, when it plainly has some.
    ///
    /// It rides along with the sheet's name because reading it costs nothing
    /// extra: the same pass over the same characters answers both questions,
    /// and the app is already waiting for one of them.
    pub grid: takeoff::grid::Grid,
}

impl SheetLabel {
    pub fn is_empty(&self) -> bool {
        self.number.is_empty() && self.title.is_empty()
    }
}

pub enum ToWorker {
    /// Open a drawing under an id the window chose. Opening the same id again
    /// replaces what was there.
    Open {
        doc: u64,
        path: PathBuf,
        /// For a locked set. Empty for the ordinary case, which is also what
        /// opens a file whose only password is the owner's.
        password: String,
    },
    /// Let go of one. Six sets open at once is a lot of mapped file.
    Close(u64),
    /// Read a drawing again after its markups were saved into it, so the
    /// sheet is drawn with them where they are now rather than where they
    /// were when it was opened. Nothing else about the drawing changes.
    Refresh { doc: u64 },
    /// Find the paragraphs of words on a sheet, for changing them.
    Paragraphs { doc: u64, page: u32 },
    /// Write one of them back, changed.
    Rewrite {
        doc: u64,
        job: u64,
        page: u32,
        found: Box<crate::textedit::Found>,
        rewrite: Box<crate::textedit::Rewrite>,
    },
    /// The complete set of tiles the view wants right now, most important first.
    /// Replaces any previous list, so nothing off-screen is ever rendered.
    Want { tiles: Vec<TileKey> },
    Preview { doc: u64, page: u32 },
    Thumbs { doc: u64, pages: Vec<u32> },
    ScanLabels(u64),
    /// Find words on every sheet. Replaces whatever search was running.
    /// VisualSearch: find everywhere on a sheet that looks like a symbol.
    Symbols {
        doc: u64,
        job: u64,
        /// The sheet holding the symbol, and the sheets to look on.
        from_page: u32,
        pages: Vec<u32>,
        /// The symbol, in sheet coordinates.
        symbol: [f64; 4],
        /// How alike, nought to one.
        alike: f32,
        darkness: f32,
    },
    /// Dynamic Fill: find the shape of a bounded region from a point inside it.
    DynamicFill {
        doc: u64,
        page: u32,
        job: u64,
        /// The part of the sheet to look at, in sheet coordinates. The fill is
        /// not allowed outside it, and reaching its edge means the region is
        /// open.
        area: [f64; 4],
        /// Where somebody clicked, in sheet coordinates.
        seed: [f64; 2],
        /// Lines drawn to close a doorway, in sheet coordinates.
        strokes: Vec<Vec<[f64; 2]>>,
        /// How dark a pixel has to be to count as a line, 0 to 1.
        darkness: f32,
    },
    /// Pull some sheets out into a file of their own, for printing.
    ///
    /// Done here rather than in the window because pdfium is not something to
    /// have two of, and because copying pages through pdfium brings the
    /// annotations with them — the markups are real annotations on those pages,
    /// so what comes out is what somebody would see on paper.
    Extract {
        doc: u64,
        job: u64,
        /// Which sheets, zero-based, in the order they should come out.
        pages: Vec<u32>,
        to: PathBuf,
    },
    /// Read a sheet's line-work, for snapping onto.
    Geometry { doc: u64, page: u32 },
    /// Read a sheet's words and line-work for a plugin.
    Read {
        doc: u64,
        page: u32,
        job: u64,
        words: bool,
        lines: bool,
    },
    /// Print a file of sheets straight to a printer, a sheet at a time.
    PrintFile {
        job: u64,
        path: PathBuf,
        printer: String,
        settings: Vec<u8>,
        fit: crate::winprint::Fit,
        orientation: crate::winprint::Orientation,
        title: String,
    },
    /// Stop the print job that is running.
    CancelPrint(u64),
    /// Compare two issues of a sheet and say what moved.
    Compare {
        job: u64,
        older: PathBuf,
        older_page: u32,
        newer: PathBuf,
        newer_page: u32,
        darkness: f32,
    },
    /// Lay two issues of a sheet over each other in different colours.
    Overlay {
        job: u64,
        older: PathBuf,
        older_page: u32,
        newer: PathBuf,
        newer_page: u32,
        darkness: f32,
        lining: crate::overlay::Lining, // WHAT: found, by two points, or as they are. WHY: the overlay window offers all three.
    },
    /// Draw two sheets small, side by side, for picking points on.
    OverlayLooks { // WHAT: ask for both sheets drawn small. WHY: to pick points on.
        job: u64,         // WHAT: which request. WHY: an answer to an old one is dropped.
        older: PathBuf,   // WHAT: the older file. WHY: drawn.
        older_page: u32,  // WHAT: its sheet. WHY: the same.
        newer: PathBuf,   // WHAT: the newer file. WHY: drawn.
        newer_page: u32,  // WHAT: its sheet. WHY: the same.
    },
    /// Do one job to a list of drawings, reporting each.
    Batch {
        job: u64,
        work: Box<crate::batch::Job>,
        files: Vec<PathBuf>,
    },
    /// Do something to whole drawings: combine, split, insert, rotate, crop,
    /// slip-sheet, flatten. Every one writes a new file and leaves what it read
    /// alone.
    Document {
        job: u64,
        op: Box<crate::docops::Operation>,
    },
    Find {
        doc: u64,
        /// Rises by one each time, so answers from a search the user has
        /// already moved on from are thrown away rather than shown.
        job: u64,
        needle: String,
        whole_words: bool,
        match_case: bool,
    },
    Quit,
}

/// Turns a rendered picture into ink and paper.
///
/// A plain average rather than a weighted luminance, and the same cut as
/// Dynamic Fill uses, so what stops a fill is what a search sees. A coloured
/// markup somebody has already put on the sheet is ink too, which is right: it
/// is on the drawing.
fn to_ink(image: &egui::ColorImage, darkness: f32) -> Vec<bool> {
    let cut = (darkness.clamp(0.05, 0.99) * 255.0) as u32;
    image
        .pixels
        .iter()
        .map(|p| (p.r() as u32 + p.g() as u32 + p.b() as u32) / 3 < cut)
        .collect()
}

/// What came of a Dynamic Fill.
#[derive(Clone, Debug)]
pub enum FillOutcome {
    Found {
        /// In sheet coordinates.
        outline: Vec<[f64; 2]>,
        holes: Vec<Vec<[f64; 2]>>,
        /// Square sheet points, from the cells that were filled — the exact
        /// count, not the simplified outline. This is the number to trust.
        area: f64,
        /// Square sheet points the outline covers but the fill did not:
        /// columns, shafts, anything it went round. The outline on its own
        /// measures `area + enclosed`, so this has to be cut out.
        enclosed: f64,
        /// Enclosed lumps too small to draw. Still counted in `enclosed`.
        not_drawn: usize,
        /// How big one cell was, in sheet points. Anything finer than this is
        /// below what the fill could see.
        resolution: f64,
    },
    /// It got out, and no area is reported.
    Escaped(&'static str),
    /// The click was on a line rather than inside anything.
    OnALine,
    /// The sheet could not be drawn to look at.
    CouldNotLook,
}

/// One place a search found what it was looking for.
#[derive(Clone, Debug)]
pub struct Hit {
    pub page: u32,
    /// The words around it, so the list reads like a sentence rather than a
    /// column of the same word.
    pub context: String,
    /// Where the needle sits inside `context`, in bytes.
    pub at: std::ops::Range<usize>,
    /// On the sheet, in the space the screen works in: top-left origin, y down.
    pub area: [f64; 4],
}

pub enum FromWorker {
    LibraryFailed(String),
    Opened {
        doc: u64,
        path: PathBuf,
        pages: Vec<PageSize>,
        millis: u128,
    },
    OpenFailed {
        doc: u64,
        why: String,
        /// True when the only thing wrong is that nobody has typed the
        /// password yet — the one failure the window can do something about.
        wants_password: bool,
    },
    Tile {
        key: TileKey,
        image: egui::ColorImage,
        millis: u128,
        /// Which reading of the file it was drawn from. See `Refreshed`.
        generation: u64,
    },
    /// A drawing has been read again after a save. Tiles drawn from an
    /// earlier reading than `generation` show its markups where they were.
    Refreshed { doc: u64, generation: u64 },
    /// The paragraphs on a sheet.
    Paragraphs {
        doc: u64,
        page: u32,
        found: Result<Vec<crate::textedit::Found>, String>,
    },
    /// A paragraph written back: the whole file as it now should be, and
    /// what was done — or why nothing was.
    Rewritten {
        doc: u64,
        job: u64,
        page: u32,
        result: Result<(Vec<u8>, String), String>,
    },
    Preview {
        doc: u64,
        page: u32,
        scale: f32,
        image: egui::ColorImage,
    },
    Thumb {
        doc: u64,
        page: u32,
        image: egui::ColorImage,
    },
    /// Some of what a search found. They arrive sheet by sheet so the list
    /// fills while the rest of a two-hundred-sheet set is still being looked
    /// at.
    Found {
        doc: u64,
        job: u64,
        page: u32,
        hits: Vec<Hit>,
        /// The last sheet. Nothing more is coming for this search.
        done: bool,
    },
    /// A comparison finished.
    Compared {
        job: u64,
        found: Box<Compared>,
    },
    CompareFailed {
        job: u64,
        why: String,
    },
    /// Two issues laid over each other.
    Overlaid {
        job: u64,
        image: Box<egui::ColorImage>,
        said: String,
    },
    /// The two sheets drawn small for picking points on, and their sizes in points.
    OverlayLooks { // WHAT: both sheets drawn small. WHY: for the overlay window.
        job: u64,                                 // WHAT: which request. WHY: matched to the window's.
        older: Box<egui::ColorImage>,             // WHAT: the older sheet. WHY: clicked on.
        older_size: [f64; 2],                     // WHAT: its size in points. WHY: a click becomes a point on the sheet.
        newer: Box<egui::ColorImage>,             // WHAT: the newer sheet. WHY: clicked on.
        newer_size: [f64; 2],                     // WHAT: its size. WHY: the same.
    },
    /// A batch got through one more file. They arrive one at a time so the
    /// list fills while the rest of forty files is still running.
    BatchLine {
        job: u64,
        line: Box<crate::batch::Line>,
        /// How many there are altogether, so a person can see how far along it
        /// is without the program having to guess.
        of: usize,
    },
    BatchDone {
        job: u64,
    },
    /// A document operation finished.
    DocDone {
        job: u64,
        done: crate::docops::Done,
    },
    DocFailed {
        job: u64,
        why: String,
    },
    /// A sheet read for a plugin.
    Read {
        job: u64,
        page: u32,
        reading: Box<Reading>,
    },
    /// Sheets pulled out into their own file.
    Geometry {
        doc: u64,
        page: u32,
        geometry: std::sync::Arc<takeoff::snap::Geometry>,
        millis: u128,
    },
    PrintProgress {
        job: u64,
        done: usize,
        of: usize,
    },
    Printed {
        job: u64,
        sheets: usize,
        printer: String,
    },
    PrintFailed {
        job: u64,
        why: String,
    },
    Extracted {
        job: u64,
        to: PathBuf,
        pages: usize,
    },
    ExtractFailed {
        job: u64,
        why: String,
    },
    /// Where a symbol was found on one sheet. They arrive sheet by sheet, so
    /// a hundred-sheet search fills the list while it runs.
    Sightings {
        doc: u64,
        job: u64,
        page: u32,
        /// In sheet coordinates.
        places: Vec<[f64; 4]>,
        done: bool,
    },
    /// What a Dynamic Fill came to. Carries the failure as loudly as the
    /// success, because a fill that escaped must not read as a small area.
    Filled {
        doc: u64,
        job: u64,
        /// The outline in sheet coordinates, and what it measures, when the
        /// region was closed.
        outcome: FillOutcome,
    },
    Labels {
        doc: u64,
        first: u32,
        labels: Vec<SheetLabel>,
    },
}

pub struct Service {
    pub tx: Sender<ToWorker>,
    pub rx: Receiver<FromWorker>,
    /// Where pdfium was found, so work that has to happen while somebody is
    /// waiting — a snapshot, a picture written out — can go straight to the
    /// one pdfium thread rather than queue behind a set of tiles.
    pub library: Option<PathBuf>,
}

impl Service {
    /// Starts the engine. `library` is the folder holding `pdfium.dll`
    /// (or `libpdfium.so`); `None` means "look wherever the system looks".
    pub fn start(library: Option<PathBuf>, ctx: egui::Context) -> Service {
        let kept = library.clone();
        let (to_tx, to_rx) = crossbeam_channel::unbounded::<ToWorker>();
        let (from_tx, from_rx) = crossbeam_channel::unbounded::<FromWorker>();
        std::thread::Builder::new()
            .name("pdf".into())
            .stack_size(8 << 20)
            .spawn(move || run(library, to_rx, from_tx, ctx))
            .expect("spawn pdf thread");
        Service {
            tx: to_tx,
            rx: from_rx,
            library: kept,
        }
    }

    pub fn send(&self, msg: ToWorker) {
        let _ = self.tx.send(msg);
    }
}

/// The pdfium library file the viewer bound to, so printing can take pdfium's
/// Windows drawing call from the very same library.
static BOUND: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn bind(library: Option<PathBuf>) -> Result<Box<dyn PdfiumLibraryBindings>, String> {
    let mut tried = Vec::new();
    let mut candidates: Vec<PathBuf> = Vec::new();
    // The engine this program carries inside itself, when it does.
    if let Some(carried) = crate::install::pdfium_library() {
        candidates.push(carried);
    }
    if let Some(dir) = library {
        candidates.push(Pdfium::pdfium_platform_library_name_at_path(&dir));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // An iPad app carries a library inside a framework of its own,
            // in the app's Frameworks folder: the only way the App Store
            // takes one.
            if cfg!(target_os = "ios") {
                candidates.push(dir.join("Frameworks").join("pdfium.framework").join("pdfium"));
            }
            candidates.push(Pdfium::pdfium_platform_library_name_at_path(dir));
            candidates.push(Pdfium::pdfium_platform_library_name_at_path(
                &dir.join("pdfium"),
            ));
        }
    }
    for path in candidates {
        match Pdfium::bind_to_library(&path) {
            Ok(b) => {
                let _ = BOUND.set(path);
                return Ok(b);
            }
            Err(e) => tried.push(format!("{}: {e}", path.display())),
        }
    }
    match Pdfium::bind_to_system_library() {
        Ok(b) => Ok(b),
        Err(e) => {
            tried.push(format!("system library: {e}"));
            Err(tried.join("\n"))
        }
    }
}

/// Does one whole-drawing operation, on this thread, without the viewer.
///
/// The viewer's own operations go through the worker because pdfium must stay
/// on one thread and the window must not wait. This is the same code reached
/// another way, for two callers that are not the window: the tests, which need
/// to prove that combining twelve files really produces twelve files' worth of
/// sheets rather than that the message was sent; and batch work, which runs
/// dozens of these with nobody watching.
pub fn do_one(
    library: Option<PathBuf>,
    op: &crate::docops::Operation,
) -> Result<crate::docops::Done, String> {
    let op = op.clone();
    with_engine(library, move |engine| engine.document_operation(&op))
}

/// Reads the words off a scanned set and writes them back in, invisibly.
pub fn ocr_over(
    library: Option<PathBuf>,
    source: &std::path::Path,
    pages: &[u32],
    to: &std::path::Path,
    dpi: u32,
    only_sheets_without_text: bool,
) -> Result<crate::docops::Done, String> {
    let Some(engine) = crate::ocr::engine_here() else {
        return Err(crate::ocr::NO_ENGINE.to_string());
    };
    let source = source.to_path_buf();
    let pages = pages.to_vec();
    let to = to.to_path_buf();
    with_engine(library, move |machine| {
        machine.ocr(
            &source,
            &pages,
            &to,
            dpi,
            only_sheets_without_text,
            engine.as_ref(),
            |_, _, _| {},
        )
    })
}

/// Runs a batch and gives back the whole report.
///
/// The window does not use this — it wants each line as it lands, so a list of
/// forty fills while it runs. This is the same work in one call, for tests and
/// for anything running with nobody watching.
pub fn batch_over(
    library: Option<PathBuf>,
    work: &crate::batch::Job,
    files: &[PathBuf],
) -> Result<crate::batch::Report, String> {
    let work = work.clone();
    let files = files.to_vec();
    with_engine(library, move |engine| {
        let mut report = crate::batch::Report::default();
        if let Some(op) = work.for_all(&files) {
            let outcome = match engine.document_operation(&op) {
                Ok(done) => crate::batch::Outcome::Done {
                    wrote: done.wrote,
                    said: done.said,
                },
                Err(why) => crate::batch::Outcome::Failed(why),
            };
            report.lines.push(crate::batch::Line {
                source: files.first().cloned().unwrap_or_default(),
                outcome,
            });
            return Ok(report);
        }
        for source in &files {
            // A file that will not open is reported here rather than as a
            // failure halfway through the operation itself.
            let outcome = match engine.open_alone(source) {
                Err(why) => crate::batch::Outcome::Failed(why),
                Ok(one) => {
                    let pages = one.count;
                    engine.close_alone(one);
                    match work.for_one(source, pages) {
                        None => crate::batch::Outcome::Failed(no_pairing(&work).into()),
                        Some(op) => match engine.document_operation(&op) {
                            Ok(done) => crate::batch::Outcome::Done {
                                wrote: done.wrote,
                                said: done.said,
                            },
                            Err(why) => crate::batch::Outcome::Failed(why),
                        },
                    }
                }
            };
            report.lines.push(crate::batch::Line {
                source: source.clone(),
                outcome,
            });
        }
        Ok(report)
    })
}

/// Writes a takeoff summary out as a PDF.
pub fn write_report(
    library: Option<PathBuf>,
    report: &crate::report::Report,
    to: &std::path::Path,
) -> Result<crate::docops::Done, String> {
    let report = report.clone();
    let to = to.to_path_buf();
    with_engine(library, move |engine| engine.write_report(&report, &to))
}

/// Compares two issues of a sheet, off the window's thread.
///
/// The same code the Compare Documents window runs, reachable for tests and for
/// batch work — the two callers that need to know a comparison is right rather
/// than that a message was sent.
pub fn compare_two(
    library: Option<PathBuf>,
    older: &std::path::Path,
    older_page: u32,
    newer: &std::path::Path,
    newer_page: u32,
    darkness: f32,
) -> Result<Compared, String> {
    let older = older.to_path_buf();
    let newer = newer.to_path_buf();
    with_engine(library, move |engine| {
        engine.compare_sheets(&older, older_page, &newer, newer_page, darkness)
    })
}

/// Lays one issue of a sheet over another, off the window's thread.
///
/// The same code Overlay Pages runs, reachable for tests: the full-size
/// picture, how well they agreed, and what the window would say.
pub fn overlay_two( // WHAT: one sheet's overlay, for tests. WHY: the window's own path, checked.
    library: Option<PathBuf>, // WHAT: where pdfium is. WHY: the engine loads it.
    older: &std::path::Path, // WHAT: the older file. WHY: laid over.
    older_page: u32, // WHAT: its sheet. WHY: the same.
    newer: &std::path::Path, // WHAT: the newer file. WHY: laid on.
    newer_page: u32, // WHAT: its sheet. WHY: the same.
    darkness: f32, // WHAT: how dark is line-work. WHY: the window's setting.
    lining: &crate::overlay::Lining, // WHAT: how to line them up. WHY: the window's choice.
) -> Result<Overlaid, String> { // WHAT: the overlay, or why not. WHY: checked by the tests.
    let (older, newer, lining) = (older.to_path_buf(), newer.to_path_buf(), lining.clone()); // WHAT: owned copies. WHY: moved to the engine's thread.
    with_engine(library, move |engine| engine.overlay_sheets(&older, older_page, &newer, newer_page, darkness, &lining)) // WHAT: runs it. WHY: the same engine the window uses.
}

/// How many sheets are in a file. Asked of pdfium, which is the only thing that
/// actually knows.
/// Renders one sheet to a picture file.
///
/// The same rendering the viewer shows, written out: markups and all, at
/// whatever size is asked for, fitted rather than stretched so a sheet never
/// comes out the wrong proportions.
pub fn picture_of(
    library: Option<PathBuf>,
    path: &std::path::Path,
    page: u32,
    across: u32,
    down: u32,
    to: &std::path::Path,
) -> Result<(), String> {
    let path = path.to_path_buf();
    let to = to.to_path_buf();
    with_engine(library, move |engine| {
        let image = engine.picture(&path, page, across, down)?;
        write_png(&image, &to)
    })
}

/// Renders part of one sheet, in that sheet's own coordinates.
///
/// What a snapshot is: the drawing exactly as it appears, markups and all,
/// taken at a size somebody can paste into an email or a report without it
/// going soft.
pub fn region_of(
    library: Option<PathBuf>,
    path: &std::path::Path,
    page: u32,
    area: [f64; 4],
    dpi: u32,
) -> Result<egui::ColorImage, String> {
    let path = path.to_path_buf();
    with_engine(library, move |engine| engine.region(&path, page, area, dpi))
}

/// Every word on one sheet, as text.
///
/// Asked of pdfium rather than of our own parser, because the question these
/// callers are asking is what a reader would find — which is what makes a set
/// searchable for the general contractor as well as here.
pub fn words_on(
    library: Option<PathBuf>,
    path: &std::path::Path,
    page: u32,
) -> Result<String, String> {
    let path = path.to_path_buf();
    with_engine(library, move |engine| engine.words_on(&path, page))
}

/// One sheet as a plugin is handed it, for a file not open in a window.
pub fn read_for_plugin(
    library: Option<PathBuf>,
    path: &std::path::Path,
    page: u32,
    words: bool,
    lines: bool,
) -> Result<Reading, String> {
    let path = path.to_path_buf();
    with_engine(library, move |engine| engine.reading_file(&path, page, words, lines))
}

/// The grid on one sheet, read off the sheet's own text.
///
/// A grid location is a label rather than a quantity — it changes no number
/// anywhere — which is the only reason it is allowed to be worked out from
/// the drawing instead of clicked.
pub fn grid_on(
    library: Option<PathBuf>,
    path: &std::path::Path,
    page: u32,
    how: takeoff::grid::HowClose,
) -> Result<takeoff::grid::Grid, String> {
    let path = path.to_path_buf();
    with_engine(library, move |engine| engine.grid_on(&path, page, how))
}

/// Writes a picture out as a PNG, whatever the file it came from.
fn write_png(image: &egui::ColorImage, to: &std::path::Path) -> Result<(), String> {
    let [width, height] = [image.width() as u32, image.height() as u32];
    let mut flat: Vec<u8> = Vec::with_capacity((width * height * 4) as usize);
    for pixel in image.pixels.iter() {
        let [r, g, b, a] = pixel.to_array();
        flat.extend_from_slice(&[r, g, b, a]);
    }
    let buffer: image::RgbaImage = image::ImageBuffer::from_raw(width, height, flat)
        .ok_or_else(|| "the picture came out the wrong size".to_string())?;
    buffer
        .save(to)
        .map_err(|e| format!("could not write {}: {e}", nice(to)))
}

pub fn pages_in(library: Option<PathBuf>, path: &std::path::Path) -> Result<usize, String> {
    let path = path.to_path_buf();
    with_engine(library, move |engine| {
        let one = engine.open_alone(&path)?;
        let count = one.count;
        engine.close_alone(one);
        Ok(count)
    })
}

/// pdfium lives on exactly one thread, for the whole life of the process.
///
/// Not a style choice. `FPDF_InitLibrary` sets up state belonging to the
/// process, a pdfium handle is a raw pointer into that state, and tearing the
/// library down while handles are alive — which is what happens when a
/// per-thread binding is dropped at thread exit — leaves every other handle
/// pointing at memory pdfium has given back. The crash from that lands a long
/// way from the cause: a document loads, reports its page count perfectly, and
/// then closing it walks off the end of memory.
///
/// The viewer already works this way; its worker thread is the only thing that
/// touches pdfium. This is the same arrangement for everything that is not the
/// viewer — the tests, and batch work — so that the rule is enforced by there
/// being nowhere else to call from, rather than remembered.
struct Errand {
    work: Box<dyn FnOnce(&mut Engine) + Send>,
}

static ERRANDS: std::sync::OnceLock<crossbeam_channel::Sender<Errand>> =
    std::sync::OnceLock::new();

fn errands(library: Option<PathBuf>) -> &'static crossbeam_channel::Sender<Errand> {
    ERRANDS.get_or_init(|| {
        let (send, receive) = crossbeam_channel::unbounded::<Errand>();
        std::thread::Builder::new()
            .name("hyperview-pdfium".into())
            .spawn(move || {
                let mut engine = match bind(library) {
                    Ok(bindings) => {
                        unsafe { bindings.FPDF_InitLibrary() };
                        Engine::new(bindings)
                    }
                    Err(why) => {
                        // Every errand will fail, and each one says why. The
                        // thread stays up rather than the channel closing, so
                        // callers get that sentence instead of a disconnect.
                        log::error!("pdfium could not be loaded: {why}");
                        while let Ok(errand) = receive.recv() {
                            drop(errand);
                        }
                        return;
                    }
                };
                while let Ok(errand) = receive.recv() {
                    (errand.work)(&mut engine);
                }
            })
            .ok();
        send
    })
}

fn with_engine<T: Send + 'static>(
    library: Option<PathBuf>,
    work: impl FnOnce(&mut Engine) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tell, told) = crossbeam_channel::bounded::<Result<T, String>>(1);
    let sent = errands(library).send(Errand {
        work: Box::new(move |engine| {
            let _ = tell.send(work(engine));
        }),
    });
    if sent.is_err() {
        return Err("pdfium is not available in this program".into());
    }
    told.recv().map_err(|_| {
        "pdfium could not be loaded — see the log beside the program for which \
         libraries were tried"
            .to_string()
    })?
}

fn run(
    library: Option<PathBuf>,
    rx: Receiver<ToWorker>,
    tx: Sender<FromWorker>,
    ctx: egui::Context,
) {
    let bindings = match bind(library) {
        Ok(b) => b,
        Err(why) => {
            let _ = tx.send(FromWorker::LibraryFailed(why));
            ctx.request_repaint();
            return;
        }
    };
    unsafe { bindings.FPDF_InitLibrary() };

    let mut engine = Engine::new(bindings);
    let mut queue = Queue::default();
    start_helpers(&queue.board, &tx, &ctx);

    loop {
        // Absorb everything pending before doing any work, so a fresh view
        // request always wins over a stale one.
        let mut disconnected = false;
        loop {
            match rx.try_recv() {
                Ok(ToWorker::Quit) | Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
                Ok(msg) => queue.accept(msg, &mut engine, &tx, &ctx),
                Err(TryRecvError::Empty) => break,
            }
        }
        if disconnected {
            break;
        }

        if queue.idle() {
            match rx.recv() {
                Ok(ToWorker::Quit) | Err(_) => break,
                Ok(msg) => queue.accept(msg, &mut engine, &tx, &ctx),
            }
            continue;
        }

        queue.step(&mut engine, &tx, &ctx);
    }

    queue.board.quit();
    for one in std::mem::take(&mut engine.open) {
        engine.let_go(one);
    }
}

// ---- more than one pair of hands for tiles ---------------------------------
//
// pdfium keeps its state in globals, so one copy of it can only draw one thing
// at a time. But two copies of the library, loaded from two files, are two
// independent engines. So each helper thread loads its own copy and draws
// tiles off the same list as the main PDF thread: a sheet that took six tiles
// in a row to go sharp now takes as many rounds as there are hands.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex};

#[derive(Default)]
struct Wants {
    wanted: Vec<TileKey>,
    taken: HashSet<TileKey>,
    /// Each open drawing's file, and how many times it has been (re)opened,
    /// so a helper knows when a saved drawing has to be read again.
    docs: HashMap<u64, (PathBuf, u64)>,
    opened: u64,
    /// Drawings a helper could not open (a password, say): only the main
    /// thread draws those.
    main_only: HashSet<u64>,
    quit: bool,
}

#[derive(Clone, Default)]
struct Board(Arc<(Mutex<Wants>, Condvar)>);

impl Board {
    fn with<T>(&self, f: impl FnOnce(&mut Wants) -> T) -> T {
        let mut wants = self.0 .0.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut wants)
    }

    fn wake(&self) {
        self.0 .1.notify_all();
    }

    fn want(&self, tiles: Vec<TileKey>) {
        self.with(|w| {
            w.wanted = tiles.into_iter().filter(|k| !w.taken.contains(k)).collect();
        });
        self.wake();
    }

    fn forget(&self, doc: u64) {
        self.with(|w| w.wanted.retain(|k| k.doc != doc));
    }

    fn opened(&self, doc: u64, path: &std::path::Path) {
        self.with(|w| {
            w.opened += 1;
            let n = w.opened;
            w.docs.insert(doc, (path.to_path_buf(), n));
            w.main_only.remove(&doc);
        });
        self.wake();
    }

    fn closed(&self, doc: u64) {
        self.with(|w| {
            w.docs.remove(&doc);
            w.wanted.retain(|k| k.doc != doc);
        });
    }

    fn quit(&self) {
        self.with(|w| w.quit = true);
        self.wake();
    }

    fn nothing_for(&self, helper: bool) -> bool {
        self.with(|w| Self::first(w, helper).is_none())
    }

    fn first(w: &Wants, helper: bool) -> Option<usize> {
        w.wanted.iter().position(|k| {
            !w.taken.contains(k)
                && (!helper || (!w.main_only.contains(&k.doc) && w.docs.contains_key(&k.doc)))
        })
    }

    /// The most important tile nobody is drawing yet.
    fn take(&self, helper: bool) -> Option<TileKey> {
        self.with(|w| {
            let at = Self::first(w, helper)?;
            let key = w.wanted.remove(at);
            w.taken.insert(key);
            Some(key)
        })
    }

    fn done(&self, key: TileKey) {
        self.with(|w| {
            w.taken.remove(&key);
        });
    }

    /// Hands a tile back that a helper could not draw.
    fn give_back(&self, key: TileKey, main_only: bool) {
        self.with(|w| {
            w.taken.remove(&key);
            if main_only {
                w.main_only.insert(key.doc);
            }
            w.wanted.insert(0, key);
        });
        self.wake();
    }

    /// Blocks a helper until there is a tile for it, or it is time to stop.
    fn wait_for_work(&self) -> Option<(TileKey, PathBuf, u64)> {
        let (lock, wake) = &*self.0;
        let mut w = lock.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if w.quit {
                return None;
            }
            if let Some(at) = Self::first(&w, true) {
                let key = w.wanted.remove(at);
                w.taken.insert(key);
                let (path, generation) = w.docs.get(&key.doc).cloned()?;
                return Some((key, path, generation));
            }
            w = wake.wait(w).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// How many times a drawing has been read, counting across all of them.
    fn generation_of(&self, doc: u64) -> u64 {
        self.with(|w| w.docs.get(&doc).map(|(_, g)| *g).unwrap_or(0))
    }

    fn still_open(&self) -> HashMap<u64, u64> {
        self.with(|w| w.docs.iter().map(|(d, (_, g))| (*d, *g)).collect())
    }
}

/// How many helpers: one fewer than the processor has cores, up to three, so
/// the window itself always has a core. `HYPERVIEW_TILE_THREADS` overrides it.
fn helper_count() -> usize {
    // An iPad will only load code the app was signed with, and a helper's
    // copy of the library is not; one pair of hands draws everything there.
    if cfg!(target_os = "ios") {
        return 0;
    }
    if let Some(n) = std::env::var("HYPERVIEW_TILE_THREADS").ok().and_then(|v| v.parse().ok()) {
        return n;
    }
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1).min(3))
        .unwrap_or(0)
}

/// The library file the main thread bound to, to copy for the helpers.
fn library_file() -> Option<PathBuf> {
    if let Some(bound) = BOUND.get() {
        return Some(bound.clone());
    }
    let name = Pdfium::pdfium_platform_library_name();
    let places = ["PDFIUM_DIR", "LD_LIBRARY_PATH"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .flat_map(|v| std::env::split_paths(&v).collect::<Vec<_>>());
    for dir in places {
        let file = dir.join(&name);
        if file.exists() {
            return Some(file);
        }
    }
    None
}

/// A copy of the library for helper `n`, made once and kept.
fn helper_library(source: &std::path::Path, n: usize) -> Option<PathBuf> {
    let size = std::fs::metadata(source).ok()?.len();
    let stem = source.file_stem()?.to_string_lossy().to_string();
    let ext = source.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
    let name = format!("{stem}-hands{n}-{size}.{ext}");
    let beside = source.parent().map(|d| d.join(&name));
    let places = beside.into_iter().chain(std::iter::once(std::env::temp_dir().join(&name)));
    for place in places {
        if std::fs::metadata(&place).map(|m| m.len() == size).unwrap_or(false) {
            return Some(place);
        }
        let part = place.with_extension("part");
        if std::fs::copy(source, &part).is_ok() && std::fs::rename(&part, &place).is_ok() {
            return Some(place);
        }
        let _ = std::fs::remove_file(&part);
    }
    None
}

fn start_helpers(board: &Board, tx: &Sender<FromWorker>, ctx: &egui::Context) {
    let wanted = helper_count();
    if wanted == 0 {
        log::info!("tiles: one pair of hands (one core to spare)");
        return;
    }
    let Some(source) = library_file() else {
        log::warn!("tiles: could not find the PDF library to copy for helpers");
        return;
    };
    for n in 1..=wanted {
        let Some(copy) = helper_library(&source, n) else {
            log::warn!("tiles: could not copy {} for helper {n}", source.display());
            break;
        };
        log::info!("tiles: helper {n} draws with {}", copy.display());
        let (board, tx, ctx) = (board.clone(), tx.clone(), ctx.clone());
        let _ = std::thread::Builder::new()
            .name(format!("pdf-hands-{n}"))
            .stack_size(8 << 20)
            .spawn(move || helper(copy, board, tx, ctx));
    }
}

fn helper(library: PathBuf, board: Board, tx: Sender<FromWorker>, ctx: egui::Context) {
    let Ok(bindings) = Pdfium::bind_to_library(&library) else {
        log::warn!("a tile helper could not load {}", library.display());
        return;
    };
    unsafe { bindings.FPDF_InitLibrary() };
    let mut engine = Engine::new(bindings);
    let mut have: HashMap<u64, u64> = HashMap::new();
    while let Some((key, path, generation)) = board.wait_for_work() {
        if have.get(&key.doc) != Some(&generation) {
            match engine.open(key.doc, &path, "") {
                Ok(_) => {
                    have.insert(key.doc, generation);
                }
                Err(_) => {
                    board.give_back(key, true);
                    continue;
                }
            }
        }
        let started = Instant::now();
        let image = engine.tile(key);
        board.done(key);
        if let Some(image) = image {
            if tx
                .send(FromWorker::Tile {
                    key,
                    image,
                    millis: started.elapsed().as_millis(),
                    generation,
                })
                .is_err()
            {
                break;
            }
            ctx.request_repaint();
        }
        // Let go of drawings the window has closed.
        let open = board.still_open();
        let gone: Vec<u64> = have.keys().copied().filter(|d| !open.contains_key(d)).collect();
        for doc in gone {
            engine.close_one(doc);
            have.remove(&doc);
        }
    }
    for one in std::mem::take(&mut engine.open) {
        engine.let_go(one);
    }
}

#[derive(Default)]
struct Queue {
    /// The tiles the screen wants, shared with the helper threads.
    board: Board,
    previews: Vec<(u64, u32)>,
    thumbs: Vec<(u64, u32)>,
    /// Which drawing's sheet numbers are being read, and how far along.
    labelling: Option<(u64, u32, u32)>,
    /// The search running now, if any.
    hunt: Option<Hunt>,
    /// The symbol search running now, if any.
    looking: Option<Looking>,
    /// The print job running now, if any.
    printing: Option<PrintRun>,
    /// Sheets whose line-work is wanted for snapping.
    geometry: Vec<(u64, u32)>,
    /// Sheets a plugin is waiting to have read: doc, page, job, words, lines.
    reading: Vec<(u64, u32, u64, bool, bool)>,
}

/// A print job, a sheet at a time between whatever the screen needs.
struct PrintRun {
    job: u64,
    doc: u64,
    path: PathBuf,
    printer: String,
    count: u32,
    next: u32,
    fit: crate::winprint::Fit,
    orientation: crate::winprint::Orientation,
    run: crate::winprint::Job,
    draw: crate::winprint::RenderToDc,
}

const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
// pdfium's numbers for the kinds of page object and path segment.
const OBJECT_PATH: u32 = 2;
const OBJECT_FORM: u32 = 5;
const SEGMENT_LINETO: u32 = 0;
const SEGMENT_BEZIERTO: u32 = 1;
const SEGMENT_MOVETO: u32 = 2;

/// `inner` then `outer`, as PDF matrices compose: a point goes through the
/// object's own transform first, then its container's.
fn then(inner: [f64; 6], outer: [f64; 6]) -> [f64; 6] {
    let [a, b, c, d, e, f] = inner;
    let [a2, b2, c2, d2, e2, f2] = outer;
    [
        a * a2 + b * c2,
        a * b2 + b * d2,
        c * a2 + d * c2,
        c * b2 + d * d2,
        e * a2 + f * c2 + e2,
        e * b2 + f * d2 + f2,
    ]
}

/// Documents opened only to be printed get ids well away from the tabs'.
const PRINT_DOC: u64 = u64::MAX / 2;

/// A VisualSearch, one sheet at a time.
struct Looking {
    doc: u64,
    job: u64,
    symbol: takeoff::symbols::Ink,
    /// How big the symbol is on the sheet, so a sighting can be given back in
    /// sheet coordinates.
    on_sheet: (f64, f64),
    scale: f64,
    alike: f32,
    darkness: f32,
    pages: Vec<u32>,
    next: usize,
}

/// A search, one sheet at a time.
///
/// Sheet by sheet rather than all at once, because a two-hundred-sheet set takes
/// a moment and a list that fills while you read it is worth far more than one
/// that appears complete after four seconds.
struct Hunt {
    doc: u64,
    job: u64,
    needle: String,
    whole_words: bool,
    match_case: bool,
    next: u32,
    count: u32,
}

impl Queue {
    fn idle(&self) -> bool {
        self.board.nothing_for(false)
            && self.previews.is_empty()
            && self.thumbs.is_empty()
            && self.labelling.is_none()
            && self.hunt.is_none()
            && self.looking.is_none()
            && self.printing.is_none()
            && self.geometry.is_empty()
    }

    fn accept(
        &mut self,
        msg: ToWorker,
        engine: &mut Engine,
        tx: &Sender<FromWorker>,
        ctx: &egui::Context,
    ) {
        match msg {
            ToWorker::Open { doc, path, password } => {
                // Work queued for a drawing that is being replaced is dropped;
                // work for the other open tabs is not, because switching tabs
                // must not throw away what the last one was drawing.
                self.forget(doc);
                let started = Instant::now();
                match engine.open(doc, &path, &password) {
                    Ok(pages) => {
                        self.board.opened(doc, &path);
                        let _ = tx.send(FromWorker::Opened {
                            doc,
                            path,
                            pages,
                            millis: started.elapsed().as_millis(),
                        });
                    }
                    Err(why) => {
                        let wants_password = why.wants_password;
                        let _ = tx.send(FromWorker::OpenFailed {
                            doc,
                            why: why.said,
                            wants_password,
                        });
                    }
                }
                ctx.request_repaint();
            }
            ToWorker::Close(doc) => {
                self.forget(doc);
                self.board.closed(doc);
                engine.close_one(doc);
            }
            ToWorker::Paragraphs { doc, page } => {
                let found = match engine.path_of(doc) {
                    Some(path) => engine.paragraphs_on(&path, page),
                    None => Err("that drawing is not open".into()),
                };
                let _ = tx.send(FromWorker::Paragraphs { doc, page, found });
                ctx.request_repaint();
            }
            ToWorker::Rewrite { doc, job, page, found, rewrite } => {
                let result = match engine.path_of(doc) {
                    Some(path) => engine.rewrite_paragraph(&path, page, &found, &rewrite),
                    None => Err("that drawing is not open".into()),
                };
                let _ = tx.send(FromWorker::Rewritten { doc, job, page, result });
                ctx.request_repaint();
            }
            ToWorker::Refresh { doc } => {
                // The same file under the same id, read again. The window is
                // not told it opened: it already has it open, and its view,
                // its selection and its undo steps all stay as they are.
                if let Some(path) = engine.path_of(doc) {
                    match engine.open(doc, &path, "") {
                        Ok(_) => {
                            self.board.opened(doc, &path);
                            let generation = self.board.generation_of(doc);
                            let _ = tx.send(FromWorker::Refreshed { doc, generation });
                            ctx.request_repaint();
                        }
                        Err(why) => {
                            log::warn!("could not read {} again: {}", path.display(), why.said);
                            // Said all the same, so a window waiting on it
                            // does not wait for ever.
                            let generation = self.board.generation_of(doc);
                            let _ = tx.send(FromWorker::Refreshed { doc, generation });
                            ctx.request_repaint();
                        }
                    }
                }
            }
            ToWorker::PrintFile {
                job,
                path,
                printer,
                settings,
                fit,
                orientation,
                title,
            } => {
                if let Some(old) = self.printing.take() {
                    old.run.abandon();
                    engine.close_one(old.doc);
                }
                let doc = PRINT_DOC + job;
                let started = (|| {
                    let draw = crate::winprint::render_to_dc(BOUND.get().map(|p| p.as_path()))
                        .ok_or("this copy of the PDF engine cannot draw onto a printer")?;
                    let sizes = engine.open(doc, &path, "")?;
                    let run = crate::winprint::Job::start(&printer, &settings, &title)?;
                    Ok::<_, String>((sizes.len() as u32, run, draw))
                })();
                match started {
                    Ok((count, run, draw)) => {
                        self.printing = Some(PrintRun {
                            job,
                            doc,
                            path,
                            printer,
                            count,
                            next: 0,
                            fit,
                            orientation,
                            run,
                            draw,
                        });
                    }
                    Err(why) => {
                        engine.close_one(doc);
                        let _ = std::fs::remove_file(&path);
                        let _ = tx.send(FromWorker::PrintFailed { job, why });
                        ctx.request_repaint();
                    }
                }
            }
            ToWorker::Geometry { doc, page } => {
                if !self.geometry.contains(&(doc, page)) {
                    self.geometry.push((doc, page));
                }
            }
            ToWorker::Read { doc, page, job, words, lines } => {
                self.reading.push((doc, page, job, words, lines));
            }
            ToWorker::CancelPrint(job) => {
                if matches!(&self.printing, Some(p) if p.job == job) {
                    if let Some(run) = self.printing.take() {
                        run.run.abandon();
                        engine.close_one(run.doc);
                        let _ = std::fs::remove_file(&run.path);
                    }
                }
            }
            ToWorker::Want { tiles } => self.board.want(tiles),
            ToWorker::Preview { doc, page } => {
                if !self.previews.contains(&(doc, page)) {
                    self.previews.push((doc, page));
                }
            }
            ToWorker::Thumbs { doc, pages } => {
                for p in pages {
                    if !self.thumbs.contains(&(doc, p)) {
                        self.thumbs.push((doc, p));
                    }
                }
                // Scrolling fast through six hundred sheets must not leave a
                // queue of thumbnails nobody is looking at any more.
                if self.thumbs.len() > 48 {
                    let extra = self.thumbs.len() - 48;
                    self.thumbs.drain(0..extra);
                }
            }
            ToWorker::ScanLabels(doc) => {
                self.labelling = Some((doc, 0, engine.page_count(doc)));
            }
            ToWorker::Symbols {
                doc,
                job,
                from_page,
                pages,
                symbol,
                alike,
                darkness,
            } => {
                self.looking = engine
                    .cut_symbol(doc, from_page, symbol, darkness)
                    .map(|(ink, on_sheet, scale)| Looking {
                        doc,
                        job,
                        symbol: ink,
                        on_sheet,
                        scale,
                        alike,
                        darkness,
                        pages,
                        next: 0,
                    });
                if self.looking.is_none() {
                    let _ = tx.send(FromWorker::Sightings {
                        doc,
                        job,
                        page: from_page,
                        places: Vec::new(),
                        done: true,
                    });
                    ctx.request_repaint();
                }
            }
            ToWorker::DynamicFill {
                doc,
                page,
                job,
                area,
                seed,
                strokes,
                darkness,
            } => {
                // Done there and then rather than queued: somebody has just
                // clicked and is waiting to see the shape, and it takes a
                // fraction of a second.
                let outcome = engine.dynamic_fill(doc, page, area, seed, &strokes, darkness);
                let _ = tx.send(FromWorker::Filled { doc, job, outcome });
                ctx.request_repaint();
            }
            ToWorker::Find {
                doc,
                job,
                needle,
                whole_words,
                match_case,
            } => {
                // A new search replaces whatever was running. Somebody who has
                // typed another letter is not waiting on the old one.
                self.hunt = (!needle.trim().is_empty()).then(|| Hunt {
                    doc,
                    job,
                    needle,
                    whole_words,
                    match_case,
                    next: 0,
                    count: engine.page_count(doc),
                });
            }
            ToWorker::Extract {
                doc,
                job,
                pages,
                to,
            } => {
                let told = match engine.extract(doc, &pages, &to) {
                    Ok(pages) => FromWorker::Extracted { job, to, pages },
                    Err(why) => FromWorker::ExtractFailed { job, why },
                };
                let _ = tx.send(told);
                ctx.request_repaint();
            }
            ToWorker::Compare {
                job,
                older,
                older_page,
                newer,
                newer_page,
                darkness,
            } => {
                let told = match engine.compare_sheets(
                    &older,
                    older_page,
                    &newer,
                    newer_page,
                    darkness,
                ) {
                    Ok(found) => FromWorker::Compared {
                        job,
                        found: Box::new(found),
                    },
                    Err(why) => FromWorker::CompareFailed { job, why },
                };
                let _ = tx.send(told);
                ctx.request_repaint();
            }
            ToWorker::Overlay { job, older, older_page, newer, newer_page, darkness, lining } => { // WHAT: lay two sheets over each other. WHY: the overlay window asked.
                let told = match engine.overlay_sheets(&older, older_page, &newer, newer_page, darkness, &lining) { // WHAT: lays them over. WHY: the window asked.
                    Ok(laid) => {
                        let (pixels, w, h) = crate::overlay::smaller(&pixels_of(&laid.image), laid.image.size[0], laid.image.size[1], PREVIEW_PIXELS); // WHAT: a smaller copy for the window. WHY: a full-size overlay is bigger than a graphics card will take as one picture.
                        FromWorker::Overlaid { job, image: Box::new(colour_image(&pixels, w, h)), said: laid.said } // WHAT: the picture and what to say. WHY: shown in the window.
                    }
                    Err(why) => FromWorker::CompareFailed { job, why }, // WHAT: why not. WHY: shown in the window.
                };
                let _ = tx.send(told); // WHAT: sends it. WHY: the window is waiting.
                ctx.request_repaint(); // WHAT: wakes the window. WHY: it only redraws when asked.
            }
            ToWorker::OverlayLooks { job, older, older_page, newer, newer_page } => { // WHAT: draw both sheets small. WHY: to pick points on.
                let told = match (engine.picture(&older, older_page, LOOK_PIXELS, LOOK_PIXELS), engine.picture(&newer, newer_page, LOOK_PIXELS, LOOK_PIXELS), engine.sheet_size(&older, older_page), engine.sheet_size(&newer, newer_page)) { // WHAT: draws both sheets and reads their sizes. WHY: points are picked on them.
                    (Ok(a), Ok(b), Ok(sa), Ok(sb)) => FromWorker::OverlayLooks { job, older: Box::new(a), older_size: sa, newer: Box::new(b), newer_size: sb }, // WHAT: both sheets and their sizes. WHY: points are picked on them.
                    (Err(why), ..) | (_, Err(why), ..) | (_, _, Err(why), _) | (.., Err(why)) => FromWorker::CompareFailed { job, why }, // WHAT: why one would not draw. WHY: shown.
                };
                let _ = tx.send(told); // WHAT: sends it. WHY: the window is waiting.
                ctx.request_repaint(); // WHAT: wakes the window. WHY: the same.
            }
            ToWorker::Batch { job, work, files } => {
                // Combining is the one job that takes the whole list at once;
                // everything else is one file at a time.
                if let Some(op) = work.for_all(&files) {
                    let line = match engine.document_operation(&op) {
                        Ok(done) => crate::batch::Line {
                            source: files.first().cloned().unwrap_or_default(),
                            outcome: crate::batch::Outcome::Done {
                                wrote: done.wrote,
                                said: done.said,
                            },
                        },
                        Err(why) => crate::batch::Line {
                            source: files.first().cloned().unwrap_or_default(),
                            outcome: crate::batch::Outcome::Failed(why),
                        },
                    };
                    let _ = tx.send(FromWorker::BatchLine {
                        job,
                        line: Box::new(line),
                        of: 1,
                    });
                } else {
                    let of = files.len();
                    for source in &files {
                        // Counted first, because most jobs need to know how
                        // many sheets there are — and a file that will not even
                        // open is reported here rather than halfway through.
                        let outcome = match engine.open_alone(source) {
                            Err(why) => crate::batch::Outcome::Failed(why),
                            Ok(one) => {
                                let pages = one.count;
                                engine.close_alone(one);
                                match work.for_one(source, pages) {
                                    None => crate::batch::Outcome::Failed(
                                        no_pairing(&work).into(),
                                    ),
                                    Some(op) => match engine.document_operation(&op) {
                                        Ok(done) => crate::batch::Outcome::Done {
                                            wrote: done.wrote,
                                            said: done.said,
                                        },
                                        Err(why) => crate::batch::Outcome::Failed(why),
                                    },
                                }
                            }
                        };
                        let _ = tx.send(FromWorker::BatchLine {
                            job,
                            line: Box::new(crate::batch::Line {
                                source: source.clone(),
                                outcome,
                            }),
                            of,
                        });
                        ctx.request_repaint();
                    }
                }
                let _ = tx.send(FromWorker::BatchDone { job });
                ctx.request_repaint();
            }
            ToWorker::Document { job, op } => {
                let told = match engine.document_operation(&op) {
                    Ok(done) => FromWorker::DocDone { job, done },
                    Err(why) => FromWorker::DocFailed { job, why },
                };
                let _ = tx.send(told);
                ctx.request_repaint();
            }
            ToWorker::Quit => {}
        }
    }

    /// Throws away everything queued for one drawing, leaving the others alone.
    fn forget(&mut self, doc: u64) {
        self.board.forget(doc);
        self.geometry.retain(|(d, _)| *d != doc);
        self.reading.retain(|(d, ..)| *d != doc);
        if matches!(self.looking.as_ref(), Some(l) if l.doc == doc) {
            self.looking = None;
        }
        self.previews.retain(|(d, _)| *d != doc);
        self.thumbs.retain(|(d, _)| *d != doc);
        if matches!(self.labelling, Some((d, _, _)) if d == doc) {
            self.labelling = None;
        }
        if matches!(self.hunt.as_ref(), Some(h) if h.doc == doc) {
            self.hunt = None;
        }
    }

    fn step(&mut self, engine: &mut Engine, tx: &Sender<FromWorker>, ctx: &egui::Context) {
        // A page the user is looking at right now beats everything else.
        if let Some((doc, page)) = pop(&mut self.previews) {
            if let Some((scale, image)) = engine.preview(doc, page) {
                let _ = tx.send(FromWorker::Preview { doc, page, scale, image });
                ctx.request_repaint();
            }
            return;
        }
        if let Some(key) = self.board.take(false) {
            let started = Instant::now();
            let generation = self.board.generation_of(key.doc);
            if let Some(image) = engine.tile(key) {
                let _ = tx.send(FromWorker::Tile {
                    key,
                    image,
                    millis: started.elapsed().as_millis(),
                    generation,
                });
                ctx.request_repaint();
            }
            self.board.done(key);
            return;
        }
        if let Some((doc, page, job, words, lines)) = pop(&mut self.reading) {
            let reading = engine.reading(doc, page, words, lines).unwrap_or_default();
            let _ = tx.send(FromWorker::Read {
                job,
                page,
                reading: Box::new(reading),
            });
            ctx.request_repaint();
            return;
        }
        if let Some((doc, page)) = pop(&mut self.geometry) {
            let started = Instant::now();
            if let Some(geometry) = engine.geometry(doc, page) {
                let _ = tx.send(FromWorker::Geometry {
                    doc,
                    page,
                    geometry: std::sync::Arc::new(geometry),
                    millis: started.elapsed().as_millis(),
                });
                ctx.request_repaint();
            }
            return;
        }
        if let Some(mut run) = self.printing.take() {
            let page = run.next;
            let printed = engine.print_page(run.doc, page, &mut run.run, run.fit, run.orientation, run.draw);
            run.next += 1;
            match printed {
                Err(why) => {
                    run.run.abandon();
                    engine.close_one(run.doc);
                    let _ = std::fs::remove_file(&run.path);
                    let _ = tx.send(FromWorker::PrintFailed { job: run.job, why });
                }
                Ok(()) if run.next >= run.count => {
                    let finished = run.run.finish();
                    engine.close_one(run.doc);
                    let _ = std::fs::remove_file(&run.path);
                    let _ = tx.send(match finished {
                        Ok(()) => FromWorker::Printed {
                            job: run.job,
                            sheets: run.count as usize,
                            printer: run.printer,
                        },
                        Err(why) => FromWorker::PrintFailed { job: run.job, why },
                    });
                }
                Ok(()) => {
                    let _ = tx.send(FromWorker::PrintProgress {
                        job: run.job,
                        done: run.next as usize,
                        of: run.count as usize,
                    });
                    self.printing = Some(run);
                }
            }
            ctx.request_repaint();
            return;
        }
        if let Some((doc, page)) = pop(&mut self.thumbs) {
            if let Some(image) = engine.thumb(doc, page) {
                let _ = tx.send(FromWorker::Thumb { doc, page, image });
                ctx.request_repaint();
            }
            return;
        }
        if let Some((doc, first, count)) = self.labelling {
            let last = (first + LABEL_BATCH).min(count);
            let labels: Vec<SheetLabel> = (first..last).map(|p| engine.label(doc, p)).collect();
            let _ = tx.send(FromWorker::Labels { doc, first, labels });
            ctx.request_repaint();
            self.labelling = (last < count).then_some((doc, last, count));
            return;
        }
        if let Some(looking) = self.looking.as_mut() {
            let Some(page) = looking.pages.get(looking.next).copied() else {
                self.looking = None;
                return;
            };
            looking.next += 1;
            let done = looking.next >= looking.pages.len();
            let places = engine.look_for(
                looking.doc,
                page,
                &looking.symbol,
                looking.on_sheet,
                looking.scale,
                looking.alike,
                looking.darkness,
            );
            let _ = tx.send(FromWorker::Sightings {
                doc: looking.doc,
                job: looking.job,
                page,
                places,
                done,
            });
            ctx.request_repaint();
            if done {
                self.looking = None;
            }
            return;
        }
        if let Some(hunt) = self.hunt.as_mut() {
            let page = hunt.next;
            let hits = engine.find(
                hunt.doc,
                page,
                &hunt.needle,
                hunt.whole_words,
                hunt.match_case,
            );
            hunt.next += 1;
            let done = hunt.next >= hunt.count;
            let _ = tx.send(FromWorker::Found {
                doc: hunt.doc,
                job: hunt.job,
                page,
                hits,
                done,
            });
            ctx.request_repaint();
            if done {
                self.hunt = None;
            }
        }
    }
}

const LABEL_BATCH: u32 = 8;

/// True when a match is not part of a longer word.
///
/// A drawing is full of marks that are words to a fabricator and punctuation to
/// a dictionary — `W12x26`, `1/2"`, `S-200`, `PL3/4` — so what counts as part of
/// a word here is a letter, a digit, or one of the handful of characters that
/// hold a designation together. Without this, searching for `PL1` answers with
/// every `PL1/2` on the sheet.
fn is_whole_word(hay: &[char], from: usize, to: usize) -> bool {
    let part_of_a_word = |c: char| c.is_alphanumeric() || matches!(c, '/' | '-' | '_' | '.' | '#');
    let before = from == 0 || !part_of_a_word(hay[from - 1]);
    let after = to >= hay.len() || !part_of_a_word(hay[to]);
    before && after
}

/// Collapses the newlines and runs of spaces that CAD output leaves in text, so
/// a line of context reads as a line.
fn squash(chars: &[char]) -> String {
    let mut out = String::with_capacity(chars.len());
    let mut space = false;
    for c in chars {
        if c.is_whitespace() {
            if !space && !out.is_empty() {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(*c);
            space = false;
        }
    }
    out
}

/// Takes the front of the list. Requests are already in priority order —
/// tiles arrive centre-of-screen outwards — so the front is always the most
/// useful thing to draw next.
fn pop<T>(v: &mut Vec<T>) -> Option<T> {
    if v.is_empty() {
        None
    } else {
        Some(v.remove(0))
    }
}

// ---------------------------------------------------------------------------
// The engine itself. Everything below here runs on the pdf thread only.
// ---------------------------------------------------------------------------

use std::ffi::c_void;

// Pdfium's public constants. pdfium-render binds the functions but not these,
// and they have not changed since the API was first published.
const FPDFBITMAP_BGRA: u32 = 4;
const FPDF_ANNOT: u32 = 0x01;
const FPDF_REVERSE_BYTE_ORDER: u32 = 0x10;

/// How many decoded pages to keep hot. Consecutive tiles of the same sheet must
/// not re-parse the content stream, and flipping back one sheet should be instant.
const PAGE_CACHE: usize = 6;
const PREVIEW_PX: f64 = 1600.0;
const THUMB_PX: f64 = 224.0;

/// A drawing opened for one operation, outside the viewer's cache.
///
/// The bytes are held rather than mapped: an operation may write over the file
/// it is reading from in the next moment (a slip-sheet whose output lands
/// beside its input), and a mapping into a file that has changed underneath is
/// how a program reads something that is no longer there.
struct Aside {
    _bytes: Vec<u8>,
    doc: FPDF_DOCUMENT,
    count: usize,
}

/// Appends revision clouds to a file, as real annotations.
///
/// Through `annot::place`, which is the same path the viewer saves markups by:
/// an incremental update that appends objects and a new cross-reference table,
/// leaving every original byte where it was. The boxes arrive in sheet space —
/// top left origin, y down, already turned by `/Rotate` — because that is what
/// the comparison looked at, so each one is put back through the page's own
/// frame before it is written.
fn write_clouds(
    path: &std::path::Path,
    author: &str,
    found_on: &[(usize, Vec<(crate::compare::Kind, [f64; 4])>)],
) -> Result<usize, String> {
    if found_on.is_empty() {
        return Ok(0);
    }
    let file = pdf::Document::open(path).map_err(|e| format!("{}: {e}", nice(path)))?;
    let mut placer = annot::place::Placer::new(&file).by(author);

    let mut made = 0usize;
    for (page, changes) in found_on {
        let Some(dict) = file.page(*page) else { continue };
        let frame = annot::page::Frame::of(&file, &dict);
        for (kind, area) in changes {
            // A little room around it, so the cloud sits outside what changed
            // rather than through it.
            let margin = 6.0;
            let a = frame.to_pdf([area[0] - margin, area[1] - margin]);
            let b = frame.to_pdf([area[2] + margin, area[3] + margin]);

            let mut markup = annot::Markup::new(annot::Subtype::Square);
            markup.set_box([
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[0].max(b[0]),
                a[1].max(b[1]),
            ]);
            let colour = match kind {
                crate::compare::Kind::Added => [0.184f32, 0.620, 0.267],
                crate::compare::Kind::Removed => [0.816, 0.271, 0.235],
                crate::compare::Kind::Changed => [0.878, 0.545, 0.102],
            };
            markup.set_colour(colour);
            markup.set_width(2.0);
            markup.set_subject(&format!("Revision — {}", kind.word()));
            // The border effect that makes a box a cloud rather than a
            // rectangle. `/I` is how deep the scallops are; 2 is what Revu
            // writes by default and what people recognise.
            let mut effect = pdf::Dict::new();
            effect.set("S", pdf::Object::name("C"));
            effect.set("I", pdf::Object::Real(2.0));
            markup.dict.set("BE", pdf::Object::Dict(effect));

            if placer.add(*page, &mut markup).is_some() {
                made += 1;
            }
        }
    }
    if made == 0 {
        return Ok(0);
    }
    let updated = placer.finish().apply(&file);
    std::fs::write(path, updated).map_err(|e| format!("could not write {}: {e}", nice(path)))?;
    Ok(made)
}

/// Breaks a sentence into lines that fit a width.
fn wrap(text: &str, width: f32, size: f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if crate::stamps::about_as_wide(&candidate, size) > width && !line.is_empty() {
            out.push(std::mem::take(&mut line));
            line = word.to_string();
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// Reads a seal's picture off disk.
///
/// PNG and JPEG, which is what a scanned seal is. Anything else is refused by
/// name rather than producing an empty stamp somebody only notices on paper.
fn read_picture(path: &std::path::Path) -> Result<egui::ColorImage, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", nice(path)))?;
    let decoded = image::load_from_memory(&bytes)
        .map_err(|e| format!("{} could not be read as a picture: {e}", nice(path)))?;
    let rgba = decoded.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    Ok(egui::ColorImage {
        size: [w, h],
        pixels: rgba
            .pixels()
            .map(|p| egui::Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
            .collect(),
        source_size: egui::vec2(w as f32, h as f32),
    })
}

/// Whether a document has any form fields in it.
///
/// Asked before a form environment is set up, so nothing is built for the
/// ninety-nine drawings out of a hundred that have no form on them.
fn has_a_form(bindings: &dyn PdfiumLibraryBindings, doc: FPDF_DOCUMENT) -> bool {
    if doc.is_null() {
        return false;
    }
    let pages = unsafe { bindings.FPDF_GetPageCount(doc) };
    // Only the first few sheets: a form is on the front of a transmittal, and
    // walking two hundred sheets of a drawing set to find out there is none
    // costs more than it saves.
    for index in 0..pages.min(8) {
        let page = unsafe { bindings.FPDF_LoadPage(doc, index) };
        if page.is_null() {
            continue;
        }
        let count = unsafe { bindings.FPDFPage_GetAnnotCount(page) };
        let mut found = false;
        for at in 0..count {
            let annot = unsafe { bindings.FPDFPage_GetAnnot(page, at) };
            if annot.is_null() {
                continue;
            }
            let kind = unsafe { bindings.FPDFAnnot_GetSubtype(annot) };
            unsafe { bindings.FPDFPage_CloseAnnot(annot) };
            // 20 is a widget, which is what a form field is.
            if kind == 20 {
                found = true;
                break;
            }
        }
        unsafe { bindings.FPDF_ClosePage(page) };
        if found {
            return true;
        }
    }
    false
}

/// Bytes nobody can guess, for a key or a salt.
///
/// One copy of this, in the crate that needs it twice over -- a key that locks
/// a drawing set, and an initialisation vector on every piece of it. Two
/// copies of a random number generator is two things to get wrong.
fn fresh_bytes<const N: usize>() -> [u8; N] {
    pdf::random::bytes::<N>()
}

/// Writes the markups a flatten turned into paint into the flattened file, so
/// Unflatten has something to put back.
///
/// Best effort on purpose: a flatten that worked must not be reported as
/// failed because the record could not be written. Nought back means there was
/// nothing to keep, or the file would not take it, and Unflatten then says
/// plainly that there is nothing to lift.
fn keep_what_was_flattened(
    source: &std::path::Path,
    pages: &[u32],
    flattened: &std::path::Path,
) -> usize {
    let Ok(before) = pdf::Document::open(source) else {
        return 0;
    };
    let wanted: std::collections::HashSet<u32> = pages.iter().copied().collect();
    let mut marks: Vec<(u32, pdf::Dict)> = Vec::new();
    for page in 0..before.page_count() {
        if !wanted.contains(&(page as u32)) {
            continue;
        }
        for (_, markup) in annot::place::read_page(&before, page) {
            marks.push((page as u32, markup.dict.clone()));
        }
    }
    if marks.is_empty() {
        return 0;
    }
    let Ok(after) = pdf::Document::open(flattened) else {
        return 0;
    };
    let mut update = pdf::write::Update::new(&after);
    crate::flatkeep::write(&mut update, &after, &marks);
    let written = update.apply(&after);
    if std::fs::write(flattened, written).is_err() {
        return 0;
    }
    marks.len()
}

/// Why a file in a batch produced no operation.
///
/// Nearly always slip-sheeting with no revision of that name — which is worth
/// saying in those words, because the alternative reading ("the batch skipped
/// it") leaves somebody wondering whether it worked.
fn no_pairing(work: &crate::batch::Job) -> &'static str {
    match work {
        crate::batch::Job::SlipSheet { .. } => {
            "there is no revision with this file's name in the revisions folder, \
             so nothing was slipped in — a revision is matched by file name, and \
             a near match is not used"
        }
        crate::batch::Job::Overlay { .. } => {
            "there is no file with this name in the older folder, so it was not \
             laid over anything — an issue is matched by file name, and a near \
             match is not used"
        }
        crate::batch::Job::Compare { .. } => {
            "there is no file with this name in the older folder, so it was not \
             compared against anything — an issue is matched by file name, and a \
             near match is not used"
        }
        _ => "that job does not apply to one file at a time",
    }
}

/// A size somebody can compare at a glance.
fn megabytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{} KB", (bytes / 1024).max(1))
    }
}

/// A file name as somebody would say it, not a path as a machine would.
fn nice(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

use crate::docops::Done;

/// Two issues of a sheet printed over each other.
pub struct Overlaid {
    pub image: egui::ColorImage,
    pub agreement: f32,
    /// False when the two could not be lined up, so what is shown is the two
    /// sheets as they are rather than as they would be if they matched.
    pub lined_up: bool,
    pub size: [f64; 2], // WHAT: the newer sheet's size in points. WHY: the overlay is written on paper exactly that size.
    pub said: String,   // WHAT: one line saying what it shows and how it was lined up. WHY: shown under the picture.
}

/// How many pixels a side, at most, the overlay window's picture has.
const PREVIEW_PIXELS: usize = 2048; // WHAT: the window's copy. WHY: every graphics card takes a picture this big.
/// How many pixels a side the sheets drawn for picking points have.
const LOOK_PIXELS: u32 = 1400; // WHAT: big enough to click on a grid line. WHY: and small enough to come back at once.
/// An overlay's resolution, in dots an inch.
const OVERLAY_DPI: f64 = 150.0; // WHAT: 150 dots an inch. WHY: bolts and dimension text stay readable printed and zoomed.
/// The most pixels an overlay has, for a sheet too big for 150 dots an inch.
const OVERLAY_PIXELS: f64 = 12_000_000.0; // WHAT: twelve million. WHY: an ARCH E sheet at full resolution would need more memory than a Remote Desktop seat can spare.

/// One sheet as the fit search sees it.
struct SheetLook { // WHAT: a sheet ready for fitting. WHY: its drawing, size and coordinates.
    luma: Vec<u8>,                // WHAT: its drawing, one byte a pixel. WHY: ink is found in it.
    width: usize,                 // WHAT: pixels across. WHY: the ink's size.
    height: usize,                // WHAT: pixels down. WHY: the same.
    k: f64,                       // WHAT: pixels per point. WHY: fits go between pixels and points.
    size: [f64; 2],               // WHAT: the sheet in points, as shown. WHY: the overlay's paper.
    own: Option<[f64; 6]>,        // WHAT: the file's coordinates to the sheet's. WHY: the likeliest fit between two plots.
}

/// How the older sheet came to be where it is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Lined { // WHAT: how a fit came about. WHY: said differently each way.
    Found,     // WHAT: lined up by the line-work. WHY: the usual case.
    NotFound,  // WHAT: tried and could not, so left as they are. WHY: said, so its colours are not read as change.
    Points,    // WHAT: lined up by two points somebody picked. WHY: said.
    AsTheyAre, // WHAT: left as they are because that was asked. WHY: said.
}

/// What fitting two sheets found.
struct Fitted { // WHAT: what fitting found. WHY: overlay and comparison both use it.
    fit: crate::overlay::Fit,         // WHAT: older sheet points to newer sheet points. WHY: the overlay is drawn through it.
    fit_px: crate::overlay::Fit,      // WHAT: the same fit in working pixels. WHY: a comparison warps the ink through it.
    agreement: f32,                   // WHAT: how much of the line-work agrees. WHY: said.
    how: Lined,                       // WHAT: how it was lined up. WHY: said.
    older_size: [f64; 2],             // WHAT: the older sheet in points. WHY: said when it differs.
    newer_size: [f64; 2],             // WHAT: the newer sheet in points. WHY: the overlay's paper.
    newer_k: f64,                     // WHAT: working pixels per point, newer sheet. WHY: back to points.
    older_ink: crate::compare::Ink,   // WHAT: the older's line-work. WHY: a comparison uses it.
    newer_ink: crate::compare::Ink,   // WHAT: the newer's. WHY: the same.
}

impl Fitted { // WHAT: what to say about a fit. WHY: one line under the picture.
    /// One line saying what the colours are and how the sheets were lined up.
    fn says(&self) -> String { // WHAT: the line. WHY: the colours and how they were lined up.
        let colours = "Red is the older issue, blue is the newer one, dark grey is both."; // WHAT: the legend. WHY: always said first.
        let percent = self.agreement * 100.0; // WHAT: the agreement as a percentage. WHY: people read percentages.
        let mut extra = Vec::new(); // WHAT: what the line-up did beyond a shift. WHY: a turn or a rescale is worth knowing.
        let mut turn = self.fit.degrees(); // WHAT: the turn, 0 to 360. WHY: said below.
        if turn > 180.0 { // WHAT: more than half a turn. WHY: said as a small turn the other way.
            turn -= 360.0; // WHAT: as a small turn either way. WHY: "-0.6°" reads better than "359.4°".
        }
        if turn.abs() >= 45.0 { // WHAT: a big turn. WHY: said as turned.
            extra.push(format!("turned {:.0}°", turn.abs())); // WHAT: a quarter or half turn. WHY: the sheet was stored or scanned turned.
        } else if turn.abs() >= 0.05 { // WHAT: a small turn. WHY: said as straightened.
            extra.push(format!("straightened {:.1}°", turn.abs())); // WHAT: a skew taken out. WHY: a crooked scan.
        }
        if (self.fit.scale - 1.0).abs() >= 0.01 { // WHAT: drawn bigger or smaller. WHY: said.
            extra.push(format!("drawn at {:.0}% to match", self.fit.scale * 100.0)); // WHAT: rescaled. WHY: a half-size print or a different paper size.
        }
        if (self.older_size[0] - self.newer_size[0]).abs() > 1.0 || (self.older_size[1] - self.newer_size[1]).abs() > 1.0 { // WHAT: different paper sizes. WHY: said.
            extra.push(format!("the sheets are different sizes ({:.0} by {:.0} and {:.0} by {:.0} inches)", self.older_size[0] / 72.0, self.older_size[1] / 72.0, self.newer_size[0] / 72.0, self.newer_size[1] / 72.0)); // WHAT: both paper sizes. WHY: so a scale change is no surprise.
        }
        let extra = if extra.is_empty() { String::new() } else { format!(" ({})", extra.join("; ")) }; // WHAT: the extras in brackets. WHY: one line.
        match self.how { // WHAT: by how it was lined up. WHY: each says it differently.
            Lined::Found => format!("{colours} Lined up by the drawing{extra}: {percent:.0}% of the line-work matches. Nothing has been measured."), // WHAT: lined up. WHY: the usual message.
            Lined::NotFound => format!("These two sheets could not be lined up — only {percent:.0}% of the line-work matches. They are shown as they are, uncorrected, so the colours do not mean change. Try lining them up by two points. {colours}"), // WHAT: not lined up. WHY: so nobody reads the colours as a revision.
            Lined::Points => format!("{colours} Lined up by the two points picked{extra}: {percent:.0}% of the line-work matches. Nothing has been measured."), // WHAT: by hand. WHY: said.
            Lined::AsTheyAre => format!("{colours} Shown as they are, not lined up: {percent:.0}% of the line-work matches. Nothing has been measured."), // WHAT: uncorrected, as asked. WHY: said.
        }
    }
}

/// The fit the two files' own coordinates give: where the newer sheet shows
/// a point of the file, after undoing where the older sheet shows it.
///
/// None unless that is a turn and a shift (with any scale), which it is for
/// any two real pages; anything else would be a mirror or a shear.
fn own_fit(older: [f64; 6], newer: [f64; 6]) -> Option<crate::overlay::Fit> { // WHAT: the fit the files' coordinates give. WHY: the first start.
    let det = older[0] * older[3] - older[1] * older[2]; // WHAT: the older transform's determinant. WHY: zero means it cannot be undone.
    if det.abs() < 1e-12 { // WHAT: cannot be undone. WHY: no fit.
        return None; // WHAT: cannot be undone. WHY: no fit follows.
    }
    let inv = [older[3] / det, -older[1] / det, -older[2] / det, older[0] / det]; // WHAT: the older's turn and scale undone. WHY: sheet back to file.
    let inv_e = -(inv[0] * older[4] + inv[2] * older[5]); // WHAT: its shift undone, across. WHY: the same.
    let inv_f = -(inv[1] * older[4] + inv[3] * older[5]); // WHAT: and down. WHY: the same.
    let a = newer[0] * inv[0] + newer[2] * inv[1]; // WHAT: the composed transform's first number. WHY: older sheet to file to newer sheet.
    let b = newer[1] * inv[0] + newer[3] * inv[1]; // WHAT: its second. WHY: the same.
    let c = newer[0] * inv[2] + newer[2] * inv[3]; // WHAT: its third. WHY: the same.
    let d = newer[1] * inv[2] + newer[3] * inv[3]; // WHAT: its fourth. WHY: the same.
    let e = newer[0] * inv_e + newer[2] * inv_f + newer[4]; // WHAT: its shift across. WHY: the same.
    let f = newer[1] * inv_e + newer[3] * inv_f + newer[5]; // WHAT: its shift down. WHY: the same.
    let scale = a.hypot(b); // WHAT: its scale. WHY: a turn-and-scale has the same in both columns.
    if scale < 1e-9 || (a - d).abs() > 1e-3 * scale || (b + c).abs() > 1e-3 * scale { // WHAT: a mirror, a shear or nothing. WHY: not a fit to draw.
        return None; // WHAT: a mirror or a shear. WHY: not a fit an overlay can draw honestly.
    }
    Some(crate::overlay::Fit { scale, turn: b.atan2(a), dx: e, dy: f }) // WHAT: the fit. WHY: the first start tried.
}

/// Coloured pixels as a picture egui can show.
fn colour_image(pixels: &[[u8; 3]], width: usize, height: usize) -> egui::ColorImage { // WHAT: coloured pixels as an egui picture. WHY: shown or written.
    egui::ColorImage { size: [width, height], pixels: pixels.iter().map(|p| egui::Color32::from_rgb(p[0], p[1], p[2])).collect(), source_size: egui::vec2(width as f32, height as f32) } // WHAT: the picture. WHY: shown, or written into a page.
}

/// A picture's pixels as plain colours.
fn pixels_of(image: &egui::ColorImage) -> Vec<[u8; 3]> { // WHAT: an egui picture as plain colours. WHY: made smaller.
    image.pixels.iter().map(|p| [p.r(), p.g(), p.b()]).collect() // WHAT: red, green and blue. WHY: made smaller by the overlay module.
}

/// What a comparison found, in the newer sheet's own coordinates.
#[derive(Clone, Debug)]
pub struct Compared {
    /// One line somebody can read.
    pub said: String,
    pub could_not_align: bool,
    pub agreement: f32,
    /// Each difference: what kind, and where on the newer sheet, in points.
    pub changes: Vec<(crate::compare::Kind, [f64; 4])>,
}

/// A drawing's bytes, read once and shared by every pdfium that has it open.
///
/// Read, not memory-mapped. A mapped file stays open for as long as the
/// drawing is on screen, and Windows will not let anybody replace a file that
/// is mapped — so a drawing on the office's file share could not be saved
/// while it was open, which is the only time anybody saves one. A mapped file
/// on a share also takes the program down if the network blinks while a page
/// is being drawn. Holding the bytes costs memory the size of the file, once,
/// however many helpers are drawing it; it costs no file handle at all.
mod held {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, OnceLock, Weak};
    use std::time::SystemTime;

    type Key = (PathBuf, u64, Option<SystemTime>);

    fn all() -> &'static Mutex<HashMap<Key, Weak<Vec<u8>>>> {
        static ALL: OnceLock<Mutex<HashMap<Key, Weak<Vec<u8>>>>> = OnceLock::new();
        ALL.get_or_init(Default::default)
    }

    /// The bytes of the file as it is now: the copy another pdfium already
    /// holds when the file has not changed since, or a fresh read when it has.
    pub fn bytes(path: &Path) -> std::io::Result<Arc<Vec<u8>>> {
        let meta = std::fs::metadata(path)?;
        let key: Key = (path.to_path_buf(), meta.len(), meta.modified().ok());
        {
            let mut map = all().lock().unwrap_or_else(|e| e.into_inner());
            map.retain(|_, weak| weak.strong_count() > 0);
            if let Some(found) = map.get(&key).and_then(Weak::upgrade) {
                return Ok(found);
            }
        }
        let read = Arc::new(std::fs::read(path)?);
        let mut map = all().lock().unwrap_or_else(|e| e.into_inner());
        map.insert(key, Arc::downgrade(&read));
        Ok(read)
    }
}

/// One drawing pdfium has open.
struct Loaded {
    id: u64,
    /// Held for the life of the document: pdfium keeps a pointer into it.
    map: Option<std::sync::Arc<Vec<u8>>>,
    doc: FPDF_DOCUMENT,
    sizes: Vec<PageSize>,
    pages: Vec<(u32, FPDF_PAGE)>,
}

/// How many drawings stay open at once.
///
/// Tabs mean several are, and each one costs a mapped file and pdfium's own
/// bookkeeping. Beyond this the least recently used is let go; reopening takes
/// about fifteen milliseconds, which nobody notices, whereas six sixty-megabyte
/// sets mapped at once on a shop machine is something they would.
const OPEN_DOCUMENTS: usize = 6;

struct Engine {
    bindings: Box<dyn PdfiumLibraryBindings>,
    /// Least recently used first.
    open: Vec<Loaded>,
    /// Where each drawing came from, so one that was let go can be brought
    /// back without the window having to notice.
    known: Vec<(u64, PathBuf)>,
    /// A form environment per open document, made only for the documents that
    /// turn out to have a form in them.
    ///
    /// Pdfium draws form fields in a pass of their own, and only when it has
    /// one of these. Without it a transmittal with twenty boxes on it prints
    /// as twenty empty spaces — which is the sort of thing nobody notices
    /// until the plot is in somebody's hand.
    ///
    /// The description pdfium is given is kept alongside the handle on
    /// purpose: pdfium holds the pointer for the life of the environment, and
    /// letting it go out of scope is a crash a few frames later, somewhere
    /// else entirely.
    forms: Vec<(FPDF_DOCUMENT, FPDF_FORMHANDLE, Box<FPDF_FORMFILLINFO>)>,
    /// The form environment for whatever is being drawn right now.
    drawing_form: Option<FPDF_FORMHANDLE>,
    /// Passwords typed this session, by file. Held here and nowhere else: not
    /// written to disk, not in the preferences, gone when the program closes.
    /// Kept at all because a drawing dropped from the list above is opened
    /// again behind the scenes, and asking somebody for the same password a
    /// second time because the program forgot is not acceptable.
    passwords: Vec<(PathBuf, String)>,
}

/// What went wrong opening a drawing.
pub struct Trouble {
    pub said: String,
    /// True when a password is all that is missing.
    pub wants_password: bool,
}

impl From<String> for Trouble {
    fn from(said: String) -> Trouble {
        Trouble { said, wants_password: false }
    }
}

/// What to tell the user when pdfium will not open a file, from its
/// `FPDF_GetLastError` code: 1 unknown, 2 file, 3 format, 4 password,
/// 5 security. Only 4 means a password would help. A file that is not a
/// PDF at all (a bid file, a spreadsheet) comes back as 3.
fn open_trouble(code: std::os::raw::c_ulong, password_given: bool) -> Trouble {
    match code {
        2 => Trouble::from("the file could not be read".to_string()),
        3 => Trouble::from(
            "this file is not a PDF, or it is damaged and pdfium could not repair it".to_string(),
        ),
        // 4 is "no" to a password: none was given, or the one given was wrong.
        4 => Trouble {
            said: if password_given {
                "that password was not accepted".to_string()
            } else {
                "this drawing set is locked".to_string()
            },
            wants_password: true,
        },
        5 => Trouble::from("this PDF uses a kind of protection pdfium does not support".to_string()),
        _ => Trouble::from(format!("pdfium could not open the file (error {code})")),
    }
}

impl From<Trouble> for String {
    fn from(trouble: Trouble) -> String {
        trouble.said
    }
}

impl Engine {
    fn new(bindings: Box<dyn PdfiumLibraryBindings>) -> Engine {
        Engine {
            bindings,
            open: Vec::new(),
            known: Vec::new(),
            forms: Vec::new(),
            drawing_form: None,
            passwords: Vec::new(),
        }
    }

    /// Brings a drawing to the front, opening it again if it was let go.
    fn current(&mut self, id: u64) -> Option<usize> {
        if let Some(at) = self.open.iter().position(|l| l.id == id) {
            let one = self.open.remove(at);
            self.open.push(one);
            return Some(self.open.len() - 1);
        }
        let path = self.known.iter().find(|(i, _)| *i == id)?.1.clone();
        self.load(id, &path).ok()?;
        Some(self.open.len() - 1)
    }

    fn page_count(&mut self, id: u64) -> u32 {
        match self.current(id) {
            Some(at) => self.open[at].sizes.len() as u32,
            None => 0,
        }
    }

    /// The file a drawing was opened from.
    fn path_of(&self, id: u64) -> Option<PathBuf> {
        self.known.iter().find(|(i, _)| *i == id).map(|(_, p)| p.clone())
    }

    fn close_one(&mut self, id: u64) {
        if let Some(at) = self.open.iter().position(|l| l.id == id) {
            let one = self.open.remove(at);
            self.let_go(one);
        }
        self.known.retain(|(i, _)| *i != id);
    }

    /// A sheet's line-work, as straight segments in sheet space, for
    /// snapping onto.
    ///
    /// Every path on the sheet, including those inside forms (which is where
    /// a CAD program usually puts a whole drawing), through each object's own
    /// transform, and then onto the sheet the way pdfium itself lays the page
    /// out — so a page box that does not start at the origin, or a sheet
    /// rotated in the file, lands where it is drawn.
    fn geometry(&mut self, id: u64, index: u32) -> Option<takeoff::snap::Geometry> {
        let size = self.size(id, index)?;
        let page = self.page(id, index)?;
        let to_sheet = self.sheet_transform(page, size)?;
        let mut tracer = takeoff::snap::Tracer::default();
        let count = unsafe { self.bindings.FPDFPage_CountObjects(page) }.max(0);
        for i in 0..count {
            let object = unsafe { self.bindings.FPDFPage_GetObject(page, i) };
            self.trace(object, IDENTITY, &to_sheet, &mut tracer, 0);
            if tracer.segments.len() >= takeoff::snap::MOST {
                break;
            }
        }
        let (segments, centres) = tracer.finish();
        Some(takeoff::snap::Geometry::new((size.width, size.height), segments, centres))
    }

    /// A sheet as a plugin is handed it: the words printed on it and its
    /// straight line-work, both in sheet space, with each line's pen width.
    fn reading(&mut self, id: u64, index: u32, words: bool, lines: bool) -> Option<Reading> {
        let size = self.size(id, index)?;
        let page = self.page(id, index)?;
        self.read_page(page, size, words, lines)
    }

    /// The same, for a file that is not open in a window.
    fn reading_file(&mut self, path: &std::path::Path, index: u32, words: bool, lines: bool) -> Result<Reading, String> {
        let one = self.open_alone(path)?;
        let mut s = FS_SIZEF { width: 612.0, height: 792.0 };
        unsafe { self.bindings.FPDF_GetPageSizeByIndexF(one.doc, index as i32, &mut s) };
        let page = unsafe { self.bindings.FPDF_LoadPage(one.doc, index as i32) };
        if page.is_null() {
            self.close_alone(one);
            return Err(format!("sheet {} of {} would not open", index + 1, nice(path)));
        }
        let reading = self.read_page(page, PageSize { width: s.width, height: s.height }, words, lines);
        unsafe { self.bindings.FPDF_ClosePage(page) };
        self.close_alone(one);
        reading.ok_or_else(|| "the sheet could not be read".into())
    }

    fn read_page(&mut self, page: FPDF_PAGE, size: PageSize, words: bool, lines: bool) -> Option<Reading> {
        let to_sheet = self.sheet_transform(page, size)?;
        let mut out = Reading { size: (size.width as f64, size.height as f64), ..Reading::default() };
        if words {
            let text = unsafe { self.bindings.FPDFText_LoadPage(page) };
            if !text.is_null() {
                // Read with no origin, so each box comes back as PDF space
                // with its y turned over, and then put onto the sheet exactly
                // the way the line-work is.
                let runs = self.runs(text, 0.0, 0.0);
                unsafe { self.bindings.FPDFText_ClosePage(text) };
                for run in runs {
                    let said = run.text.trim();
                    if said.is_empty() {
                        continue;
                    }
                    let corners = [
                        (run.area.x0 as f64, -run.area.y0 as f64),
                        (run.area.x1 as f64, -run.area.y1 as f64),
                        (run.area.x0 as f64, -run.area.y1 as f64),
                        (run.area.x1 as f64, -run.area.y0 as f64),
                    ];
                    let mut area = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
                    for (x, y) in corners {
                        let sx = to_sheet[0] * x + to_sheet[2] * y + to_sheet[4];
                        let sy = to_sheet[1] * x + to_sheet[3] * y + to_sheet[5];
                        area = [area[0].min(sx), area[1].min(sy), area[2].max(sx), area[3].max(sy)];
                    }
                    out.words.push(plugin_api::Word {
                        text: said.to_string(),
                        area,
                        size: run.size as f64,
                    });
                }
            }
        }
        if lines {
            let count = unsafe { self.bindings.FPDFPage_CountObjects(page) }.max(0);
            for i in 0..count {
                let object = unsafe { self.bindings.FPDFPage_GetObject(page, i) };
                self.trace_lines(object, IDENTITY, &to_sheet, &mut out, 0);
                if out.lines.len() >= MOST_LINES_FOR_A_PLUGIN {
                    out.cut_short = true;
                    break;
                }
            }
        }
        Some(out)
    }

    /// The straight, stroked pieces of one object, with their pen width.
    /// Fills are left out — a filled shape is hatching or lettering, not a
    /// line somebody drew.
    fn trace_lines(
        &mut self,
        object: FPDF_PAGEOBJECT,
        outer: [f64; 6],
        to_sheet: &[f64; 6],
        out: &mut Reading,
        depth: u32,
    ) {
        if object.is_null() || depth > 12 || out.lines.len() >= MOST_LINES_FOR_A_PLUGIN {
            return;
        }
        let mut m = FS_MATRIX { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };
        unsafe { self.bindings.FPDFPageObj_GetMatrix(object, &mut m) };
        let own = [m.a as f64, m.b as f64, m.c as f64, m.d as f64, m.e as f64, m.f as f64];
        let here = then(own, outer);
        let kind = unsafe { self.bindings.FPDFPageObj_GetType(object) } as u32;
        if kind == OBJECT_FORM {
            let n = unsafe { self.bindings.FPDFFormObj_CountObjects(object) }.max(0);
            for i in 0..n {
                let inner = unsafe { self.bindings.FPDFFormObj_GetObject(object, i as _) };
                self.trace_lines(inner, here, to_sheet, out, depth + 1);
            }
            return;
        }
        if kind != OBJECT_PATH {
            return;
        }
        let (mut fill, mut stroke) = (0, 0);
        unsafe { self.bindings.FPDFPath_GetDrawMode(object, &mut fill, &mut stroke) };
        if stroke == 0 {
            return;
        }
        let whole = then(here, *to_sheet);
        let mut pen = 1.0f32;
        unsafe { self.bindings.FPDFPageObj_GetStrokeWidth(object, &mut pen) };
        // A width of nothing is the thinnest line the device can draw.
        let width = (pen as f64).max(0.0) * (whole[0] * whole[3] - whole[1] * whole[2]).abs().sqrt();
        let dashed = unsafe { self.bindings.FPDFPageObj_GetDashCount(object) } > 0;
        let point = |x: f32, y: f32| -> [f64; 2] {
            let (x, y) = (x as f64, y as f64);
            [
                whole[0] * x + whole[2] * y + whole[4],
                whole[1] * x + whole[3] * y + whole[5],
            ]
        };
        let n = unsafe { self.bindings.FPDFPath_CountSegments(object) }.max(0);
        let mut start: Option<[f64; 2]> = None;
        let mut at: Option<[f64; 2]> = None;
        for i in 0..n {
            let segment = unsafe { self.bindings.FPDFPath_GetPathSegment(object, i) };
            if segment.is_null() {
                continue;
            }
            let (mut x, mut y) = (0f32, 0f32);
            unsafe { self.bindings.FPDFPathSegment_GetPoint(segment, &mut x, &mut y) };
            let p = point(x, y);
            match unsafe { self.bindings.FPDFPathSegment_GetType(segment) } as u32 {
                SEGMENT_MOVETO => {
                    start = Some(p);
                }
                SEGMENT_LINETO => {
                    if let Some(a) = at {
                        push_line(out, a, p, width, dashed);
                    }
                }
                // A curve's pieces are not lines anybody would take off.
                _ => {}
            }
            at = Some(p);
            if unsafe { self.bindings.FPDFPathSegment_GetClose(segment) } != 0 {
                if let (Some(a), Some(b)) = (at, start) {
                    push_line(out, a, b, width, dashed);
                }
                at = start;
            }
        }
    }

    /// Page space to sheet space, as an affine `[a, b, c, d, e, f]`, read off
    /// pdfium's own page layout at a thousand times the size so it is exact to
    /// a thousandth of a point.
    fn sheet_transform(&mut self, page: FPDF_PAGE, size: PageSize) -> Option<[f64; 6]> {
        const K: f64 = 1000.0;
        let (w, h) = ((size.width as f64 * K).round() as i32, (size.height as f64 * K).round() as i32);
        let at = |x: f64, y: f64| -> Option<(f64, f64)> {
            let (mut dx, mut dy) = (0i32, 0i32);
            let ok = unsafe { self.bindings.FPDF_PageToDevice(page, 0, 0, w, h, 0, x, y, &mut dx, &mut dy) };
            (ok != 0).then_some((dx as f64 / K, dy as f64 / K))
        };
        let o = at(0.0, 0.0)?;
        let x = at(1000.0, 0.0)?;
        let y = at(0.0, 1000.0)?;
        Some([
            (x.0 - o.0) / 1000.0,
            (x.1 - o.1) / 1000.0,
            (y.0 - o.0) / 1000.0,
            (y.1 - o.1) / 1000.0,
            o.0,
            o.1,
        ])
    }

    fn trace(
        &mut self,
        object: FPDF_PAGEOBJECT,
        outer: [f64; 6],
        to_sheet: &[f64; 6],
        tracer: &mut takeoff::snap::Tracer,
        depth: u32,
    ) {
        if object.is_null() || depth > 12 {
            return;
        }
        let mut m = FS_MATRIX { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };
        unsafe { self.bindings.FPDFPageObj_GetMatrix(object, &mut m) };
        let own = [m.a as f64, m.b as f64, m.c as f64, m.d as f64, m.e as f64, m.f as f64];
        let here = then(own, outer);
        let kind = unsafe { self.bindings.FPDFPageObj_GetType(object) } as u32;
        if kind == OBJECT_FORM {
            let n = unsafe { self.bindings.FPDFFormObj_CountObjects(object) }.max(0);
            for i in 0..n {
                let inner = unsafe { self.bindings.FPDFFormObj_GetObject(object, i as _) };
                self.trace(inner, here, to_sheet, tracer, depth + 1);
            }
            return;
        }
        if kind != OBJECT_PATH {
            return;
        }
        let whole = then(here, *to_sheet);
        let point = |x: f32, y: f32| -> [f32; 2] {
            let (x, y) = (x as f64, y as f64);
            [
                (whole[0] * x + whole[2] * y + whole[4]) as f32,
                (whole[1] * x + whole[3] * y + whole[5]) as f32,
            ]
        };
        let n = unsafe { self.bindings.FPDFPath_CountSegments(object) }.max(0);
        let mut curve: Vec<[f32; 2]> = Vec::with_capacity(3);
        for i in 0..n {
            let segment = unsafe { self.bindings.FPDFPath_GetPathSegment(object, i) };
            if segment.is_null() {
                continue;
            }
            let (mut x, mut y) = (0f32, 0f32);
            unsafe { self.bindings.FPDFPathSegment_GetPoint(segment, &mut x, &mut y) };
            let p = point(x, y);
            match unsafe { self.bindings.FPDFPathSegment_GetType(segment) } as u32 {
                SEGMENT_MOVETO => {
                    curve.clear();
                    tracer.move_to(p);
                }
                SEGMENT_LINETO => {
                    curve.clear();
                    tracer.line_to(p);
                }
                SEGMENT_BEZIERTO => {
                    curve.push(p);
                    if curve.len() == 3 {
                        tracer.curve_to(curve[0], curve[1], curve[2]);
                        curve.clear();
                    }
                }
                _ => {}
            }
            if unsafe { self.bindings.FPDFPathSegment_GetClose(segment) } != 0 {
                tracer.close();
            }
        }
    }

    /// Batch ▸ Print: every sheet of a file to the usual printer, fitted to
    /// its paper and turned to suit it.
    fn print_whole(&mut self, source: &std::path::Path) -> Result<Done, String> {
        let (printer, settings) = crate::winprint::usual()
            .ok_or("There is no printer set up on this computer.")?;
        let draw = crate::winprint::render_to_dc(BOUND.get().map(|p| p.as_path()))
            .ok_or("This copy of the PDF engine cannot draw onto a printer.")?;
        let id = PRINT_DOC - 1;
        let sizes = self.open(id, source, "")?;
        let title = nice(source);
        let printed = (|| {
            let mut run = crate::winprint::Job::start(&printer, &settings, &title)?;
            for index in 0..sizes.len() as u32 {
                if let Err(why) = self.print_page(
                    id,
                    index,
                    &mut run,
                    crate::winprint::Fit::ToPaper,
                    crate::winprint::Orientation::Auto,
                    draw,
                ) {
                    run.abandon();
                    return Err(why);
                }
            }
            run.finish()
        })();
        self.close_one(id);
        printed?;
        Ok(Done {
            wrote: Vec::new(),
            said: format!("{title}: {} sheet(s) sent to {printer}.", sizes.len()),
        })
    }

    /// Draws one sheet onto the printer, as vectors, sized and turned to suit
    /// the paper.
    fn print_page(
        &mut self,
        id: u64,
        index: u32,
        run: &mut crate::winprint::Job,
        fit: crate::winprint::Fit,
        orientation: crate::winprint::Orientation,
        draw: crate::winprint::RenderToDc,
    ) -> Result<(), String> {
        let size = self.size(id, index).ok_or("that sheet is not in the file")?;
        let page = self.page(id, index).ok_or("that sheet could not be read")?;
        let at = crate::winprint::place((size.width, size.height), run.paper(), fit, orientation);
        run.page(|hdc| unsafe {
            draw(
                hdc,
                page as *mut std::ffi::c_void,
                at.x,
                at.y,
                at.w,
                at.h,
                at.rotate,
                crate::winprint::PRINT_FLAGS,
            )
        })
    }

    fn let_go(&mut self, mut one: Loaded) {
        for (_, p) in std::mem::take(&mut one.pages) {
            unsafe { self.bindings.FPDF_ClosePage(p) };
        }
        if !one.doc.is_null() {
            unsafe { self.bindings.FPDF_CloseDocument(one.doc) };
        }
        // The bytes go last: pdfium held a pointer into them until now.
        one.map = None;
        drop(one);
    }

    fn open(&mut self, id: u64, path: &std::path::Path, password: &str) -> Result<Vec<PageSize>, Trouble> {
        // Opening the same id again replaces what was there, which is what
        // happens when a drawing is saved and reopened.
        self.close_one(id);
        self.known.push((id, path.to_path_buf()));
        if !password.is_empty() {
            self.passwords.retain(|(p, _)| p != path);
            self.passwords.push((path.to_path_buf(), password.to_string()));
        }
        self.load(id, path)
    }

    /// The password typed for this file, if one was.
    fn password_for(&self, path: &std::path::Path) -> String {
        self.passwords
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, w)| w.clone())
            .unwrap_or_default()
    }

    /// Pulls chosen sheets out into a file of their own.
    ///
    /// Copied through pdfium rather than cut out of the bytes by hand, because
    /// a page is not a self-contained thing in a PDF — it refers to fonts,
    /// images and colour spaces that live elsewhere in the file, and an
    /// extractor that does not follow all of them produces sheets that open
    /// blank. Pdfium already knows how to follow them, and it brings the
    /// annotations with the page, which is the whole point: the markups *are*
    /// annotations, so what comes out is what somebody would see on paper.
    // ---- whole-drawing operations -----------------------------------------
    //
    // These work on files rather than on the open documents, because most of
    // them are about drawings nobody has open — combining four issues into one
    // set, slip-sheeting a revision that arrived this morning. So each opens
    // what it needs, does the work, writes a new file and closes everything.
    //
    // None of them touches the file it read. Where a name is already taken, a
    // number is added. A drawing set is the file of record for a bid and this
    // program does not get to destroy one because somebody chose the wrong menu
    // item.

    fn document_operation(&mut self, op: &crate::docops::Operation) -> Result<Done, String> {
        use crate::docops::Operation;
        match op {
            Operation::Combine { sources, to } => self.combine(sources, to),
            Operation::Split { source, into, every } => self.split(source, into, *every),
            Operation::Extract { source, pages, to, and_remove } => {
                self.extract_out(source, pages, to, *and_remove)
            }
            Operation::Insert { source, insert, at, to } => self.insert(source, insert, *at, to),
            Operation::Delete { source, pages, to } => self.delete(source, pages, to),
            Operation::Rotate { source, pages, quarter_turns, to } => {
                self.rotate(source, pages, *quarter_turns, to)
            }
            Operation::Crop { source, pages, box_, to } => self.crop(source, pages, *box_, to),
            Operation::SlipSheet { source, revisions, to, keep_superseded } => {
                self.slip_sheet(source, revisions, to, *keep_superseded)
            }
            Operation::Flatten { source, pages, to } => self.flatten(source, pages, to),
            Operation::Stamp { source, pages, stamps, to } => {
                self.stamp(source, pages, stamps, to)
            }
            Operation::Shrink { source, to, dpi } => self.shrink(source, to, *dpi),
            Operation::Compare { newer, older, to, darkness } => {
                self.compare_whole_set(newer, older, to, *darkness)
            }
            Operation::Overlay { newer, older, to, darkness } => {
                self.overlay_whole_set(newer, older, to, *darkness)
            }
            Operation::OverlaySheet { newer, newer_page, older, older_page, darkness, lining, to } => self.overlay_one_sheet(newer, *newer_page, older, *older_page, *darkness, lining, to), // WHAT: one sheet's overlay, written as a file. WHY: so it can be opened, printed and sent.
            Operation::Print { source } => self.print_whole(source),
            Operation::Summary { sources, to } => self.summary_of_many(sources, to),
            Operation::Secure {
                source,
                to,
                open_password,
                owner_password,
                allowed,
            } => self.secure(source, to, open_password, owner_password, *allowed),
            Operation::NewBlank { to, sheets, size } => {
                self.new_blank(to, *sheets, *size)
            }
            Operation::FromPictures { pictures, to, dpi } => {
                self.from_pictures(pictures, to, *dpi)
            }
            Operation::Repair { source, to } => self.repair(source, to),
            Operation::PageLabels { source, labels, to } => {
                self.write_page_labels(source, labels, to)
            }
            Operation::Unflatten { source, to } => self.unflatten(source, to),
            Operation::ApplyRedactions { source, to, as_pictures, dpi } => {
                self.apply_redactions(source, to, *as_pictures, *dpi)
            }
            Operation::ExportPictures { source, pages, into, dpi, format } => {
                self.export_pictures(source, pages, into, *dpi, format)
            }
            Operation::Seal { source, picture, pages, placement, to } => {
                self.seal(source, picture, pages, *placement, to)
            }
            Operation::Ocr { source, pages, to, dpi, only_scans } => {
                let Some(engine) = crate::ocr::engine_here() else {
                    return Err(crate::ocr::NO_ENGINE.to_string());
                };
                self.ocr(source, pages, to, *dpi, *only_scans, engine.as_ref(), |_, _, _| {})
            }
        }
    }

    /// Reads the words off scanned sheets and writes them back as an invisible
    /// text layer.
    ///
    /// The page's own picture is left exactly as it is. What is added is text
    /// drawn in render mode 3 — invisible — sitting over the marks it was read
    /// from, which is how every OCR'd PDF in the world works and why a scan can
    /// be searched without looking any different.
    fn ocr(
        &mut self,
        source: &std::path::Path,
        pages: &[u32],
        to: &std::path::Path,
        dpi: u32,
        only_sheets_without_text: bool,
        engine: &dyn crate::ocr::Engine,
        mut progress: impl FnMut(usize, usize, usize),
    ) -> Result<Done, String> {
        let one = self.open_alone(source)?;
        let count = one.count;
        let all: Vec<u32> = (0..count as u32).collect();
        let fresh = match self.assemble(&[(one.doc, all)]) {
            Ok(fresh) => fresh,
            Err(why) => {
                self.close_alone(one);
                return Err(why);
            }
        };
        self.close_alone(one);

        let font = unsafe { self.bindings.FPDFText_LoadStandardFont(fresh, "Helvetica") };
        if font.is_null() {
            unsafe { self.bindings.FPDF_CloseDocument(fresh) };
            return Err("the text layer's font could not be loaded".into());
        }

        let scale = (dpi.clamp(100, 600) as f64) / 72.0;
        let wanted: Vec<u32> = pages.iter().copied().filter(|p| (*p as usize) < count).collect();
        let total = wanted.len();
        let mut read = 0usize;
        let mut skipped = 0usize;
        let mut words_put = 0usize;

        for (n, index) in wanted.iter().enumerate() {
            progress(n, total, words_put);
            let page = unsafe { self.bindings.FPDF_LoadPage(fresh, *index as i32) };
            if page.is_null() {
                continue;
            }

            // A sheet that already has text is left alone by default. Running
            // OCR over a drawing that was never scanned adds a second, worse
            // copy of every word underneath the good one, and search then finds
            // things twice.
            if only_sheets_without_text {
                let text = unsafe { self.bindings.FPDFText_LoadPage(page) };
                let chars = if text.is_null() {
                    0
                } else {
                    let n = unsafe { self.bindings.FPDFText_CountChars(text) };
                    unsafe { self.bindings.FPDFText_ClosePage(text) };
                    n
                };
                if chars > 20 {
                    unsafe { self.bindings.FPDF_ClosePage(page) };
                    skipped += 1;
                    continue;
                }
            }

            let width = unsafe { self.bindings.FPDF_GetPageWidthF(page) } as f64;
            let height = unsafe { self.bindings.FPDF_GetPageHeightF(page) } as f64;
            let across = (width * scale).round().max(1.0) as u32;
            let down = (height * scale).round().max(1.0) as u32;
            let Some(image) = self.raster(page, scale, 0.0, 0.0, across, down) else {
                unsafe { self.bindings.FPDF_ClosePage(page) };
                continue;
            };

            let grey: Vec<u8> = image
                .pixels
                .iter()
                .map(|p| {
                    ((p.r() as u32 + p.g() as u32 + p.b() as u32) / 3) as u8
                })
                .collect();
            let words = match engine.read(&grey, across, down) {
                Ok(words) => crate::ocr::worth_keeping(words),
                Err(why) => {
                    unsafe { self.bindings.FPDF_ClosePage(page) };
                    unsafe { self.bindings.FPDF_CloseDocument(fresh) };
                    return Err(why);
                }
            };
            let placed = crate::ocr::place(&words, (across, down), (width as f32, height as f32));

            for word in &placed {
                let object = unsafe {
                    self.bindings
                        .FPDFPageObj_CreateTextObj(fresh, font, word.size)
                };
                if object.is_null() {
                    continue;
                }
                if !self
                    .bindings
                    .is_true(unsafe { self.bindings.FPDFText_SetText_str(object, &word.text) })
                {
                    continue;
                }
                // Render mode 3: draw nothing. The word is there to be found,
                // not to be seen — the page already shows it, in ink.
                unsafe { self.bindings.FPDFTextObj_SetTextRenderMode(object, 3) };
                unsafe {
                    self.bindings.FPDFPageObj_Transform(
                        object,
                        1.0,
                        0.0,
                        0.0,
                        1.0,
                        word.x as f64,
                        word.y as f64,
                    )
                };
                unsafe { self.bindings.FPDFPage_InsertObject(page, object) };
                words_put += 1;
            }
            if !placed.is_empty() {
                unsafe { self.bindings.FPDFPage_GenerateContent(page) };
                read += 1;
            }
            unsafe { self.bindings.FPDF_ClosePage(page) };
        }
        progress(total, total, words_put);

        let wrote = self.write_out(fresh, to);
        unsafe { self.bindings.FPDF_CloseDocument(fresh) };
        wrote?;

        let mut said = format!(
            "{read} sheet{} read, {words_put} word{} added. They are invisible — the \
             drawing looks exactly as it did, and now it can be searched.",
            if read == 1 { "" } else { "s" },
            if words_put == 1 { "" } else { "s" }
        );
        if skipped > 0 {
            said.push_str(&format!(
                " {skipped} sheet{} already had text and {} left alone.",
                if skipped == 1 { "" } else { "s" },
                if skipped == 1 { "was" } else { "were" }
            ));
        }
        said.push_str(&format!(" Read by {}.", engine.name()));
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said,
        })
    }

    /// Writes a takeoff summary out as an ordinary PDF.
    ///
    /// Built page by page rather than by stamping an existing drawing, because
    /// this is a new document: a report somebody opens in whatever they already
    /// read PDFs with, prints, and puts in a bid folder. Nothing in it needs
    /// Hyperview.
    fn write_report(
        &mut self,
        report: &crate::report::Report,
        to: &std::path::Path,
    ) -> Result<Done, String> {
        use crate::report::{FIRST_TOP, LINE, MARGIN, PAGE, SIZE, TOP};

        let doc = unsafe { self.bindings.FPDF_CreateNewDocument() };
        if doc.is_null() {
            return Err("could not make the report".into());
        }
        let font = unsafe { self.bindings.FPDFText_LoadStandardFont(doc, "Helvetica") };
        let bold = unsafe { self.bindings.FPDFText_LoadStandardFont(doc, "Helvetica-Bold") };
        if font.is_null() {
            unsafe { self.bindings.FPDF_CloseDocument(doc) };
            return Err("the report's font could not be loaded".into());
        }
        let bold = if bold.is_null() { font } else { bold };

        let usable = PAGE.0 - MARGIN * 2.0;
        let mut columns: Vec<f32> = Vec::with_capacity(report.widths.len());
        let mut at = MARGIN;
        for width in &report.widths {
            columns.push(at);
            at += width * usable;
        }

        for (number, page) in report.pages.iter().enumerate() {
            let handle = unsafe {
                self.bindings
                    .FPDFPage_New(doc, number as i32, PAGE.0 as f64, PAGE.1 as f64)
            };
            if handle.is_null() {
                continue;
            }
            let mut y = PAGE.1 - MARGIN;

            if number == 0 {
                self.write_text(doc, handle, bold, &report.title, MARGIN, y - 16.0, 17.0, [0, 0, 0]);
                y -= 34.0;
                self.write_text(doc, handle, font, &report.drawing, MARGIN, y, 11.0, [60, 60, 60]);
                y -= 16.0;
                self.write_text(
                    doc,
                    handle,
                    font,
                    &format!("{} · {}", report.who, report.when),
                    MARGIN,
                    y,
                    9.0,
                    [110, 110, 110],
                );
                y -= 18.0;
                if !report.short_by.is_empty() {
                    // The warning goes above the table, in the colour the rest
                    // of the program uses for it, because somebody bidding off
                    // a short takeoff is the worst thing this program can let
                    // happen.
                    for piece in wrap(&report.short_by, usable, 9.0) {
                        self.write_text(
                            doc, handle, bold, &piece, MARGIN, y, 9.0, [180, 90, 20],
                        );
                        y -= 12.0;
                    }
                    y -= 6.0;
                }
            }

            // The column headings, on every page, because page four of a
            // takeoff with no headings is a page of numbers.
            for (at, heading) in report.headings.iter().enumerate() {
                let x = columns.get(at).copied().unwrap_or(MARGIN);
                let room = report.widths.get(at).copied().unwrap_or(0.1) * usable - 6.0;
                self.write_text(
                    doc,
                    handle,
                    bold,
                    &crate::report::fit(heading, room),
                    x,
                    y,
                    SIZE,
                    [40, 40, 40],
                );
            }
            y -= 4.0;
            self.rule(doc, handle, MARGIN, y, PAGE.0 - MARGIN);
            y -= LINE;
            let _ = if number == 0 { FIRST_TOP } else { TOP };

            for line in &page.lines {
                for (at, cell) in line.cells.iter().enumerate() {
                    if cell.is_empty() {
                        continue;
                    }
                    let x = columns.get(at).copied().unwrap_or(MARGIN);
                    let room = report.widths.get(at).copied().unwrap_or(0.1) * usable - 6.0;
                    self.write_text(
                        doc,
                        handle,
                        if line.heading { bold } else { font },
                        &crate::report::fit(cell, room),
                        x,
                        y,
                        SIZE,
                        if line.heading { [0, 0, 0] } else { [30, 30, 30] },
                    );
                }
                y -= LINE;
            }

            self.write_text(
                doc,
                handle,
                font,
                &format!("Page {} of {}", number + 1, report.pages.len()),
                PAGE.0 - MARGIN - 60.0,
                MARGIN - 14.0,
                8.0,
                [140, 140, 140],
            );
            unsafe { self.bindings.FPDFPage_GenerateContent(handle) };
            unsafe { self.bindings.FPDF_ClosePage(handle) };
        }

        let wrote = self.write_out(doc, to);
        unsafe { self.bindings.FPDF_CloseDocument(doc) };
        wrote?;
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said: format!(
                "The summary is in {} — {} page{}.",
                nice(to),
                report.pages.len(),
                if report.pages.len() == 1 { "" } else { "s" }
            ),
        })
    }

    /// One piece of text on a page.
    fn write_text(
        &self,
        doc: FPDF_DOCUMENT,
        page: FPDF_PAGE,
        font: pdfium_render::prelude::FPDF_FONT,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        colour: [u8; 3],
    ) {
        if text.is_empty() {
            return;
        }
        let object = unsafe { self.bindings.FPDFPageObj_CreateTextObj(doc, font, size) };
        if object.is_null() {
            return;
        }
        if !self
            .bindings
            .is_true(unsafe { self.bindings.FPDFText_SetText_str(object, text) })
        {
            return;
        }
        unsafe {
            self.bindings.FPDFPageObj_SetFillColor(
                object,
                colour[0] as u32,
                colour[1] as u32,
                colour[2] as u32,
                255,
            )
        };
        unsafe {
            self.bindings
                .FPDFPageObj_Transform(object, 1.0, 0.0, 0.0, 1.0, x as f64, y as f64)
        };
        unsafe { self.bindings.FPDFPage_InsertObject(page, object) };
    }

    /// A hairline under the column headings.
    fn rule(&self, doc: FPDF_DOCUMENT, page: FPDF_PAGE, from: f32, y: f32, to: f32) {
        let path = unsafe {
            self.bindings
                .FPDFPageObj_CreateNewPath(from as f32, y as f32)
        };
        if path.is_null() {
            return;
        }
        let _ = doc;
        unsafe { self.bindings.FPDFPath_LineTo(path, to as f32, y as f32) };
        unsafe { self.bindings.FPDFPageObj_SetStrokeColor(path, 150, 150, 150, 255) };
        unsafe { self.bindings.FPDFPageObj_SetStrokeWidth(path, 0.5) };
        unsafe { self.bindings.FPDFPath_SetDrawMode(path, 0, 1) };
        unsafe { self.bindings.FPDFPage_InsertObject(page, path) };
    }

    /// Compares every sheet of two issues and clouds what changed.
    ///
    /// Sheet for sheet by position, not by sheet number, because two issues of
    /// a set have the same sheets in the same order — and when they do not,
    /// that is worth reporting rather than silently comparing S-201 against
    /// S-300.
    ///
    /// The clouds are written by the same code the viewer writes markups with:
    /// a copy of the newer file, with real annotations appended by incremental
    /// update. They are markups like any other — editable, deletable, and
    /// visible in Revu — rather than something only this program understands.
    fn compare_whole_set(
        &mut self,
        newer: &std::path::Path,
        older: &std::path::Path,
        to: &std::path::Path,
        darkness: f32,
    ) -> Result<Done, String> {
        let new_one = self.open_alone(newer)?;
        let new_count = new_one.count;
        self.close_alone(new_one);
        let old_one = self.open_alone(older)?;
        let old_count = old_one.count;
        self.close_alone(old_one);

        let mut found_on: Vec<(usize, Vec<(crate::compare::Kind, [f64; 4])>)> = Vec::new();
        let mut unaligned: Vec<usize> = Vec::new();
        for page in 0..new_count.min(old_count) {
            let found =
                match self.compare_sheets(older, page as u32, newer, page as u32, darkness) {
                    Ok(found) => found,
                    Err(_) => continue,
                };
            if found.could_not_align {
                unaligned.push(page + 1);
                continue;
            }
            if !found.changes.is_empty() {
                found_on.push((page, found.changes));
            }
        }

        // The newer issue, copied whole, then the clouds appended to the copy.
        std::fs::copy(newer, to)
            .map_err(|e| format!("could not write {}: {e}", nice(to)))?;
        let clouded = write_clouds(to, "Compare Documents", &found_on)?;

        let sheets_with_changes = found_on.len();
        let mut said = if clouded == 0 {
            format!("Nothing changed between these two issues. {} written.", nice(to))
        } else {
            format!(
                "{clouded} difference{} on {sheets_with_changes} sheet{}, clouded in {}. \
                 Nothing has been measured — the clouds say where to look.",
                if clouded == 1 { "" } else { "s" },
                if sheets_with_changes == 1 { "" } else { "s" },
                nice(to)
            )
        };
        if new_count != old_count {
            said.push_str(&format!(
                " The two issues have different numbers of sheets ({new_count} and \
                 {old_count}); only the first {} were compared.",
                new_count.min(old_count)
            ));
        }
        if !unaligned.is_empty() {
            said.push_str(&format!(
                " {} sheet{} could not be lined up and {} left unmarked: {}.",
                unaligned.len(),
                if unaligned.len() == 1 { "" } else { "s" },
                if unaligned.len() == 1 { "was" } else { "were" },
                unaligned
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said,
        })
    }

    /// Puts a picture of a seal on chosen sheets.
    fn seal(
        &mut self,
        source: &std::path::Path,
        picture: &std::path::Path,
        pages: &[u32],
        placement: crate::seal::Placement,
        to: &std::path::Path,
    ) -> Result<Done, String> {
        let image = read_picture(picture)?;
        let one = self.open_alone(source)?;
        let count = one.count;
        let all: Vec<u32> = (0..count as u32).collect();
        let fresh = match self.assemble(&[(one.doc, all)]) {
            Ok(fresh) => fresh,
            Err(why) => {
                self.close_alone(one);
                return Err(why);
            }
        };
        self.close_alone(one);

        let mut sealed = 0usize;
        for index in pages {
            if (*index as usize) >= count {
                continue;
            }
            let page = unsafe { self.bindings.FPDF_LoadPage(fresh, *index as i32) };
            if page.is_null() {
                continue;
            }
            let width = unsafe { self.bindings.FPDF_GetPageWidthF(page) };
            let height = unsafe { self.bindings.FPDF_GetPageHeightF(page) };
            // A placement of nothing means "somewhere sensible", which is the
            // bottom-right corner of whatever size this sheet happens to be —
            // and a set is not always all one size.
            let placement = match placement.corner {
                // A corner is worked out per sheet, because a batch runs over
                // sets on different paper and the bottom right of a letter
                // page is nowhere near the bottom right of an ARCH E1.
                Some(_) => placement.on(
                    (width, height),
                    (image.size[0] as u32, image.size[1] as u32),
                ),
                None if placement.x == 0.0 && placement.y == 0.0 => {
                    crate::seal::Placement::suggested((width, height), placement.width)
                }
                None => placement,
            };
            let box_ = placement.box_for((image.size[0] as u32, image.size[1] as u32));
            if self.put_picture(fresh, page, &image, box_) {
                unsafe { self.bindings.FPDFPage_GenerateContent(page) };
                sealed += 1;
            }
            unsafe { self.bindings.FPDF_ClosePage(page) };
        }

        let wrote = self.write_out(fresh, to);
        unsafe { self.bindings.FPDF_CloseDocument(fresh) };
        wrote?;
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said: format!(
                "The seal is on {sealed} sheet{}, in {}. It is a picture on the sheet, \
                 not a digital signature.",
                if sealed == 1 { "" } else { "s" },
                nice(to)
            ),
        })
    }

    /// Puts an image on a page at a given box.
    fn put_picture(
        &self,
        into: FPDF_DOCUMENT,
        page: FPDF_PAGE,
        image: &egui::ColorImage,
        box_: [f32; 4],
    ) -> bool {
        let (across, down) = (image.size[0] as i32, image.size[1] as i32);
        if across <= 0 || down <= 0 {
            return false;
        }
        let mut bgra: Vec<u8> = Vec::with_capacity((across * down * 4) as usize);
        for pixel in &image.pixels {
            bgra.push(pixel.b());
            bgra.push(pixel.g());
            bgra.push(pixel.r());
            bgra.push(pixel.a());
        }
        let bitmap = unsafe {
            self.bindings.FPDFBitmap_CreateEx(
                across,
                down,
                FPDFBITMAP_BGRA as i32,
                bgra.as_mut_ptr() as *mut c_void,
                across * 4,
            )
        };
        if bitmap.is_null() {
            return false;
        }
        let object = unsafe { self.bindings.FPDFPageObj_NewImageObj(into) };
        if object.is_null() {
            unsafe { self.bindings.FPDFBitmap_Destroy(bitmap) };
            return false;
        }
        let mut pages = [page];
        let set = unsafe {
            self.bindings
                .FPDFImageObj_SetBitmap(pages.as_mut_ptr(), 1, object, bitmap)
        };
        let ok = self.bindings.is_true(set);
        if ok {
            let w = (box_[2] - box_[0]) as f64;
            let h = (box_[3] - box_[1]) as f64;
            unsafe {
                self.bindings.FPDFPageObj_Transform(
                    object,
                    w,
                    0.0,
                    0.0,
                    h,
                    box_[0] as f64,
                    box_[1] as f64,
                )
            };
            unsafe { self.bindings.FPDFPage_InsertObject(page, object) };
        }
        unsafe { self.bindings.FPDFBitmap_Destroy(bitmap) };
        drop(bgra);
        ok
    }

    /// Makes one page of the new file out of a rendered picture of the old one.
    ///
    /// The picture is handed to pdfium as a bitmap and becomes an image object
    /// filling the page, which is what "a picture of the sheet" means in a PDF.
    fn page_from_picture(
        &self,
        into: FPDF_DOCUMENT,
        at: usize,
        width: f64,
        height: f64,
        image: &egui::ColorImage,
    ) -> bool {
        let (across, down) = (image.size[0] as i32, image.size[1] as i32);
        if across <= 0 || down <= 0 {
            return false;
        }
        // Pdfium wants BGRA, bottom row first is *not* required — but the
        // buffer must outlive the bitmap, which is why it is built here and
        // dropped only after the bitmap is destroyed.
        let mut bgra: Vec<u8> = Vec::with_capacity((across * down * 4) as usize);
        for pixel in &image.pixels {
            bgra.push(pixel.b());
            bgra.push(pixel.g());
            bgra.push(pixel.r());
            bgra.push(255);
        }
        let bitmap = unsafe {
            self.bindings.FPDFBitmap_CreateEx(
                across,
                down,
                FPDFBITMAP_BGRA as i32,
                bgra.as_mut_ptr() as *mut c_void,
                across * 4,
            )
        };
        if bitmap.is_null() {
            return false;
        }

        let page = unsafe { self.bindings.FPDFPage_New(into, at as i32, width, height) };
        if page.is_null() {
            unsafe { self.bindings.FPDFBitmap_Destroy(bitmap) };
            return false;
        }
        let object = unsafe { self.bindings.FPDFPageObj_NewImageObj(into) };
        if object.is_null() {
            unsafe { self.bindings.FPDFBitmap_Destroy(bitmap) };
            unsafe { self.bindings.FPDF_ClosePage(page) };
            return false;
        }
        let mut pages = [page];
        let set = unsafe {
            self.bindings
                .FPDFImageObj_SetBitmap(pages.as_mut_ptr(), 1, object, bitmap)
        };
        let ok = self.bindings.is_true(set);
        if ok {
            // An image object is a unit square until it is told otherwise, so
            // the transform is what makes it fill the sheet.
            unsafe {
                self.bindings
                    .FPDFPageObj_Transform(object, width, 0.0, 0.0, height, 0.0, 0.0)
            };
            unsafe { self.bindings.FPDFPage_InsertObject(page, object) };
            unsafe { self.bindings.FPDFPage_GenerateContent(page) };
        }
        unsafe { self.bindings.FPDFBitmap_Destroy(bitmap) };
        unsafe { self.bindings.FPDF_ClosePage(page) };
        drop(bgra);
        ok
    }

    /// Writes text into the corners of chosen sheets.
    fn stamp(
        &mut self,
        source: &std::path::Path,
        pages: &[u32],
        stamps: &[crate::stamps::Stamp],
        to: &std::path::Path,
    ) -> Result<Done, String> {
        use std::ffi::CString;

        if stamps.iter().all(|s| s.text.trim().is_empty()) {
            return Err("there is nothing to stamp".into());
        }
        let one = self.open_alone(source)?;
        let total = one.count;
        let numbers = self.sheet_numbers(one.doc, one.count);
        let all: Vec<u32> = (0..one.count as u32).collect();
        let fresh = match self.assemble(&[(one.doc, all)]) {
            Ok(fresh) => fresh,
            Err(why) => {
                self.close_alone(one);
                return Err(why);
            }
        };
        self.close_alone(one);

        let file = nice(source);
        let date = crate::stamps::today();
        // One font for the whole job. Helvetica because it is one of the
        // fourteen every reader has, so a stamped set opens the same everywhere
        // and carries no embedded font.
        let font_name = CString::new("Helvetica").expect("a literal");
        let font = unsafe {
            self.bindings
                .FPDFText_LoadStandardFont(fresh, font_name.to_str().unwrap_or("Helvetica"))
        };
        if font.is_null() {
            unsafe { self.bindings.FPDF_CloseDocument(fresh) };
            return Err("the stamping font could not be loaded".into());
        }

        let mut stamped = 0usize;
        for page_index in pages {
            if (*page_index as usize) >= total {
                continue;
            }
            let page = unsafe { self.bindings.FPDF_LoadPage(fresh, *page_index as i32) };
            if page.is_null() {
                continue;
            }
            let width = unsafe { self.bindings.FPDF_GetPageWidthF(page) };
            let height = unsafe { self.bindings.FPDF_GetPageHeightF(page) };
            let sheet = numbers
                .get(*page_index as usize)
                .cloned()
                .unwrap_or_default();

            let mut put_anything = false;
            for stamp in stamps {
                let text = crate::stamps::fill(
                    &stamp.text,
                    *page_index as usize,
                    total,
                    &sheet,
                    &file,
                    &date,
                );
                if text.trim().is_empty() {
                    continue;
                }
                let object = unsafe {
                    self.bindings
                        .FPDFPageObj_CreateTextObj(fresh, font, stamp.size)
                };
                if object.is_null() {
                    continue;
                }
                let set = unsafe { self.bindings.FPDFText_SetText_str(object, &text) };
                if !self.bindings.is_true(set) {
                    continue;
                }
                unsafe {
                    self.bindings.FPDFPageObj_SetFillColor(
                        object,
                        stamp.colour[0] as u32,
                        stamp.colour[1] as u32,
                        stamp.colour[2] as u32,
                        255,
                    )
                };
                let wide = crate::stamps::about_as_wide(&text, stamp.size);
                let (x, y) = stamp.spot.place((width, height), wide, stamp.margin);
                // Identity with a translation: the text keeps its own size and
                // is simply put where it belongs.
                unsafe {
                    self.bindings.FPDFPageObj_Transform(
                        object,
                        1.0,
                        0.0,
                        0.0,
                        1.0,
                        x as f64,
                        y as f64,
                    )
                };
                unsafe { self.bindings.FPDFPage_InsertObject(page, object) };
                put_anything = true;
            }
            if put_anything {
                unsafe { self.bindings.FPDFPage_GenerateContent(page) };
                stamped += 1;
            }
            unsafe { self.bindings.FPDF_ClosePage(page) };
        }

        let wrote = self.write_out(fresh, to);
        unsafe { self.bindings.FPDF_CloseDocument(fresh) };
        wrote?;
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said: format!(
                "{stamped} sheet{} stamped, in {}. The original is untouched.",
                if stamped == 1 { "" } else { "s" },
                nice(to)
            ),
        })
    }

    /// Renders every sheet into a new file, to make it small enough to send.
    fn shrink(
        &mut self,
        source: &std::path::Path,
        to: &std::path::Path,
        dpi: u32,
    ) -> Result<Done, String> {
        let one = self.open_alone(source)?;
        let count = one.count;
        let was = std::fs::metadata(source).map(|m| m.len()).unwrap_or(0);

        let fresh = unsafe { self.bindings.FPDF_CreateNewDocument() };
        if fresh.is_null() {
            self.close_alone(one);
            return Err("could not make a new file".into());
        }

        let scale = (dpi.clamp(50, 600) as f64) / 72.0;
        let mut done_pages = 0usize;
        for index in 0..count {
            let page = unsafe { self.bindings.FPDF_LoadPage(one.doc, index as i32) };
            if page.is_null() {
                continue;
            }
            let width = unsafe { self.bindings.FPDF_GetPageWidthF(page) } as f64;
            let height = unsafe { self.bindings.FPDF_GetPageHeightF(page) } as f64;
            let across = (width * scale).round().max(1.0) as u32;
            let down = (height * scale).round().max(1.0) as u32;
            let image = self.raster(page, scale, 0.0, 0.0, across, down);
            unsafe { self.bindings.FPDF_ClosePage(page) };
            let Some(image) = image else { continue };

            let made = self.page_from_picture(fresh, index, width, height, &image);
            if made {
                done_pages += 1;
            }
        }
        let wrote = self.write_out(fresh, to);
        unsafe { self.bindings.FPDF_CloseDocument(fresh) };
        self.close_alone(one);
        wrote?;

        let now = std::fs::metadata(to).map(|m| m.len()).unwrap_or(0);
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said: format!(
                "{done_pages} sheet{} rendered. {} — was {}. There is no text left to \
                 search in it; the original still has all of it.",
                if done_pages == 1 { "" } else { "s" },
                megabytes(now),
                megabytes(was)
            ),
        })
    }

    /// Opens a file for one operation, outside the viewer's cache.
    ///
    /// Separate from [`Engine::load`] on purpose: that one is the LRU of things
    /// somebody is looking at, and combining twelve files should not evict the
    /// drawing on screen.
    fn open_alone(&mut self, path: &std::path::Path) -> Result<Aside, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", nice(path)))?;
        // Looked at before pdfium is asked, because pdfium reports a file that
        // is not a PDF at all as "password protected" about as often as it
        // reports it as "not a PDF", and sending somebody to look for a
        // password on a Word document renamed to .pdf wastes an afternoon.
        if !bytes.starts_with(b"%PDF") {
            return Err(format!(
                "{} is not a PDF. It starts with something else, so whatever made it \
                 did not make a PDF.",
                nice(path)
            ));
        }
        let doc = unsafe { self.bindings.FPDF_LoadMemDocument64(&bytes, None) };
        if doc.is_null() {
            let code = unsafe { self.bindings.FPDF_GetLastError() };
            return Err(match code {
                1 => format!("{} is not a PDF.", nice(path)),
                2 => format!("{} is damaged and could not be repaired.", nice(path)),
                3 | 4 => format!("{} is password protected.", nice(path)),
                _ => format!("{} could not be opened (pdfium error {code}).", nice(path)),
            });
        }
        let count = unsafe { self.bindings.FPDF_GetPageCount(doc) }.max(0) as usize;
        // A form environment only for a document that actually has a form in
        // it, so nothing is set up for the ninety-nine drawings out of a
        // hundred that have none.
        self.drawing_form = if has_a_form(self.bindings.as_ref(), doc) {
            self.form_for(doc)
        } else {
            None
        };
        Ok(Aside { _bytes: bytes, doc, count })
    }

    fn close_alone(&mut self, one: Aside) {
        if !one.doc.is_null() {
            // The form environment has to go before the document it belongs
            // to, or pdfium is left holding a dead pointer.
            self.drop_form(one.doc);
            unsafe { self.bindings.FPDF_CloseDocument(one.doc) };
        }
    }

    /// A fresh empty document, and the pages listed copied into it.
    fn assemble(&self, from: &[(FPDF_DOCUMENT, Vec<u32>)]) -> Result<FPDF_DOCUMENT, String> {
        let fresh = unsafe { self.bindings.FPDF_CreateNewDocument() };
        if fresh.is_null() {
            return Err("could not make a new file".into());
        }
        let mut at = 0i32;
        for (source, pages) in from {
            if pages.is_empty() {
                continue;
            }
            let indices: Vec<i32> = pages.iter().map(|p| *p as i32).collect();
            let ok = unsafe {
                self.bindings.FPDF_ImportPagesByIndex(
                    fresh,
                    *source,
                    indices.as_ptr(),
                    indices.len() as std::os::raw::c_ulong,
                    at,
                )
            };
            if !self.bindings.is_true(ok) {
                unsafe { self.bindings.FPDF_CloseDocument(fresh) };
                return Err("pdfium would not copy those sheets".into());
            }
            at += indices.len() as i32;
        }
        Ok(fresh)
    }

    fn write_out(&self, doc: FPDF_DOCUMENT, to: &std::path::Path) -> Result<(), String> {
        let bytes = self.save_to_bytes(doc)?;
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(to, &bytes).map_err(|e| format!("could not write {}: {e}", nice(to)))
    }

    fn combine(&mut self, sources: &[std::path::PathBuf], to: &std::path::Path) -> Result<Done, String> {
        if sources.is_empty() {
            return Err("nothing to combine".into());
        }
        let mut open = Vec::new();
        let mut sheets = 0usize;
        for path in sources {
            match self.open_alone(path) {
                Ok(one) => {
                    sheets += one.count;
                    open.push(one);
                }
                Err(why) => {
                    for one in open {
                        self.close_alone(one);
                    }
                    return Err(why);
                }
            }
        }
        let plan: Vec<(FPDF_DOCUMENT, Vec<u32>)> = open
            .iter()
            .map(|one| (one.doc, (0..one.count as u32).collect()))
            .collect();
        let files: Vec<(String, usize)> = sources
            .iter()
            .zip(&open)
            .map(|(path, one)| {
                let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                (name, one.count)
            })
            .collect();
        let result = self
            .assemble(&plan)
            .and_then(|fresh| {
                let wrote = self.write_out(fresh, to);
                unsafe { self.bindings.FPDF_CloseDocument(fresh) };
                wrote
            });
        for one in open {
            self.close_alone(one);
        }
        result?;
        // A bookmark per file is a nicety: a combined set without them is
        // still the combined set, so a failure here is logged, not reported.
        if let Err(why) = crate::docops::bookmark_each_file(to, &files) {
            log::warn!("combined {} but could not bookmark it: {why}", nice(to));
        }
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said: format!(
                "{} sheets from {} file{} are now in {}.",
                sheets,
                sources.len(),
                if sources.len() == 1 { "" } else { "s" },
                nice(to)
            ),
        })
    }

    fn split(
        &mut self,
        source: &std::path::Path,
        into: &std::path::Path,
        every: usize,
    ) -> Result<Done, String> {
        let one = self.open_alone(source)?;
        let runs = crate::docops::in_runs_of(one.count, every);
        if runs.is_empty() {
            self.close_alone(one);
            return Err("that would make no files".into());
        }
        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "drawing".into());

        let mut wrote = Vec::new();
        let mut trouble = None;
        for (n, pages) in runs.iter().enumerate() {
            let first = pages.first().copied().unwrap_or(0) + 1;
            let last = pages.last().copied().unwrap_or(0) + 1;
            let label = if first == last {
                format!("{first}")
            } else {
                format!("{first}-{last}")
            };
            let to = crate::docops::free_name(&into.join(format!("{stem} {label}.pdf")));
            let made = self.assemble(&[(one.doc, pages.clone())]).and_then(|fresh| {
                let ok = self.write_out(fresh, &to);
                unsafe { self.bindings.FPDF_CloseDocument(fresh) };
                ok
            });
            match made {
                Ok(()) => wrote.push(to),
                Err(why) => {
                    trouble = Some(format!("after {} file(s): {why}", n));
                    break;
                }
            }
        }
        self.close_alone(one);
        if let Some(why) = trouble {
            // The ones already written are left. They are whole files and
            // deleting them because a later one failed helps nobody.
            return Err(why);
        }
        let count = wrote.len();
        Ok(Done {
            said: format!(
                "{count} file{} in {}.",
                if count == 1 { "" } else { "s" },
                nice(into)
            ),
            wrote,
        })
    }

    fn extract_out(
        &mut self,
        source: &std::path::Path,
        pages: &[u32],
        to: &std::path::Path,
        and_remove: bool,
    ) -> Result<Done, String> {
        let one = self.open_alone(source)?;
        let taking: Vec<u32> = pages.iter().copied().filter(|p| (*p as usize) < one.count).collect();
        if taking.is_empty() {
            self.close_alone(one);
            return Err("none of those sheets are in that drawing".into());
        }
        let mut wrote = Vec::new();
        let made = self.assemble(&[(one.doc, taking.clone())]).and_then(|fresh| {
            let ok = self.write_out(fresh, to);
            unsafe { self.bindings.FPDF_CloseDocument(fresh) };
            ok
        });
        if let Err(why) = made {
            self.close_alone(one);
            return Err(why);
        }
        wrote.push(to.to_path_buf());

        let mut said = format!(
            "{} sheet{} in {}.",
            taking.len(),
            if taking.len() == 1 { "" } else { "s" },
            nice(to)
        );

        if and_remove {
            // The rest, as a second file. Still not touching the original —
            // "move" means "here are both halves", not "your set is now
            // shorter and there is no going back".
            let rest = crate::docops::all_but(one.count, &taking);
            if rest.is_empty() {
                said.push_str(" Nothing was left over, so the original stands as it is.");
            } else {
                let without = crate::docops::beside(source, "without those sheets");
                let made = self.assemble(&[(one.doc, rest.clone())]).and_then(|fresh| {
                    let ok = self.write_out(fresh, &without);
                    unsafe { self.bindings.FPDF_CloseDocument(fresh) };
                    ok
                });
                match made {
                    Ok(()) => {
                        said.push_str(&format!(
                            " The other {} are in {}. The original is untouched.",
                            rest.len(),
                            nice(&without)
                        ));
                        wrote.push(without);
                    }
                    Err(why) => {
                        said.push_str(&format!(" The rest could not be written: {why}"));
                    }
                }
            }
        }
        self.close_alone(one);
        Ok(Done { wrote, said })
    }

    fn insert(
        &mut self,
        source: &std::path::Path,
        insert: &std::path::Path,
        at: usize,
        to: &std::path::Path,
    ) -> Result<Done, String> {
        let host = self.open_alone(source)?;
        let guest = match self.open_alone(insert) {
            Ok(guest) => guest,
            Err(why) => {
                self.close_alone(host);
                return Err(why);
            }
        };
        let at = at.min(host.count);
        let plan = vec![
            (host.doc, (0..at as u32).collect::<Vec<u32>>()),
            (guest.doc, (0..guest.count as u32).collect()),
            (host.doc, (at as u32..host.count as u32).collect()),
        ];
        let made = self.assemble(&plan).and_then(|fresh| {
            let ok = self.write_out(fresh, to);
            unsafe { self.bindings.FPDF_CloseDocument(fresh) };
            ok
        });
        let inserted = guest.count;
        let total = host.count + guest.count;
        self.close_alone(guest);
        self.close_alone(host);
        made?;
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said: format!(
                "{inserted} sheet{} inserted after sheet {at}. {total} sheets in {}.",
                if inserted == 1 { "" } else { "s" },
                nice(to)
            ),
        })
    }

    fn delete(
        &mut self,
        source: &std::path::Path,
        pages: &[u32],
        to: &std::path::Path,
    ) -> Result<Done, String> {
        let one = self.open_alone(source)?;
        let keeping = crate::docops::all_but(one.count, pages);
        if keeping.is_empty() {
            self.close_alone(one);
            return Err("that would leave no sheets at all".into());
        }
        let left_out = one.count - keeping.len();
        let made = self.assemble(&[(one.doc, keeping.clone())]).and_then(|fresh| {
            let ok = self.write_out(fresh, to);
            unsafe { self.bindings.FPDF_CloseDocument(fresh) };
            ok
        });
        self.close_alone(one);
        made?;
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said: format!(
                "{left_out} sheet{} left out. {} in {}. The original still has all of them.",
                if left_out == 1 { "" } else { "s" },
                keeping.len(),
                nice(to)
            ),
        })
    }

    fn rotate(
        &mut self,
        source: &std::path::Path,
        pages: &[u32],
        quarter_turns: i32,
        to: &std::path::Path,
    ) -> Result<Done, String> {
        let one = self.open_alone(source)?;
        // Copied whole first, then turned in the copy, so the file that was
        // read is never the file that is changed.
        let all: Vec<u32> = (0..one.count as u32).collect();
        let fresh = match self.assemble(&[(one.doc, all)]) {
            Ok(fresh) => fresh,
            Err(why) => {
                self.close_alone(one);
                return Err(why);
            }
        };
        let mut turned = 0usize;
        for page in pages {
            if (*page as usize) >= one.count {
                continue;
            }
            let handle = unsafe { self.bindings.FPDF_LoadPage(fresh, *page as i32) };
            if handle.is_null() {
                continue;
            }
            let was = unsafe { self.bindings.FPDFPage_GetRotation(handle) };
            let now = (was + quarter_turns).rem_euclid(4);
            unsafe { self.bindings.FPDFPage_SetRotation(handle, now) };
            unsafe { self.bindings.FPDF_ClosePage(handle) };
            turned += 1;
        }
        let wrote = self.write_out(fresh, to);
        unsafe { self.bindings.FPDF_CloseDocument(fresh) };
        self.close_alone(one);
        wrote?;
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said: format!(
                "{turned} sheet{} turned, in {}.",
                if turned == 1 { "" } else { "s" },
                nice(to)
            ),
        })
    }

    fn crop(
        &mut self,
        source: &std::path::Path,
        pages: &[u32],
        box_: [f32; 4],
        to: &std::path::Path,
    ) -> Result<Done, String> {
        let one = self.open_alone(source)?;
        let all: Vec<u32> = (0..one.count as u32).collect();
        let fresh = match self.assemble(&[(one.doc, all)]) {
            Ok(fresh) => fresh,
            Err(why) => {
                self.close_alone(one);
                return Err(why);
            }
        };
        let mut cropped = 0usize;
        for page in pages {
            if (*page as usize) >= one.count {
                continue;
            }
            let handle = unsafe { self.bindings.FPDF_LoadPage(fresh, *page as i32) };
            if handle.is_null() {
                continue;
            }
            // The crop box only, never the media box. The content outside stays
            // in the file, which is what makes a crop something somebody can
            // undo by cropping wider.
            unsafe {
                self.bindings
                    .FPDFPage_SetCropBox(handle, box_[0], box_[1], box_[2], box_[3])
            };
            unsafe { self.bindings.FPDF_ClosePage(handle) };
            cropped += 1;
        }
        let wrote = self.write_out(fresh, to);
        unsafe { self.bindings.FPDF_CloseDocument(fresh) };
        self.close_alone(one);
        wrote?;
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said: format!(
                "{cropped} sheet{} cropped, in {}. Nothing was thrown away — \
                 cropping wider brings it back.",
                if cropped == 1 { "" } else { "s" },
                nice(to)
            ),
        })
    }

    fn slip_sheet(
        &mut self,
        source: &std::path::Path,
        revisions: &std::path::Path,
        to: &std::path::Path,
        keep_superseded: bool,
    ) -> Result<Done, String> {
        let set = self.open_alone(source)?;
        let new = match self.open_alone(revisions) {
            Ok(new) => new,
            Err(why) => {
                self.close_alone(set);
                return Err(why);
            }
        };

        let set_numbers = self.sheet_numbers(set.doc, set.count);
        let new_numbers = self.sheet_numbers(new.doc, new.count);
        let plan = crate::docops::plan_slip(&set_numbers, &new_numbers);

        // Built as a list of (which file, which page) so the order of the set
        // is kept exactly and only the replaced sheets change.
        let mut order: Vec<(FPDF_DOCUMENT, u32)> = Vec::with_capacity(set.count);
        let mut superseded: Vec<u32> = Vec::new();
        for page in 0..set.count as u32 {
            match plan.replacing.iter().find(|(at, _)| *at == page) {
                Some((_, from)) => {
                    order.push((new.doc, *from));
                    superseded.push(page);
                }
                None => order.push((set.doc, page)),
            }
        }
        if keep_superseded {
            for page in &superseded {
                order.push((set.doc, *page));
            }
        }

        // Runs of the same source, so consecutive pages import in one call.
        let mut runs: Vec<(FPDF_DOCUMENT, Vec<u32>)> = Vec::new();
        for (doc, page) in order {
            match runs.last_mut() {
                Some((last, pages)) if *last == doc => pages.push(page),
                _ => runs.push((doc, vec![page])),
            }
        }

        let made = self.assemble(&runs).and_then(|fresh| {
            let ok = self.write_out(fresh, to);
            unsafe { self.bindings.FPDF_CloseDocument(fresh) };
            ok
        });
        self.close_alone(new);
        self.close_alone(set);
        made?;

        let mut said = plan.says();
        said.push_str(&format!(" Written to {}.", nice(to)));
        if keep_superseded && !superseded.is_empty() {
            said.push_str(&format!(
                " The {} superseded sheet{} are at the back.",
                superseded.len(),
                if superseded.len() == 1 { "" } else { "s" }
            ));
        }
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said,
        })
    }

    fn flatten(
        &mut self,
        source: &std::path::Path,
        pages: &[u32],
        to: &std::path::Path,
    ) -> Result<Done, String> {
        let one = self.open_alone(source)?;
        let all: Vec<u32> = (0..one.count as u32).collect();
        let fresh = match self.assemble(&[(one.doc, all)]) {
            Ok(fresh) => fresh,
            Err(why) => {
                self.close_alone(one);
                return Err(why);
            }
        };
        let mut flattened = 0usize;
        let mut refused = 0usize;
        for page in pages {
            if (*page as usize) >= one.count {
                continue;
            }
            let handle = unsafe { self.bindings.FPDF_LoadPage(fresh, *page as i32) };
            if handle.is_null() {
                continue;
            }
            // FLAT_NORMALDISPLAY: what the page looks like on screen, which is
            // what somebody flattening a drawing means.
            let outcome = unsafe { self.bindings.FPDFPage_Flatten(handle, 0) };
            unsafe { self.bindings.FPDFPage_GenerateContent(handle) };
            unsafe { self.bindings.FPDF_ClosePage(handle) };
            // 0 = FLATTEN_FAIL, 1 = FLATTEN_SUCCESS, 2 = FLATTEN_NOTHINGTODO.
            if outcome == 0 {
                refused += 1;
            } else {
                flattened += 1;
            }
        }
        let wrote = self.write_out(fresh, to);
        unsafe { self.bindings.FPDF_CloseDocument(fresh) };
        self.close_alone(one);
        wrote?;

        // Paint cannot be read back as a markup by looking at it, so the
        // markups themselves go into the flattened file as a record. That is
        // the whole of what makes Unflatten possible, and it is why Hyperview
        // can offer it where nothing else can.
        let kept = keep_what_was_flattened(source, pages, to);

        let mut said = format!(
            "{flattened} sheet{} flattened into {}. The original keeps its markups.",
            if flattened == 1 { "" } else { "s" },
            nice(to)
        );
        if kept > 0 {
            said.push_str(&format!(
                " {kept} markup{} kept inside the flattened file as well, so                  Unflatten can lift {} back out.",
                if kept == 1 { "" } else { "s" },
                if kept == 1 { "it" } else { "them" }
            ));
        }
        if refused > 0 {
            said.push_str(&format!(
                " {refused} sheet{} could not be flattened and {} copied as {} were.",
                if refused == 1 { "" } else { "s" },
                if refused == 1 { "was" } else { "were" },
                if refused == 1 { "it" } else { "they" }
            ));
        }
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said,
        })
    }


    /// Lays one issue of a whole set over another, sheet for sheet.
    ///
    /// The other half of a revision review, and the one people find easier to
    /// read: rather than a list of regions, the two issues are printed over
    /// each other, one in each colour, and everything that did not move comes
    /// out dark. Done across a set instead of a sheet at a time.
    ///
    /// Sheets are matched by sheet number, so a sheet put in the middle of a
    /// set does not push every sheet after it onto the wrong partner. Every
    /// newer sheet gets its page, in order — page 5 of the overlay is always
    /// sheet 5 of the newer set — and a sheet with nothing to lay it over is
    /// shown all in the newer colour rather than left out.
    fn overlay_whole_set(&mut self, newer: &std::path::Path, older: &std::path::Path, to: &std::path::Path, darkness: f32) -> Result<Done, String> { // WHAT: a whole set laid over another. WHY: the batch and Document menus.
        let b = self.open_alone(newer)?; // WHAT: the newer set. WHY: open for the whole run, not once a sheet.
        let a = match self.open_alone(older) { // WHAT: the older set. WHY: open for the whole run.
            Ok(a) => a, // WHAT: the older set. WHY: the same.
            Err(why) => { // WHAT: the older would not open. WHY: close the newer and say why.
                self.close_alone(b); // WHAT: closes the newer. WHY: nothing left open on the way out.
                return Err(why); // WHAT: why the older would not open. WHY: said plainly.
            }
        };
        let new_numbers = self.sheet_numbers(b.doc, b.count); // WHAT: the newer set's sheet numbers. WHY: sheets are matched by them.
        let old_numbers = self.sheet_numbers(a.doc, a.count); // WHAT: the older set's. WHY: the same.
        let pairs = crate::overlay::pair_sheets(&new_numbers, &old_numbers); // WHAT: which older sheet each newer sheet goes over. WHY: by number, else by place.
        let by_number = new_numbers.iter().any(|n| !n.trim().is_empty()) && old_numbers.iter().any(|n| !n.trim().is_empty()); // WHAT: were numbers used. WHY: said in the message.
        let fresh = unsafe { self.bindings.FPDF_CreateNewDocument() }; // WHAT: the overlay file. WHY: a new file; neither issue is touched.
        if fresh.is_null() { // WHAT: no new file. WHY: close both and say so.
            self.close_alone(a); // WHAT: closes both. WHY: nothing left open.
            self.close_alone(b); // WHAT: the same. WHY: the same.
            return Err("a new drawing could not be started".into()); // WHAT: says so. WHY: plainly.
        }
        let names = [nice(older), nice(newer)]; // WHAT: the two files' names. WHY: the legend and the messages.
        let label = |i: usize, numbers: &[String]| if numbers[i].trim().is_empty() { format!("sheet {}", i + 1) } else { numbers[i].trim().to_string() }; // WHAT: a sheet as a person names it. WHY: "S-101A", or "sheet 3" when it has no number.
        let (mut laid, mut alone, mut unaligned, mut failed) = (0usize, Vec::new(), Vec::new(), Vec::new()); // WHAT: what happened to each sheet. WHY: the message says it.
        for (i, pair) in pairs.iter().enumerate() { // WHAT: each newer sheet in order. WHY: page N is sheet N.
            let at = unsafe { self.bindings.FPDF_GetPageCount(fresh) }.max(0) as usize; // WHAT: where this sheet's page goes. WHY: at the end, so pages follow the newer set.
            let this = label(i, &new_numbers); // WHAT: this sheet's name. WHY: the messages.
            let mut made = false; // WHAT: did its page go in. WHY: if not, a blank keeps its place.
            if let Some(j) = pair { // WHAT: it has an older partner. WHY: laid over it.
                match self.overlay_open(a.doc, *j as u32, b.doc, i as u32, darkness, &crate::overlay::Lining::Auto, [&names[0], &names[1]]) { // WHAT: laid over, or why not. WHY: either is reported.
                    Ok(laid_over) => { // WHAT: laid over. WHY: written.
                        if !laid_over.lined_up { // WHAT: could not be lined up. WHY: listed.
                            unaligned.push(this.clone()); // WHAT: noted. WHY: the message lists sheets left as they stand.
                        }
                        let legend = format!("Overlay. Red: {} {}. Blue: {} {}. Dark grey: on both. Nothing has been measured.", names[0], label(*j, &old_numbers), names[1], this); // WHAT: which colour is which, and which sheets. WHY: a print has no dialog beside it.
                        made = self.overlay_page(fresh, at, &laid_over, &legend); // WHAT: the page. WHY: the overlay.
                        if made { // WHAT: its page went in. WHY: counted.
                            laid += 1; // WHAT: counted. WHY: the message.
                        }
                    }
                    Err(why) => failed.push(format!("{this} ({why})")), // WHAT: noted with the reason. WHY: a sheet is never dropped without saying.
                }
            } else {
                alone.push(this.clone()); // WHAT: noted. WHY: the message names new sheets.
            }
            if !made { // WHAT: no page yet. WHY: shown alone instead.
                if let Some(only) = self.newer_alone(b.doc, i as u32, darkness) { // WHAT: it draws alone. WHY: all in blue.
                    let legend = format!("Overlay. Blue: {} {}, which has no sheet to lay it over. Nothing has been measured.", names[1], this); // WHAT: says why it is all blue. WHY: the same as any other page.
                    made = self.overlay_page(fresh, at, &only, &legend); // WHAT: the sheet alone, all new. WHY: it keeps its place in the set.
                }
            }
            if !made { // WHAT: still no page. WHY: a blank keeps its place.
                failed.push(format!("{this} (it would not draw, so its page is blank)")); // WHAT: noted. WHY: said, never silent.
                let blank = unsafe { self.bindings.FPDFPage_New(fresh, at as i32, 612.0, 792.0) }; // WHAT: a blank page in its place. WHY: page N stays sheet N.
                if !blank.is_null() { // WHAT: the blank was made. WHY: closed.
                    unsafe { self.bindings.FPDF_ClosePage(blank) }; // WHAT: closes it. WHY: done.
                }
            }
        }
        let used: Vec<usize> = pairs.iter().flatten().copied().collect(); // WHAT: older sheets that were laid under something. WHY: the rest are only in the older issue.
        let only_older: Vec<String> = (0..a.count).filter(|j| !used.contains(j)).map(|j| label(j, &old_numbers)).collect(); // WHAT: older sheets with no newer partner. WHY: named, so nobody wonders.
        self.close_alone(a); // WHAT: closes the older. WHY: done reading.
        self.close_alone(b); // WHAT: closes the newer. WHY: the same.
        let wrote = self.write_out(fresh, to); // WHAT: writes the overlay file. WHY: the result.
        unsafe { self.bindings.FPDF_CloseDocument(fresh) }; // WHAT: closes it. WHY: done.
        wrote?; // WHAT: stops if it did not write. WHY: says why.

        let plural = |n: usize| if n == 1 { "" } else { "s" }; // WHAT: "sheet" or "sheets". WHY: messages read as English.
        let mut said = format!("{laid} sheet{} laid over each other in {}. Red is the older issue, blue the newer, dark grey both. Nothing has been measured.", plural(laid), nice(to)); // WHAT: what was done. WHY: the first thing to say.
        said.push_str(if by_number { " Sheets were matched by sheet number." } else { " Sheets were matched by their place in the set, because they have no sheet numbers to match by." }); // WHAT: how sheets were paired. WHY: so a wrong pairing is never a surprise.
        if !alone.is_empty() { // WHAT: sheets new in this issue. WHY: named.
            said.push_str(&format!(" New in this issue, shown all in blue: {}.", alone.join(", "))); // WHAT: sheets with no older issue. WHY: named.
        }
        if !only_older.is_empty() { // WHAT: sheets only in the older. WHY: named.
            said.push_str(&format!(" Only in the older issue, so not in the overlay: {}.", only_older.join(", "))); // WHAT: sheets that went. WHY: named.
        }
        if !unaligned.is_empty() {
            said.push_str(&format!(" Could not be lined up, so laid over as they stand: {}.", unaligned.join(", "))); // WHAT: sheets left uncorrected. WHY: their colours do not mean change.
        }
        if !failed.is_empty() { // WHAT: sheets that failed. WHY: named with why.
            said.push_str(&format!(" Could not be laid over: {}.", failed.join("; "))); // WHAT: sheets that failed, with why. WHY: never silent.
        }
        Ok(Done { wrote: vec![to.to_path_buf()], said }) // WHAT: the file and the message. WHY: the batch and Document menus show both.
    }

    /// One sheet laid over another and written as a one-page file, so it can
    /// be opened, zoomed, printed or sent like any other drawing.
    #[allow(clippy::too_many_arguments)] // WHAT: allows the long list. WHY: two sheets, the darkness, the line-up and where to write.
    fn overlay_one_sheet(&mut self, newer: &std::path::Path, newer_page: u32, older: &std::path::Path, older_page: u32, darkness: f32, lining: &crate::overlay::Lining, to: &std::path::Path) -> Result<Done, String> { // WHAT: one overlaid sheet written as a file. WHY: Save as PDF.
        let laid = self.overlay_sheets(older, older_page, newer, newer_page, darkness, lining)?; // WHAT: the overlay at full size. WHY: written out.
        let fresh = unsafe { self.bindings.FPDF_CreateNewDocument() }; // WHAT: the new file. WHY: neither issue is touched.
        if fresh.is_null() { // WHAT: no new file. WHY: say so.
            return Err("a new drawing could not be started".into()); // WHAT: says so. WHY: plainly.
        }
        let legend = format!("Overlay. Red: {} sheet {}. Blue: {} sheet {}. Dark grey: on both. Nothing has been measured.", nice(older), older_page + 1, nice(newer), newer_page + 1); // WHAT: which colour is which. WHY: on the paper itself.
        let made = self.overlay_page(fresh, 0, &laid, &legend); // WHAT: the page. WHY: the overlay.
        let wrote = if made { self.write_out(fresh, to) } else { Err("the overlay would not go into a page".to_string()) }; // WHAT: writes it, or says why not. WHY: plainly.
        unsafe { self.bindings.FPDF_CloseDocument(fresh) }; // WHAT: closes it. WHY: done.
        wrote?; // WHAT: stops if it did not write. WHY: says why.
        Ok(Done { wrote: vec![to.to_path_buf()], said: format!("{} {} written.", laid.said, nice(to)) }) // WHAT: the file and what it shows. WHY: opened straight away.
    }

    /// One takeoff report covering several sets.
    ///
    /// Every markup in every file, measured against the scale written into the
    /// sheet it is on. A sheet with no scale has its measurements reported and
    /// left out of the totals — never counted as zero — exactly as one set's
    /// own summary does, because a bid built on a total that quietly swallowed
    /// six unscaled sheets is a bid somebody loses money on.
    fn summary_of_many(
        &mut self,
        sources: &[std::path::PathBuf],
        to: &std::path::Path,
    ) -> Result<Done, String> {
        if sources.is_empty() {
            return Err("No files were chosen.".into());
        }
        let mut lines: Vec<crate::report::Line> = Vec::new();
        let mut unreadable: Vec<String> = Vec::new();
        let mut total_markups = 0usize;
        let mut unscaled = 0usize;
        let mut unscaled_pages: std::collections::BTreeSet<(String, usize)> =
            Default::default();

        for source in sources {
            let file = match pdf::Document::open(source) {
                Ok(file) => file,
                Err(_) => {
                    unreadable.push(nice(source));
                    continue;
                }
            };
            let name = nice(source);
            for page in 0..file.page_count() {
                let scale = file
                    .page(page)
                    .and_then(|p| annot::viewport::scale_of(&file, &p));
                for (reference, markup) in annot::place::read_page(&file, page) {
                    let row = takeoff::measure(page, reference, &markup, scale.as_ref());
                    if !markup.kind().measures() {
                        continue;
                    }
                    total_markups += 1;
                    if scale.is_none() {
                        unscaled += 1;
                        unscaled_pages.insert((name.clone(), page + 1));
                    }
                    lines.push(crate::report::Line {
                        cells: vec![
                            name.clone(),
                            (page + 1).to_string(),
                            row.subject.clone(),
                            markup.kind().name().to_string(),
                            row.caption.clone(),
                            row.author.clone(),
                        ],
                        heading: false,
                    });
                }
            }
        }

        let headings: Vec<String> = vec![
            "File".into(),
            "Sheet".into(),
            "Subject".into(),
            "Kind".into(),
            "Reads".into(),
            "Author".into(),
        ];

        let mut short_by = String::new();
        if unscaled > 0 {
            short_by = format!(
                "SHORT: {unscaled} measurement{} {} on {} sheet{} with no scale set. \
                 {} reported above and left out of any total — never counted as zero. \
                 Set the scale on those sheets and take this report again.",
                if unscaled == 1 { "" } else { "s" },
                if unscaled == 1 { "is" } else { "are" },
                unscaled_pages.len(),
                if unscaled_pages.len() == 1 { "" } else { "s" },
                if unscaled == 1 { "It is" } else { "They are" }
            );
        }
        if !unreadable.is_empty() {
            if !short_by.is_empty() {
                short_by.push(' ');
            }
            short_by.push_str(&format!(
                "{} file{} could not be read and {} left out: {}.",
                unreadable.len(),
                if unreadable.len() == 1 { "" } else { "s" },
                if unreadable.len() == 1 { "was" } else { "were" },
                unreadable.join(", ")
            ));
        }

        let widths = crate::report::widths_for(&headings, &lines);
        let report = crate::report::Report {
            title: "Takeoff summary".into(),
            drawing: format!(
                "{} set{}, {total_markups} measurement{}",
                sources.len(),
                if sources.len() == 1 { "" } else { "s" },
                if total_markups == 1 { "" } else { "s" }
            ),
            when: crate::stamps::today(),
            who: String::new(),
            short_by,
            headings,
            widths,
            pages: crate::report::paginate(lines),
        };

        self.write_report(&report, to)
    }

    // ---- repair, labels, unflattening, redaction and export ----------------

    /// A new drawing with nothing on it.
    fn new_blank(
        &mut self,
        to: &std::path::Path,
        sheets: usize,
        size: [f32; 2],
    ) -> Result<Done, String> {
        if sheets == 0 {
            return Err("A drawing with no sheets is not a drawing.".into());
        }
        if size[0] <= 1.0 || size[1] <= 1.0 {
            return Err("That is not a paper size.".into());
        }
        let doc = unsafe { self.bindings.FPDF_CreateNewDocument() };
        if doc.is_null() {
            return Err("a new drawing could not be started".into());
        }
        for at in 0..sheets {
            let page = unsafe {
                self.bindings.FPDFPage_New(
                    doc,
                    at as i32,
                    size[0] as f64,
                    size[1] as f64,
                )
            };
            if page.is_null() {
                unsafe { self.bindings.FPDF_CloseDocument(doc) };
                return Err(format!("sheet {} could not be made", at + 1));
            }
            unsafe { self.bindings.FPDFPage_GenerateContent(page) };
            unsafe { self.bindings.FPDF_ClosePage(page) };
        }
        let wrote = self.write_out(doc, to);
        unsafe { self.bindings.FPDF_CloseDocument(doc) };
        wrote?;
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said: format!(
                "{sheets} blank sheet{} written to {}, {:.0} by {:.0} inches.",
                if sheets == 1 { "" } else { "s" },
                nice(to),
                size[0] / 72.0,
                size[1] / 72.0
            ),
        })
    }

    /// A drawing made from pictures, one sheet each.
    ///
    /// The commonest way a set of scans arrives: a folder of TIFFs or JPEGs
    /// off somebody's scanner. Each sheet is the size the picture is at the
    /// resolution given, so a 300 dpi scan of a D-size sheet comes out D-size
    /// rather than as a postage stamp.
    fn from_pictures(
        &mut self,
        pictures: &[std::path::PathBuf],
        to: &std::path::Path,
        dpi: u32,
    ) -> Result<Done, String> {
        if pictures.is_empty() {
            return Err("No pictures were chosen.".into());
        }
        let doc = unsafe { self.bindings.FPDF_CreateNewDocument() };
        if doc.is_null() {
            return Err("a new drawing could not be started".into());
        }
        let per_inch = (dpi.max(1)) as f64;
        let mut made = 0usize;
        let mut refused: Vec<String> = Vec::new();
        for path in pictures {
            let image = match read_picture(path) {
                Ok(image) => image,
                Err(_) => {
                    refused.push(nice(path));
                    continue;
                }
            };
            let width = image.size[0] as f64 / per_inch * 72.0;
            let height = image.size[1] as f64 / per_inch * 72.0;
            let at = unsafe { self.bindings.FPDF_GetPageCount(doc) } as usize;
            if self.page_from_picture(doc, at, width, height, &image) {
                made += 1;
            } else {
                refused.push(nice(path));
            }
        }
        if made == 0 {
            unsafe { self.bindings.FPDF_CloseDocument(doc) };
            return Err(format!(
                "None of those could be read as a picture: {}.",
                refused.join(", ")
            ));
        }
        let wrote = self.write_out(doc, to);
        unsafe { self.bindings.FPDF_CloseDocument(doc) };
        wrote?;

        let mut said = format!(
            "{made} picture{} written into {} at {dpi} dpi, one sheet each.",
            if made == 1 { "" } else { "s" },
            nice(to)
        );
        if !refused.is_empty() {
            said.push_str(&format!(
                " {} could not be read and {} left out: {}.",
                refused.len(),
                if refused.len() == 1 { "was" } else { "were" },
                refused.join(", ")
            ));
        }
        said.push_str(
            " There is no text on these sheets to search yet — run OCR and there will be.",
        );
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said,
        })
    }

    /// Locks a set with a password.
    fn secure(
        &mut self,
        source: &std::path::Path,
        to: &std::path::Path,
        open_password: &str,
        owner_password: &str,
        allowed: pdf::crypt::Allowed,
    ) -> Result<Done, String> {
        let file = pdf::Document::open(source).map_err(|e| format!("{}: {e}", nice(source)))?;
        if file.encrypted {
            return Err(format!(
                "{} is already locked. Open it with its password, save a copy, and \
                 lock that.",
                nice(source)
            ));
        }
        let sheets_before = file.page_count();

        // The key and the salts have to be unguessable, and they have to come
        // from the machine rather than from anything in the file.
        let key = fresh_bytes::<32>();
        let salts = fresh_bytes::<32>();
        let id = fresh_bytes::<16>();
        let locked = pdf::crypt::lock(open_password, owner_password, allowed, key, salts);

        // Checked before it is written: a set locked with a password that turns
        // out not to work is a set nobody gets back.
        if pdf::crypt::unlock(&locked.encrypt, open_password) != Some(key) {
            return Err(
                "The password did not check out against the file this program was \
                 about to write, so nothing has been written. This is a fault in \
                 Excalibur View rather than anything you did."
                    .into(),
            );
        }

        let bytes = pdf::crypt::write_locked(&file, &locked, id);
        std::fs::write(to, &bytes).map_err(|e| format!("could not write {}: {e}", nice(to)))?;

        // And checked again through pdfium, which is a different program's
        // opinion of whether the file opens.
        let opened = self.open_with_password(to, open_password);
        match opened {
            Ok(count) if count == sheets_before => {}
            Ok(count) => {
                return Err(format!(
                    "{} was written but opens with {count} sheet{} instead of \
                     {sheets_before}. The original is untouched; do not send this one.",
                    nice(to),
                    if count == 1 { "" } else { "s" }
                ))
            }
            Err(why) => {
                return Err(format!(
                    "{} was written but will not open again: {why}. The original is \
                     untouched; do not send this one.",
                    nice(to)
                ))
            }
        }

        let mut said = if open_password.is_empty() {
            format!(
                "{} written and locked. Anybody may open it without a password, and \
                 a reader is asked to honour the permissions. ",
                nice(to)
            )
        } else {
            format!(
                "{} written and locked. It cannot be opened without the password. ",
                nice(to)
            )
        };
        said.push_str(&allowed.in_words());
        said.push_str(
            " Permissions are a request a reader honours, not a lock — the password \
             is the lock. Excalibur View keeps no copy of it, and nobody can recover it.",
        );
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said,
        })
    }

    /// Opens a file with a password, and says how many sheets came back.
    fn open_with_password(
        &mut self,
        path: &std::path::Path,
        password: &str,
    ) -> Result<usize, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", nice(path)))?;
        let doc = unsafe { self.bindings.FPDF_LoadMemDocument64(&bytes, Some(password)) };
        if doc.is_null() {
            let code = unsafe { self.bindings.FPDF_GetLastError() };
            return Err(match code {
                3 | 4 => "the password was not accepted".to_string(),
                other => format!("pdfium error {other}"),
            });
        }
        let count = unsafe { self.bindings.FPDF_GetPageCount(doc) }.max(0) as usize;
        unsafe { self.bindings.FPDF_CloseDocument(doc) };
        Ok(count)
    }

    /// Rebuilds a file whose cross-reference table no longer matches what is
    /// in it.
    ///
    /// The usual cause is a set that has been through a program that wrote the
    /// table wrong, or a file that was cut short in transit. Every object is
    /// found by reading the file itself, and a fresh table is written from
    /// what was actually there — so a drawing that opens with sheets missing
    /// comes back whole, or Hyperview says which objects it could not find
    /// rather than quietly writing a shorter set.
    fn repair(
        &mut self,
        source: &std::path::Path,
        to: &std::path::Path,
    ) -> Result<Done, String> {
        let bytes = std::fs::read(source).map_err(|e| format!("{}: {e}", nice(source)))?;
        if !bytes.starts_with(b"%PDF") {
            return Err(format!(
                "{} does not start like a PDF, so there is nothing to rebuild.",
                nice(source)
            ));
        }
        let before = pdf::Document::from_bytes(bytes.clone());
        let sheets_before = before.page_count();

        let rebuilt = pdf::repair::rebuild(&bytes);
        std::fs::write(to, &rebuilt.bytes)
            .map_err(|e| format!("could not write {}: {e}", nice(to)))?;

        // What actually came back, read through pdfium rather than our own
        // parser, because the question is whether every reader can open it.
        let after = match self.open_alone(to) {
            Ok(one) => {
                let count = one.count;
                self.close_alone(one);
                count
            }
            Err(why) => {
                return Err(format!(
                    "{} was rebuilt but still will not open: {why}",
                    nice(to)
                ))
            }
        };

        let mut said = format!(
            "{} rebuilt as {}: {} object{} found, {after} sheet{}.",
            nice(source),
            nice(to),
            rebuilt.found,
            if rebuilt.found == 1 { "" } else { "s" },
            if after == 1 { "" } else { "s" }
        );
        if after > sheets_before {
            said.push_str(&format!(
                " The original opened with {sheets_before}, so {} more {} come back.",
                after - sheets_before,
                if after - sheets_before == 1 { "has" } else { "have" }
            ));
        } else if after < sheets_before {
            said.push_str(&format!(
                " The original opened with {sheets_before}, so this is {} fewer — \
                 keep the original.",
                sheets_before - after
            ));
        } else {
            said.push_str(" The same sheets as before, with a table every reader can follow.");
        }
        if rebuilt.unreadable > 0 {
            said.push_str(&format!(
                " {} object{} could not be read and {} left out.",
                rebuilt.unreadable,
                if rebuilt.unreadable == 1 { "" } else { "s" },
                if rebuilt.unreadable == 1 { "was" } else { "were" }
            ));
        }
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said,
        })
    }

    /// Writes each sheet's own number into the file as its page label.
    ///
    /// Hyperview reads the number off the drawing and shows it in the sheet
    /// list without needing this. Writing it in is for everybody else: a
    /// general contractor opening the set in Acrobat sees S-201 in the page
    /// box instead of "7".
    fn write_page_labels(
        &mut self,
        source: &std::path::Path,
        labels: &[(u32, String)],
        to: &std::path::Path,
    ) -> Result<Done, String> {
        if labels.is_empty() {
            return Err(
                "No sheet numbers were read off this set, so there is nothing to \
                 write in. OCR a scanned set first and the numbers will be there."
                    .into(),
            );
        }
        std::fs::copy(source, to).map_err(|e| format!("could not write {}: {e}", nice(to)))?;
        let file = pdf::Document::open(to).map_err(|e| format!("{}: {e}", nice(to)))?;

        // A PDF's page labels are a number tree: a run starts at a sheet and
        // carries a prefix from there on. One run per sheet is the honest
        // shape when every sheet has a number of its own.
        let mut numbers: Vec<pdf::Object> = Vec::new();
        let mut sorted: Vec<(u32, String)> = labels.to_vec();
        sorted.sort_by_key(|(page, _)| *page);
        for (page, label) in &sorted {
            let mut entry = pdf::Dict::new();
            entry.set("P", pdf::Object::text(label));
            numbers.push(pdf::Object::Int(*page as i64));
            numbers.push(pdf::Object::Dict(entry));
        }

        let mut update = pdf::write::Update::new(&file);
        let mut tree = pdf::Dict::new();
        tree.set("Nums", pdf::Object::Array(numbers));
        let tree_ref = update.add(pdf::Object::Dict(tree));

        let catalog_ref = file
            .xref
            .trailer
            .get("Root")
            .and_then(|o| o.as_ref())
            .ok_or_else(|| format!("{} has no catalog to write labels into", nice(to)))?;
        let mut catalog = file.catalog();
        catalog.set("PageLabels", pdf::Object::Ref(tree_ref));
        update.replace(catalog_ref, pdf::Object::Dict(catalog));

        let written = update.apply(&file);
        std::fs::write(to, written).map_err(|e| format!("could not write {}: {e}", nice(to)))?;

        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said: format!(
                "{} sheet number{} written into {} as page labels. Every reader now \
                 shows them.",
                sorted.len(),
                if sorted.len() == 1 { "" } else { "s" },
                nice(to)
            ),
        })
    }

    /// Lifts markups Hyperview flattened back out into markups again.
    ///
    /// Flattening turns a markup into paint, and paint cannot be turned back
    /// into a markup by looking at it. So Hyperview keeps the markups it
    /// flattened, in the file it writes, under a key of its own — and this
    /// puts them back. Markups flattened by another program are not there to
    /// put back, and that is said rather than guessed at.
    fn unflatten(
        &mut self,
        source: &std::path::Path,
        to: &std::path::Path,
    ) -> Result<Done, String> {
        let file = pdf::Document::open(source).map_err(|e| format!("{}: {e}", nice(source)))?;
        let kept = crate::flatkeep::read(&file);
        if kept.is_empty() {
            return Err(format!(
                "{} carries no markups that Excalibur View flattened. Flattening turns a \
                 markup into paint, and paint cannot be read back as a markup — so \
                 a set flattened by another program has nothing to lift out.",
                nice(source)
            ));
        }

        std::fs::copy(source, to).map_err(|e| format!("could not write {}: {e}", nice(to)))?;
        let file = pdf::Document::open(to).map_err(|e| format!("{}: {e}", nice(to)))?;
        let mut placer = annot::place::Placer::new(&file);
        let mut put_back = 0usize;
        for (page, dict) in &kept {
            let mut markup = annot::Markup {
                dict: dict.clone(),
                picture: None,
            };
            if placer.add(*page as usize, &mut markup).is_some() {
                put_back += 1;
            }
        }
        let mut update = placer.finish();
        crate::flatkeep::clear(&file, &mut update);
        let written = update.apply(&file);
        std::fs::write(to, written).map_err(|e| format!("could not write {}: {e}", nice(to)))?;

        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said: format!(
                "{put_back} markup{} lifted back out into {}. They are markups again: \
                 in the list, in the totals, and editable. The paint they were \
                 flattened into is still on the page underneath, so this is a copy to \
                 read the numbers off rather than one to issue.",
                if put_back == 1 { "" } else { "s" },
                nice(to)
            ),
        })
    }

    /// Takes out what is under every redaction mark, for good.
    fn apply_redactions(
        &mut self,
        source: &std::path::Path,
        to: &std::path::Path,
        as_pictures: bool,
        dpi: u32,
    ) -> Result<Done, String> {
        // Where the marks are, read from the file itself rather than from the
        // window, so this works on a set somebody else marked up.
        let file = pdf::Document::open(source).map_err(|e| format!("{}: {e}", nice(source)))?;
        let mut boxes: Vec<(usize, [f64; 4])> = Vec::new();
        for page in 0..file.page_count() {
            for (_, markup) in annot::place::read_page(&file, page) {
                let redaction = markup.subtype() == annot::Subtype::Square
                    && (markup.subject().eq_ignore_ascii_case("redaction")
                        || markup
                            .dict
                            .get("IT")
                            .and_then(|o| o.as_name())
                            .map(|n| n.as_str() == "Redact")
                            .unwrap_or(false));
                if !redaction {
                    continue;
                }
                // The box as it was drawn, not the wider area its outline
                // is allowed to reach into.
                if let Some(area) = markup.drawn_box() {
                    boxes.push((page, area));
                }
            }
        }
        if boxes.is_empty() {
            return Err(format!(
                "There are no redaction marks on {}. Draw them with the Redaction \
                 tool first; nothing comes out of a drawing until you do.",
                nice(source)
            ));
        }

        if as_pictures {
            return self.redact_by_rendering(source, to, &boxes, dpi);
        }
        self.redact_by_removing(source, to, &boxes)
    }

    /// Redaction that keeps the sheet as a drawing: every object the box
    /// touches is taken out of the file, and a solid box is drawn over the
    /// area.
    fn redact_by_removing(
        &mut self,
        source: &std::path::Path,
        to: &std::path::Path,
        boxes: &[(usize, [f64; 4])],
    ) -> Result<Done, String> {
        let one = self.open_alone(source)?;
        let all: Vec<u32> = (0..one.count as u32).collect();
        let fresh = match self.assemble(&[(one.doc, all)]) {
            Ok(fresh) => fresh,
            Err(why) => {
                self.close_alone(one);
                return Err(why);
            }
        };

        let mut removed = 0usize;
        let mut spilled = 0usize;
        let mut touched: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        for (page, area) in boxes {
            let handle = unsafe { self.bindings.FPDF_LoadPage(fresh, *page as i32) };
            if handle.is_null() {
                continue;
            }
            touched.insert(*page);
            // Backwards, because removing an object shifts the ones after it.
            let count = unsafe { self.bindings.FPDFPage_CountObjects(handle) };
            for at in (0..count).rev() {
                let object = unsafe { self.bindings.FPDFPage_GetObject(handle, at) };
                if object.is_null() {
                    continue;
                }
                let (mut l, mut b, mut r, mut t) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
                let got = unsafe {
                    self.bindings
                        .FPDFPageObj_GetBounds(object, &mut l, &mut b, &mut r, &mut t)
                };
                if got == 0 {
                    continue;
                }
                let overlaps = (l as f64) < area[2]
                    && (r as f64) > area[0]
                    && (b as f64) < area[3]
                    && (t as f64) > area[1];
                if !overlaps {
                    continue;
                }
                // Anything the box only clips still has to go: half a word is
                // still the word. How much reached outside is reported, so
                // nobody is surprised by what went with it.
                let inside = (l as f64) >= area[0]
                    && (r as f64) <= area[2]
                    && (b as f64) >= area[1]
                    && (t as f64) <= area[3];
                if !inside {
                    spilled += 1;
                }
                if unsafe { self.bindings.FPDFPage_RemoveObject(handle, object) } != 0 {
                    unsafe { self.bindings.FPDFPageObj_Destroy(object) };
                    removed += 1;
                }
            }
            // And a solid box over the area, so the sheet shows that something
            // was taken out rather than simply having a hole in it.
            let cover = unsafe {
                self.bindings.FPDFPageObj_CreateNewRect(
                    area[0] as f32,
                    area[1] as f32,
                    (area[2] - area[0]) as f32,
                    (area[3] - area[1]) as f32,
                )
            };
            if !cover.is_null() {
                unsafe {
                    self.bindings.FPDFPageObj_SetFillColor(cover, 0, 0, 0, 255);
                    self.bindings.FPDFPath_SetDrawMode(cover, 2, 0);
                    self.bindings.FPDFPage_InsertObject(handle, cover);
                }
            }
            unsafe { self.bindings.FPDFPage_GenerateContent(handle) };
            unsafe { self.bindings.FPDF_ClosePage(handle) };
        }

        let wrote = self.write_out(fresh, to);
        unsafe { self.bindings.FPDF_CloseDocument(fresh) };
        self.close_alone(one);
        wrote?;

        let mut said = format!(
            "{removed} thing{} taken out from under {} redaction mark{} on {} sheet{}, \
             written to {}. They are gone from that file for good.",
            if removed == 1 { "" } else { "s" },
            boxes.len(),
            if boxes.len() == 1 { "" } else { "s" },
            touched.len(),
            if touched.len() == 1 { "" } else { "s" },
            nice(to)
        );
        if spilled > 0 {
            said.push_str(&format!(
                " {spilled} of them reached outside a box and went with it — check \
                 those sheets before the set goes out."
            ));
        }
        said.push_str(" The original still has everything.");
        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said,
        })
    }

    /// Redaction that is certain: the affected sheets become pictures with the
    /// redacted areas painted out, so nothing of what was underneath survives
    /// anywhere in the file.
    fn redact_by_rendering(
        &mut self,
        source: &std::path::Path,
        to: &std::path::Path,
        boxes: &[(usize, [f64; 4])],
        dpi: u32,
    ) -> Result<Done, String> {
        let one = self.open_alone(source)?;
        let count = one.count;
        let fresh = unsafe { self.bindings.FPDF_CreateNewDocument() };
        if fresh.is_null() {
            self.close_alone(one);
            return Err("a new drawing could not be started".into());
        }

        let affected: std::collections::BTreeSet<usize> =
            boxes.iter().map(|(page, _)| *page).collect();
        let scale = (dpi as f64 / 72.0).clamp(0.25, 8.0);
        let mut painted = 0usize;

        for page in 0..count {
            if !affected.contains(&page) {
                // Untouched sheets are copied across as they are, so only the
                // sheets that had to change do.
                let taken = unsafe {
                    self.bindings.FPDF_ImportPagesByIndex(
                        fresh,
                        one.doc,
                        [page as i32].as_ptr(),
                        1,
                        (page.min(usize::MAX)) as i32,
                    )
                };
                let _ = taken;
                continue;
            }
            let handle = unsafe { self.bindings.FPDF_LoadPage(one.doc, page as i32) };
            if handle.is_null() {
                continue;
            }
            let width = unsafe { self.bindings.FPDF_GetPageWidthF(handle) } as f64;
            let height = unsafe { self.bindings.FPDF_GetPageHeightF(handle) } as f64;
            let across = (width * scale).round().max(1.0) as u32;
            let down = (height * scale).round().max(1.0) as u32;
            let image = self.raster(handle, scale, 0.0, 0.0, across, down);
            unsafe { self.bindings.FPDF_ClosePage(handle) };
            let Some(mut image) = image else { continue };

            // Paint the boxes out, in the picture, before it ever becomes a
            // page. PDF y is up from the bottom; the picture's is down from
            // the top.
            for (at, area) in boxes.iter().filter(|(p, _)| *p == page) {
                let _ = at;
                let x0 = ((area[0] * scale).floor().max(0.0)) as usize;
                let x1 = ((area[2] * scale).ceil().min(across as f64)) as usize;
                let y0 = (((height - area[3]) * scale).floor().max(0.0)) as usize;
                let y1 = (((height - area[1]) * scale).ceil().min(down as f64)) as usize;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let at = y * across as usize + x;
                        if let Some(pixel) = image.pixels.get_mut(at) {
                            *pixel = egui::Color32::BLACK;
                        }
                    }
                }
                painted += 1;
            }

            let at = unsafe { self.bindings.FPDF_GetPageCount(fresh) } as usize;
            if !self.page_from_picture(fresh, at, width, height, &image) {
                continue;
            }
        }

        let wrote = self.write_out(fresh, to);
        unsafe { self.bindings.FPDF_CloseDocument(fresh) };
        self.close_alone(one);
        wrote?;

        Ok(Done {
            wrote: vec![to.to_path_buf()],
            said: format!(
                "{painted} redaction mark{} applied on {} sheet{}, written to {}. \
                 Those sheets are pictures now: nothing of what was under the boxes \
                 is left anywhere in the file, and those sheets can no longer be \
                 searched. Every other sheet came across untouched, and the original \
                 still has everything.",
                if painted == 1 { "" } else { "s" },
                affected.len(),
                if affected.len() == 1 { "" } else { "s" },
                nice(to)
            ),
        })
    }

    /// Writes chosen sheets out as picture files.
    fn export_pictures(
        &mut self,
        source: &std::path::Path,
        pages: &[u32],
        into: &std::path::Path,
        dpi: u32,
        format: &str,
    ) -> Result<Done, String> {
        if pages.is_empty() {
            return Err("No sheets were chosen.".into());
        }
        std::fs::create_dir_all(into)
            .map_err(|e| format!("could not use {}: {e}", nice(into)))?;
        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "drawing".into());
        let extension = if format.eq_ignore_ascii_case("jpg")
            || format.eq_ignore_ascii_case("jpeg")
        {
            "jpg"
        } else {
            "png"
        };

        let one = self.open_alone(source)?;
        let count = one.count;
        let scale = (dpi as f64 / 72.0).clamp(0.25, 8.0);
        let mut wrote = Vec::new();
        let mut refused: Vec<u32> = Vec::new();
        for page in pages {
            if (*page as usize) >= count {
                refused.push(page + 1);
                continue;
            }
            let handle = unsafe { self.bindings.FPDF_LoadPage(one.doc, *page as i32) };
            if handle.is_null() {
                refused.push(page + 1);
                continue;
            }
            let width = unsafe { self.bindings.FPDF_GetPageWidthF(handle) } as f64;
            let height = unsafe { self.bindings.FPDF_GetPageHeightF(handle) } as f64;
            let across = (width * scale).round().max(1.0) as u32;
            let down = (height * scale).round().max(1.0) as u32;
            let image = self.raster(handle, scale, 0.0, 0.0, across, down);
            unsafe { self.bindings.FPDF_ClosePage(handle) };
            let Some(image) = image else {
                refused.push(page + 1);
                continue;
            };
            let named = crate::docops::free_name(
                &into.join(format!("{stem} {:03}.{extension}", page + 1)),
            );
            match write_png(&image, &named) {
                Ok(()) => wrote.push(named),
                Err(_) => refused.push(page + 1),
            }
        }
        self.close_alone(one);

        let mut said = format!(
            "{} sheet{} written into {} at {dpi} dpi.",
            wrote.len(),
            if wrote.len() == 1 { "" } else { "s" },
            nice(into)
        );
        if !refused.is_empty() {
            said.push_str(&format!(
                " {} could not be written: {}.",
                refused.len(),
                refused
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(Done { wrote, said })
    }

    // ---- comparing two issues of a sheet ----------------------------------

    /// Draws one sheet at the size asked for, fitted inside it.
    fn picture(
        &mut self,
        path: &std::path::Path,
        index: u32,
        across: u32,
        down: u32,
    ) -> Result<egui::ColorImage, String> {
        let one = self.open_alone(path)?;
        if index as usize >= one.count {
            let count = one.count;
            self.close_alone(one);
            return Err(format!(
                "{} has {count} sheet{}, so there is no sheet {}.",
                nice(path),
                if count == 1 { "" } else { "s" },
                index + 1
            ));
        }
        let page = unsafe { self.bindings.FPDF_LoadPage(one.doc, index as i32) };
        if page.is_null() {
            self.close_alone(one);
            return Err(format!("sheet {} of {} would not open", index + 1, nice(path)));
        }
        let width = unsafe { self.bindings.FPDF_GetPageWidthF(page) } as f64;
        let height = unsafe { self.bindings.FPDF_GetPageHeightF(page) } as f64;
        let scale = (across as f64 / width.max(1.0)).min(down as f64 / height.max(1.0));
        let w = (width * scale).round().max(1.0) as u32;
        let h = (height * scale).round().max(1.0) as u32;
        let image = self.raster(page, scale, 0.0, 0.0, w, h);
        unsafe { self.bindings.FPDF_ClosePage(page) };
        self.close_alone(one);
        image.ok_or_else(|| format!("sheet {} would not render", index + 1))
    }

    /// One sheet's size as shown, in points.
    fn sheet_size(&mut self, path: &std::path::Path, index: u32) -> Result<[f64; 2], String> { // WHAT: a sheet's size in points. WHY: clicks become points.
        let one = self.open_alone(path)?; // WHAT: the file. WHY: its sheet is measured.
        let page = if (index as usize) < one.count { unsafe { self.bindings.FPDF_LoadPage(one.doc, index as i32) } } else { std::ptr::null_mut() }; // WHAT: the sheet, if there is one. WHY: a missing sheet is said below.
        let size = if page.is_null() { None } else { Some([f64::from(unsafe { self.bindings.FPDF_GetPageWidthF(page) }), f64::from(unsafe { self.bindings.FPDF_GetPageHeightF(page) })]) }; // WHAT: its width and height. WHY: a click on its picture becomes a point on it.
        if !page.is_null() { // WHAT: the sheet was loaded. WHY: closed.
            unsafe { self.bindings.FPDF_ClosePage(page) }; // WHAT: closes it. WHY: done.
        }
        self.close_alone(one); // WHAT: closes the file. WHY: done.
        size.ok_or_else(|| format!("there is no sheet {} in {}", index + 1, nice(path))) // WHAT: the size, or why not. WHY: said plainly.
    }

    /// Every word on one sheet.
    fn words_on(&mut self, path: &std::path::Path, index: u32) -> Result<String, String> {
        let one = self.open_alone(path)?;
        if index as usize >= one.count {
            self.close_alone(one);
            return Err(format!("there is no sheet {} in {}", index + 1, nice(path)));
        }
        let page = unsafe { self.bindings.FPDF_LoadPage(one.doc, index as i32) };
        if page.is_null() {
            self.close_alone(one);
            return Err(format!("sheet {} of {} would not open", index + 1, nice(path)));
        }
        let text = unsafe { self.bindings.FPDFText_LoadPage(page) };
        let mut out = String::new();
        if !text.is_null() {
            let count = unsafe { self.bindings.FPDFText_CountChars(text) };
            if count > 0 {
                // Two bytes a character, plus the terminator pdfium adds.
                let mut buffer = vec![0u16; count as usize + 1];
                let got = unsafe {
                    self.bindings.FPDFText_GetText(
                        text,
                        0,
                        count,
                        buffer.as_mut_ptr() as *mut _,
                    )
                };
                if got > 0 {
                    buffer.truncate((got as usize).saturating_sub(1));
                    out = String::from_utf16_lossy(&buffer);
                }
            }
            unsafe { self.bindings.FPDFText_ClosePage(text) };
        }
        unsafe { self.bindings.FPDF_ClosePage(page) };
        self.close_alone(one);
        Ok(out)
    }

    /// The grid bubbles on one sheet, read off its own text.
    ///
    /// Every candidate goes to [`takeoff::grid::read`], which is where the
    /// deciding happens — this only hands over what is written on the sheet
    /// and where. A sheet with no grid comes back with no grid.
    fn grid_on(
        &mut self,
        path: &std::path::Path,
        index: u32,
        how: takeoff::grid::HowClose,
    ) -> Result<takeoff::grid::Grid, String> {
        let one = self.open_alone(path)?;
        if index as usize >= one.count {
            self.close_alone(one);
            return Err(format!("there is no sheet {} in {}", index + 1, nice(path)));
        }
        let mut size = FS_SIZEF {
            width: 612.0,
            height: 792.0,
        };
        unsafe {
            self.bindings
                .FPDF_GetPageSizeByIndexF(one.doc, index as i32, &mut size)
        };
        let page = unsafe { self.bindings.FPDF_LoadPage(one.doc, index as i32) };
        if page.is_null() {
            self.close_alone(one);
            return Err(format!("sheet {} of {} would not open", index + 1, nice(path)));
        }
        // Sheets are often laid out around the origin rather than up from
        // (0,0), so positions have to be measured from the page's own box.
        let mut area = FS_RECTF {
            left: 0.0,
            top: size.height,
            right: size.width,
            bottom: 0.0,
        };
        unsafe { self.bindings.FPDF_GetPageBoundingBox(page, &mut area) };
        let text = unsafe { self.bindings.FPDFText_LoadPage(page) };
        let mut marks: Vec<takeoff::grid::Mark> = Vec::new();
        if !text.is_null() {
            for run in self.runs(text, area.left, area.top.max(area.bottom)) {
                let label = run.text.trim().to_string();
                if label.is_empty() {
                    continue;
                }
                marks.push(takeoff::grid::Mark {
                    label,
                    at: [
                        ((run.area.x0 + run.area.x1) * 0.5) as f64,
                        ((run.area.y0 + run.area.y1) * 0.5) as f64,
                    ],
                });
            }
            unsafe { self.bindings.FPDFText_ClosePage(text) };
        }
        unsafe { self.bindings.FPDF_ClosePage(page) };
        self.close_alone(one);
        Ok(takeoff::grid::read(
            &marks,
            [size.width as f64, size.height as f64],
            how,
        ))
    }

    /// The form environment for a document, made the first time it is asked
    /// for and kept until the document is closed.
    fn form_for(&mut self, doc: FPDF_DOCUMENT) -> Option<FPDF_FORMHANDLE> {
        if doc.is_null() {
            return None;
        }
        if let Some((_, handle, _)) = self.forms.iter().find(|(d, _, _)| *d == doc) {
            return Some(*handle);
        }
        // Pdfium wants a filled-in structure describing what the host can do,
        // and keeps the pointer. Hyperview draws forms rather than interacting
        // with them, so the version number is the only part that matters — but
        // the structure itself has to outlive the call, which is why it is
        // boxed and kept.
        let mut info: Box<FPDF_FORMFILLINFO> = Box::new(unsafe { std::mem::zeroed() });
        info.version = 2;
        let handle = unsafe {
            self.bindings
                .FPDFDOC_InitFormFillEnvironment(doc, info.as_mut() as *mut _)
        };
        if handle.is_null() {
            return None;
        }
        unsafe { self.bindings.FPDF_SetFormFieldHighlightAlpha(handle, 0) };
        self.forms.push((doc, handle, info));
        Some(handle)
    }

    /// Lets go of a document's form environment, before the document itself
    /// goes. The other way round and pdfium is left holding a dead pointer.
    fn drop_form(&mut self, doc: FPDF_DOCUMENT) {
        if let Some(at) = self.forms.iter().position(|(d, _, _)| *d == doc) {
            let (_, handle, info) = self.forms.remove(at);
            unsafe { self.bindings.FPDFDOC_ExitFormFillEnvironment(handle) };
            // Only now is pdfium finished with it.
            drop(info);
        }
        self.drawing_form = None;
    }

    /// Draws part of one sheet at the resolution asked for.
    ///
    /// The area comes in as sheet space — top left origin, y down, already
    /// turned — because that is what somebody dragged a box out in.
    fn region(
        &mut self,
        path: &std::path::Path,
        index: u32,
        area: [f64; 4],
        dpi: u32,
    ) -> Result<egui::ColorImage, String> {
        let one = self.open_alone(path)?;
        if index as usize >= one.count {
            let count = one.count;
            self.close_alone(one);
            return Err(format!(
                "{} has {count} sheet{}, so there is no sheet {}.",
                nice(path),
                if count == 1 { "" } else { "s" },
                index + 1
            ));
        }
        let page = unsafe { self.bindings.FPDF_LoadPage(one.doc, index as i32) };
        if page.is_null() {
            self.close_alone(one);
            return Err(format!("sheet {} of {} would not open", index + 1, nice(path)));
        }
        // A PDF point is a seventy-second of an inch, so this is the whole of
        // the conversion between what somebody asks for and what gets drawn.
        let scale = (dpi as f64 / 72.0).clamp(0.25, 16.0);
        let across = ((area[2] - area[0]) * scale).round();
        let down = ((area[3] - area[1]) * scale).round();
        if across < 1.0 || down < 1.0 {
            unsafe { self.bindings.FPDF_ClosePage(page) };
            self.close_alone(one);
            return Err("that box is too small to take a picture of".into());
        }
        // Guard against a box dragged across a whole D-size sheet at 600 dpi,
        // which would be a bitmap nothing can hold.
        const MOST_PIXELS: f64 = 64.0 * 1024.0 * 1024.0;
        let (across, down) = if across * down > MOST_PIXELS {
            let shrink = (MOST_PIXELS / (across * down)).sqrt();
            ((across * shrink).max(1.0), (down * shrink).max(1.0))
        } else {
            (across, down)
        };
        let image = self.raster(
            page,
            scale,
            area[0] * scale,
            area[1] * scale,
            across as u32,
            down as u32,
        );
        unsafe { self.bindings.FPDF_ClosePage(page) };
        self.close_alone(one);
        image.ok_or_else(|| "that part of the sheet would not render".to_string())
    }

    /// Compares two issues of a sheet and gives the differences back in the
    /// newer sheet's own coordinates, so they can be drawn on it.
    ///
    /// Lined up the way Overlay Pages lines them up — shift, quarter turns,
    /// skew and scale — and then compared on the newer sheet's own pixels, so
    /// a half-size print or a crooked scan is compared as the drawing it is.
    fn compare_sheets(&mut self, older: &std::path::Path, older_page: u32, newer: &std::path::Path, newer_page: u32, darkness: f32) -> Result<Compared, String> { // WHAT: two sheets compared, lined up first. WHY: Compare Documents.
        let (a, b) = self.open_pair(older, older_page, newer, newer_page)?; // WHAT: both files. WHY: open while they are fitted.
        let fitted = self.fit_sheets(a.doc, older_page, b.doc, newer_page, darkness, &crate::overlay::Lining::Auto, [&nice(older), &nice(newer)]); // WHAT: how the older sits on the newer. WHY: compared through it.
        self.close_alone(a); // WHAT: closes the older. WHY: done with the file.
        self.close_alone(b); // WHAT: closes the newer. WHY: the same.
        let fitted = fitted?; // WHAT: the fit, or why not. WHY: said plainly.
        let (w, h) = (fitted.newer_ink.width, fitted.newer_ink.height); // WHAT: the newer sheet's working pixels. WHY: both are compared on them.
        let moved = crate::overlay::warp(&fitted.older_ink, &fitted.fit_px, w, h); // WHAT: the older ink laid on the newer's pixels. WHY: compared pixel for pixel.
        let found = crate::compare::compare_fitted(&moved, &fitted.newer_ink, fitted.agreement, 1); // WHAT: what changed. WHY: the comparison.
        let k = fitted.newer_k; // WHAT: working pixels per point on the newer sheet. WHY: changes go back to points.
        let changes = found.changes.iter().map(|c| (c.kind, [c.area[0] as f64 / k, c.area[1] as f64 / k, c.area[2] as f64 / k, c.area[3] as f64 / k])).collect(); // WHAT: each change in the newer sheet's points. WHY: that is the sheet the clouds go on.
        Ok(Compared { said: found.says(), could_not_align: found.could_not_align, agreement: found.alignment.agreement, changes }) // WHAT: what was found. WHY: shown and clouded.
    }

    /// Draws one sheet's drawing alone — no markups, no form fields — through
    /// a transform, into a grey picture of the size asked for.
    ///
    /// Without markups because an overlay and a comparison are about the
    /// drawing: an estimator's takeoff on the newer sheet is not a revision,
    /// and a comparison's own clouds must not show up as changes the next
    /// time it is run. Brightness only, one byte a pixel, because colour is
    /// never what decides ink from paper and a big sheet is four times
    /// smaller this way.
    fn drawing_alone(&mut self, page: FPDF_PAGE, m: [f64; 6], width: u32, height: u32) -> Option<Vec<u8>> { // WHAT: a sheet's drawing alone, as brightness. WHY: overlay and comparison.
        if width == 0 || height == 0 { // WHAT: no size. WHY: nothing to draw.
            return None; // WHAT: nothing to draw into. WHY: pdfium refuses an empty bitmap.
        }
        let mut buf = vec![0u8; width as usize * height as usize * 4]; // WHAT: the picture's memory. WHY: pdfium draws into it.
        let bitmap = unsafe { self.bindings.FPDFBitmap_CreateEx(width as i32, height as i32, FPDFBITMAP_BGRA as i32, buf.as_mut_ptr() as *mut c_void, (width * 4) as i32) }; // WHAT: a bitmap over that memory. WHY: what pdfium renders to.
        if bitmap.is_null() { // WHAT: no bitmap. WHY: nothing drawn.
            return None; // WHAT: no bitmap. WHY: out of memory, most likely.
        }
        unsafe { self.bindings.FPDFBitmap_FillRect(bitmap, 0, 0, width as i32, height as i32, 0xFFFF_FFFFu32 as _) }; // WHAT: white first. WHY: paper.
        let matrix = FS_MATRIX { a: m[0] as f32, b: m[1] as f32, c: m[2] as f32, d: m[3] as f32, e: m[4] as f32, f: m[5] as f32 }; // WHAT: the transform. WHY: turned, scaled and moved onto the other sheet.
        let clip = FS_RECTF { left: 0.0, top: 0.0, right: width as f32, bottom: height as f32 }; // WHAT: the whole picture. WHY: nothing outside it is drawn.
        unsafe { self.bindings.FPDF_RenderPageBitmapWithMatrix(bitmap, page, &matrix, &clip, 0) }; // WHAT: the drawing alone, no annotations. WHY: markups are not the drawing.
        unsafe { self.bindings.FPDFBitmap_Destroy(bitmap) }; // WHAT: lets the bitmap go. WHY: the memory is ours and is read below.
        Some(buf.chunks_exact(4).map(|p| ((u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3) as u8).collect()) // WHAT: each pixel's brightness. WHY: the same plain average every ink test in the program uses.
    }

    /// One sheet looked at for lining up: its drawing, at about `most` pixels
    /// on its long side and in its own proportions, and its size.
    fn sheet_look(&mut self, doc: FPDF_DOCUMENT, index: u32, most: f64, name: &str) -> Result<SheetLook, String> { // WHAT: a sheet looked at for fitting. WHY: its drawing and size.
        let page = unsafe { self.bindings.FPDF_LoadPage(doc, index as i32) }; // WHAT: the sheet. WHY: drawn below.
        if page.is_null() { // WHAT: it would not load. WHY: say which.
            return Err(format!("sheet {} of {name} would not open", index + 1)); // WHAT: says which. WHY: nobody guesses.
        }
        let width = f64::from(unsafe { self.bindings.FPDF_GetPageWidthF(page) }); // WHAT: its width as shown, in points. WHY: turned and cropped as the viewer shows it.
        let height = f64::from(unsafe { self.bindings.FPDF_GetPageHeightF(page) }); // WHAT: its height. WHY: the same.
        let k = most / width.max(height).max(1.0); // WHAT: pixels per point. WHY: the long side is `most` pixels.
        let (w, h) = ((width * k).round().max(1.0) as u32, (height * k).round().max(1.0) as u32); // WHAT: the picture's size. WHY: the sheet's own proportions, never stretched.
        let own = self.sheet_transform(page, PageSize { width: width as f32, height: height as f32 }); // WHAT: where the file's own coordinates put things on the sheet. WHY: the likeliest fit between two plots.
        let luma = self.drawing_alone(page, [k, 0.0, 0.0, k, 0.0, 0.0], w, h); // WHAT: the drawing, scaled. WHY: looked at.
        unsafe { self.bindings.FPDF_ClosePage(page) }; // WHAT: closes the sheet. WHY: done with it.
        let luma = luma.ok_or_else(|| format!("sheet {} of {name} would not draw", index + 1))?; // WHAT: the picture, or why not. WHY: said plainly.
        Ok(SheetLook { luma, width: w as usize, height: h as usize, k, size: [width, height], own }) // WHAT: the look. WHY: fitted next.
    }

    /// How the older sheet is laid on the newer, found or chosen.
    #[allow(clippy::too_many_arguments)] // WHAT: allows the long list. WHY: two sheets, two pages, the darkness and how to line them up are all needed.
    fn fit_sheets(&mut self, older: FPDF_DOCUMENT, older_page: u32, newer: FPDF_DOCUMENT, newer_page: u32, darkness: f32, lining: &crate::overlay::Lining, names: [&str; 2]) -> Result<Fitted, String> { // WHAT: how the older lies on the newer. WHY: overlay and comparison.
        use crate::overlay::{self, Fit, Lining}; // WHAT: the fitting. WHY: short names here.
        const WORK: f64 = 1800.0; // WHAT: the working size, long side, in pixels. WHY: a bolt symbol is a few pixels across, and hundreds of fits are tried in a moment.
        let a = self.sheet_look(older, older_page, WORK, names[0])?; // WHAT: the older sheet. WHY: fitted onto the newer.
        let b = self.sheet_look(newer, newer_page, WORK, names[1])?; // WHAT: the newer sheet. WHY: what the older is laid on.
        let ink_a = overlay::ink_of(&a.luma, a.width, a.height, darkness); // WHAT: the older's line-work. WHY: fits are found on ink.
        let ink_b = overlay::ink_of(&b.luma, b.width, b.height, darkness); // WHAT: the newer's. WHY: the same.
        // The files' own coordinates: the newer's sheet transform after the
        // older's undone. Two issues plotted from one drawing have it there.
        let own = match (a.own, b.own) { // WHAT: the files' own fit, if both have one. WHY: the first start.
            (Some(ta), Some(tb)) => own_fit(ta, tb).map(|f| f.rescaled(a.k, b.k)), // WHAT: that fit, in working pixels. WHY: the first start tried.
            _ => None, // WHAT: unknown. WHY: then the other starts do.
        };
        let same_points = b.k / a.k; // WHAT: the scale that keeps a point a point. WHY: bigger paper, same drawing.
        let as_they_are = own.unwrap_or(Fit { scale: same_points, ..Fit::SAME }); // WHAT: the sheets as they are. WHY: shown when nothing lines them up.
        let (fit_px, agreement, how) = match lining { // WHAT: the fit by the way chosen. WHY: found, picked or as they are.
            Lining::Auto => { // WHAT: by the drawing. WHY: searched.
                let found = overlay::find(&ink_a, &ink_b, &overlay::starts([a.width as f64, a.height as f64], [b.width as f64, b.height as f64], same_points, own)); // WHAT: the best fit. WHY: lined up by the line-work.
                if found.agreement >= crate::compare::ENOUGH_AGREEMENT { // WHAT: enough agrees. WHY: lined up.
                    (found.fit, found.agreement, Lined::Found) // WHAT: lined up. WHY: enough of it agrees.
                } else {
                    (as_they_are, found.agreement, Lined::NotFound) // WHAT: left as they are. WHY: a fit that does not agree would only mislead.
                }
            }
            Lining::Points { older: po, newer: pn } => { // WHAT: by two points. WHY: settled from them.
                let picked = Fit::from_points(*po, *pn).ok_or("The two points on a sheet have to be in different places.")?; // WHAT: the fit through the points, in sheet points. WHY: somebody chose it.
                let found = overlay::settle(&ink_a, &ink_b, picked.rescaled(a.k, b.k)); // WHAT: settled onto the line-work. WHY: a click is never exact to the pixel.
                (found.fit, found.agreement, Lined::Points) // WHAT: lined up by hand. WHY: shown whatever it agrees.
            }
            Lining::AsTheyAre => (as_they_are, overlay::agreement(&ink_a, &ink_b, &as_they_are), Lined::AsTheyAre), // WHAT: uncorrected. WHY: asked for.
        };
        Ok(Fitted { fit: fit_px.rescaled(1.0 / a.k, 1.0 / b.k), fit_px, agreement, how, older_size: a.size, newer_size: b.size, newer_k: b.k, older_ink: ink_a, newer_ink: ink_b }) // WHAT: the fit in points and in pixels, and both inks. WHY: the overlay draws through the first; a comparison uses the rest.
    }

    /// Lays two issues of a sheet on top of each other in different colours.
    ///
    /// The other half of a revision review, and the one people find easier to
    /// read: rather than a list of regions, the two drawings are printed over
    /// each other, one in each colour, and anything that did not move comes out
    /// dark where both agree. Where only one sheet has ink, its colour shows —
    /// so what was taken out and what was put in are the two colours, and
    /// everything unchanged is the third.
    ///
    /// The older sheet is drawn by pdfium straight through the fit, as
    /// vectors, at the newer sheet's own size and proportions — so the
    /// overlay is the newer sheet's paper, and a line on it is where the line
    /// is on the newer sheet.
    #[allow(clippy::too_many_arguments)] // WHAT: allows the long list. WHY: the same as fit_sheets.
    fn overlay_open(&mut self, older: FPDF_DOCUMENT, older_page: u32, newer: FPDF_DOCUMENT, newer_page: u32, darkness: f32, lining: &crate::overlay::Lining, names: [&str; 2]) -> Result<Overlaid, String> { // WHAT: two open sheets laid over each other. WHY: a sheet or a set at a time.
        let fitted = self.fit_sheets(older, older_page, newer, newer_page, darkness, lining, names)?; // WHAT: how the older sits on the newer. WHY: drawn through it.
        let [width, height] = fitted.newer_size; // WHAT: the newer sheet in points. WHY: the overlay is its paper.
        let ks = (OVERLAY_DPI / 72.0).min((OVERLAY_PIXELS / (width * height).max(1.0)).sqrt()); // WHAT: pixels per point for the overlay. WHY: 150 a inch, less on a sheet so big it would not fit in memory.
        let (w, h) = ((width * ks).round().max(1.0) as u32, (height * ks).round().max(1.0) as u32); // WHAT: the overlay's size. WHY: the newer sheet's proportions exactly.
        let page = unsafe { self.bindings.FPDF_LoadPage(newer, newer_page as i32) }; // WHAT: the newer sheet. WHY: drawn as it is.
        if page.is_null() { // WHAT: the newer would not load. WHY: say which.
            return Err(format!("sheet {} of {} would not open", newer_page + 1, names[1])); // WHAT: says which. WHY: plainly.
        }
        let b = self.drawing_alone(page, [ks, 0.0, 0.0, ks, 0.0, 0.0], w, h); // WHAT: the newer drawing at overlay size. WHY: one of the two layers.
        unsafe { self.bindings.FPDF_ClosePage(page) }; // WHAT: closes it. WHY: done.
        let page = unsafe { self.bindings.FPDF_LoadPage(older, older_page as i32) }; // WHAT: the older sheet. WHY: drawn through the fit.
        if page.is_null() { // WHAT: the older would not load. WHY: say which.
            return Err(format!("sheet {} of {} would not open", older_page + 1, names[0])); // WHAT: says which. WHY: plainly.
        }
        let f = fitted.fit; // WHAT: the fit in sheet points. WHY: turned into pdfium's transform.
        let (s, c) = f.turn.sin_cos(); // WHAT: the turn. WHY: the transform's first four numbers.
        let m = [ks * f.scale * c, ks * f.scale * s, -ks * f.scale * s, ks * f.scale * c, ks * f.dx, ks * f.dy]; // WHAT: older sheet points to overlay pixels. WHY: the fit, then the overlay's scale.
        let a = self.drawing_alone(page, m, w, h); // WHAT: the older drawing, laid on the newer paper. WHY: the other layer, still vectors when drawn.
        unsafe { self.bindings.FPDF_ClosePage(page) }; // WHAT: closes it. WHY: done.
        let (a, b) = (a.ok_or("the older sheet would not draw")?, b.ok_or("the newer sheet would not draw")?); // WHAT: both layers, or why not. WHY: plainly.
        let tolerance = (0.75 * ks / fitted.newer_k).ceil().clamp(1.0, 4.0) as usize; // WHAT: how near is the same line, in overlay pixels. WHY: the fit is good to under a working pixel, which is a few overlay pixels.
        let pixels = crate::overlay::colour(&a, &b, w as usize, h as usize, darkness, tolerance); // WHAT: the coloured overlay. WHY: what people look at.
        Ok(Overlaid { image: colour_image(&pixels, w as usize, h as usize), agreement: fitted.agreement, lined_up: fitted.how != Lined::NotFound, size: [width, height], said: fitted.says() }) // WHAT: the picture and what to say about it. WHY: shown, saved or printed.
    }

    /// The same, opening the two files itself.
    #[allow(clippy::too_many_arguments)] // WHAT: allows the long list. WHY: the same as fit_sheets.
    fn overlay_sheets(&mut self, older: &std::path::Path, older_page: u32, newer: &std::path::Path, newer_page: u32, darkness: f32, lining: &crate::overlay::Lining) -> Result<Overlaid, String> { // WHAT: two files' sheets laid over each other. WHY: the window's overlay.
        let (a, b) = self.open_pair(older, older_page, newer, newer_page)?; // WHAT: both files. WHY: open while the overlay is drawn.
        let done = self.overlay_open(a.doc, older_page, b.doc, newer_page, darkness, lining, [&nice(older), &nice(newer)]); // WHAT: lays them over. WHY: the work.
        self.close_alone(a); // WHAT: closes the older. WHY: done.
        self.close_alone(b); // WHAT: closes the newer. WHY: done.
        done // WHAT: the overlay, or why not. WHY: handed back.
    }

    /// Opens two files and checks each has the sheet asked for.
    fn open_pair(&mut self, older: &std::path::Path, older_page: u32, newer: &std::path::Path, newer_page: u32) -> Result<(Aside, Aside), String> { // WHAT: opens two files, checking their sheets. WHY: overlay and comparison.
        let a = self.open_alone(older)?; // WHAT: the older file. WHY: read.
        let b = match self.open_alone(newer) { // WHAT: the newer file. WHY: open alongside.
            Ok(b) => b, // WHAT: the newer file. WHY: read.
            Err(why) => { // WHAT: the newer would not open. WHY: close the older and say why.
                self.close_alone(a); // WHAT: closes the older. WHY: nothing left open on the way out.
                return Err(why); // WHAT: why the newer would not open. WHY: plainly.
            }
        };
        let missing = [(a.count, older_page, older), (b.count, newer_page, newer)].into_iter().find(|(count, page, _)| *page as usize >= *count); // WHAT: a sheet asked for that is not there. WHY: the commonest mistake.
        if let Some((count, page, path)) = missing { // WHAT: a sheet asked for is missing. WHY: close both and say which.
            let why = format!("{} has {count} sheet{}, so there is no sheet {}.", nice(path), if count == 1 { "" } else { "s" }, page + 1); // WHAT: which sheet is missing. WHY: said plainly.
            self.close_alone(a); // WHAT: closes both. WHY: nothing left open.
            self.close_alone(b); // WHAT: the same. WHY: the same.
            return Err(why); // WHAT: the reason. WHY: shown.
        }
        Ok((a, b)) // WHAT: both open. WHY: used by the caller, who closes them.
    }

    /// Writes one overlaid sheet as a page of `into`, with a line saying
    /// which colour is which.
    ///
    /// The line goes on the page itself, because an overlay printed and
    /// handed across a desk has no dialog beside it to say what red means.
    fn overlay_page(&mut self, into: FPDF_DOCUMENT, at: usize, laid: &Overlaid, legend: &str) -> bool { // WHAT: an overlay as a page with its legend. WHY: a file to print or send.
        if !self.page_from_picture(into, at, laid.size[0], laid.size[1], &laid.image) { // WHAT: the picture would not go in. WHY: reported.
            return false; // WHAT: the picture did not go in. WHY: the caller says which sheet.
        }
        let page = unsafe { self.bindings.FPDF_LoadPage(into, at as i32) }; // WHAT: the page just made. WHY: the legend goes on it.
        if page.is_null() { // WHAT: the page would not reopen. WHY: kept without a legend.
            return true; // WHAT: the picture is there, just no legend. WHY: better than losing the sheet.
        }
        let font = unsafe { self.bindings.FPDFText_LoadStandardFont(into, "Helvetica") }; // WHAT: a plain font. WHY: every reader has it.
        let size = (laid.size[0].min(laid.size[1]) / 90.0).clamp(6.0, 14.0) as f32; // WHAT: the legend's size. WHY: readable on a letter sheet and an ARCH E alike.
        self.write_text(into, page, font, legend, size * 1.5, size * 1.5, size, [0x33, 0x33, 0x33]); // WHAT: the legend, bottom left. WHY: where a print's notes go.
        unsafe { self.bindings.FPDFPage_GenerateContent(page) }; // WHAT: writes it into the page. WHY: pdfium keeps edits in memory until asked.
        unsafe { self.bindings.FPDF_ClosePage(page) }; // WHAT: closes it. WHY: done.
        true // WHAT: written. WHY: reported.
    }

    /// One sheet with nothing to lay it over, drawn all in the newer colour.
    fn newer_alone(&mut self, newer: FPDF_DOCUMENT, page_index: u32, darkness: f32) -> Option<Overlaid> { // WHAT: a sheet with nothing under it, all blue. WHY: it keeps its place.
        let page = unsafe { self.bindings.FPDF_LoadPage(newer, page_index as i32) }; // WHAT: the sheet. WHY: drawn alone.
        if page.is_null() { // WHAT: it would not load. WHY: reported.
            return None; // WHAT: it would not open. WHY: the caller says so.
        }
        let width = f64::from(unsafe { self.bindings.FPDF_GetPageWidthF(page) }); // WHAT: its width. WHY: the page's paper.
        let height = f64::from(unsafe { self.bindings.FPDF_GetPageHeightF(page) }); // WHAT: its height. WHY: the same.
        let ks = (OVERLAY_DPI / 72.0).min((OVERLAY_PIXELS / (width * height).max(1.0)).sqrt()); // WHAT: the overlay's scale. WHY: the same as a laid-over sheet.
        let (w, h) = ((width * ks).round().max(1.0) as u32, (height * ks).round().max(1.0) as u32); // WHAT: its size. WHY: the same.
        let b = self.drawing_alone(page, [ks, 0.0, 0.0, ks, 0.0, 0.0], w, h); // WHAT: the drawing. WHY: all of it is new.
        unsafe { self.bindings.FPDF_ClosePage(page) }; // WHAT: closes it. WHY: done.
        let b = b?; // WHAT: the drawing, or nothing. WHY: the caller says so.
        let blank = vec![255u8; b.len()]; // WHAT: no older sheet: white paper. WHY: everything on it arrived with this issue.
        let pixels = crate::overlay::colour(&blank, &b, w as usize, h as usize, darkness, 1); // WHAT: all blue. WHY: all new.
        Some(Overlaid { image: colour_image(&pixels, w as usize, h as usize), agreement: 0.0, lined_up: true, size: [width, height], said: String::new() }) // WHAT: the page. WHY: it keeps its place in the set.
    }

    /// The sheet numbers in a document opened aside, read the same way the
    /// sheet list reads them.
    ///
    /// The same reader on purpose: slip-sheeting matches on these, and a
    /// revision that matches in the sheet list but not in the slip-sheet would
    /// be the worst kind of bug — one where the screen says the operation
    /// should have worked.
    fn sheet_numbers(&self, doc: FPDF_DOCUMENT, count: usize) -> Vec<String> {
        let mut out = Vec::with_capacity(count);
        for index in 0..count {
            let page = unsafe { self.bindings.FPDF_LoadPage(doc, index as i32) };
            if page.is_null() {
                out.push(String::new());
                continue;
            }
            let width = unsafe { self.bindings.FPDF_GetPageWidthF(page) };
            let height = unsafe { self.bindings.FPDF_GetPageHeightF(page) };
            let mut area = FS_RECTF {
                left: 0.0,
                top: height,
                right: width,
                bottom: 0.0,
            };
            unsafe { self.bindings.FPDF_GetPageBoundingBox(page, &mut area) };
            let text = unsafe { self.bindings.FPDFText_LoadPage(page) };
            if text.is_null() {
                unsafe { self.bindings.FPDF_ClosePage(page) };
                out.push(String::new());
                continue;
            }
            let runs = self.runs(text, area.left, area.top.max(area.bottom));
            unsafe { self.bindings.FPDFText_ClosePage(text) };
            unsafe { self.bindings.FPDF_ClosePage(page) };
            out.push(label_from_runs(&runs, PageSize { width, height }).number);
        }
        out
    }

    fn extract(&mut self, id: u64, pages: &[u32], to: &std::path::Path) -> Result<usize, String> {
        let Some(at) = self.current(id) else {
            return Err("that drawing is not open".into());
        };
        let source = self.open[at].doc;
        let count = self.open[at].sizes.len() as u32;
        let wanted: Vec<i32> = pages
            .iter()
            .filter(|p| **p < count)
            .map(|p| *p as i32)
            .collect();
        if wanted.is_empty() {
            return Err("no sheets to print".into());
        }

        let fresh = unsafe { self.bindings.FPDF_CreateNewDocument() };
        if fresh.is_null() {
            return Err("could not make a file to print from".into());
        }
        // Everything below must close `fresh` on the way out, hence the closure
        // rather than an early return.
        let done = (|| -> Result<usize, String> {
            let ok = unsafe {
                self.bindings.FPDF_ImportPagesByIndex(
                    fresh,
                    source,
                    wanted.as_ptr(),
                    wanted.len() as std::os::raw::c_ulong,
                    0,
                )
            };
            if !self.bindings.is_true(ok) {
                return Err("pdfium would not copy those sheets".into());
            }
            let bytes = self.save_to_bytes(fresh)?;
            std::fs::write(to, &bytes).map_err(|e| format!("{}: {e}", to.display()))?;
            Ok(wanted.len())
        })();
        unsafe { self.bindings.FPDF_CloseDocument(fresh) };
        done
    }

    /// Pdfium hands a saved file back a block at a time through a C callback,
    /// so this is the callback, and the struct it is reached through.
    fn save_to_bytes(&self, document: FPDF_DOCUMENT) -> Result<Vec<u8>, String> {
        // The layout pdfium expects, with our own pointer on the end. Pdfium
        // only ever reads the first two fields and passes the whole pointer
        // back, which is what makes the third one safe to hang off it.
        #[repr(C)]
        struct Writer {
            version: std::os::raw::c_int,
            write_block: unsafe extern "C" fn(
                *mut Writer,
                *const c_void,
                std::os::raw::c_ulong,
            ) -> std::os::raw::c_int,
            into: *mut Vec<u8>,
        }

        unsafe extern "C" fn write_block(
            me: *mut Writer,
            data: *const c_void,
            size: std::os::raw::c_ulong,
        ) -> std::os::raw::c_int {
            if me.is_null() || data.is_null() {
                return 0;
            }
            let into = (*me).into;
            if into.is_null() {
                return 0;
            }
            let block = std::slice::from_raw_parts(data as *const u8, size as usize);
            (*into).extend_from_slice(block);
            1
        }

        let mut out: Vec<u8> = Vec::new();
        let mut writer = Writer {
            version: 1,
            write_block,
            into: &mut out as *mut Vec<u8>,
        };
        let ok = unsafe {
            self.bindings.FPDF_SaveAsCopy(
                document,
                &mut writer as *mut Writer as *mut pdfium_render::prelude::FPDF_FILEWRITE,
                0,
            )
        };
        if !self.bindings.is_true(ok) || out.is_empty() {
            return Err("pdfium could not write the file".into());
        }
        Ok(out)
    }

    fn load(&mut self, id: u64, path: &std::path::Path) -> Result<Vec<PageSize>, Trouble> {
        while self.open.len() >= OPEN_DOCUMENTS {
            let oldest = self.open.remove(0);
            self.let_go(oldest);
        }
        let password = self.password_for(path);
        let map = held::bytes(path).map_err(|e| Trouble::from(format!("{}: {e}", path.display())))?;
        // Pdfium keeps this pointer for the life of the document; the bytes
        // are held in `self.map` and only dropped in `close`. An Arc'd Vec
        // does not move its buffer, so the pointer stays good.
        let bytes: &[u8] = unsafe { std::slice::from_raw_parts(map.as_ptr(), map.len()) };
        let doc = unsafe {
            self.bindings.FPDF_LoadMemDocument64(
                bytes,
                if password.is_empty() { None } else { Some(&password) },
            )
        };
        if doc.is_null() {
            let code = unsafe { self.bindings.FPDF_GetLastError() };
            return Err(open_trouble(code, !password.is_empty()));
        }
        let count = unsafe { self.bindings.FPDF_GetPageCount(doc) }.max(0) as u32;
        let mut sizes = Vec::with_capacity(count as usize);
        for i in 0..count {
            let mut s = FS_SIZEF {
                width: 612.0,
                height: 792.0,
            };
            unsafe {
                self.bindings
                    .FPDF_GetPageSizeByIndexF(doc, i as i32, &mut s)
            };
            sizes.push(PageSize {
                width: s.width,
                height: s.height,
            });
        }
        self.open.push(Loaded {
            id,
            map: Some(map),
            doc,
            sizes: sizes.clone(),
            pages: Vec::new(),
        });
        Ok(sizes)
    }

    fn size(&mut self, id: u64, index: u32) -> Option<PageSize> {
        let at = self.current(id)?;
        self.open[at].sizes.get(index as usize).copied()
    }

    fn page(&mut self, id: u64, index: u32) -> Option<FPDF_PAGE> {
        let at = self.current(id)?;
        if self.open[at].doc.is_null() || index as usize >= self.open[at].sizes.len() {
            return None;
        }
        // Whichever form environment belongs to this drawing, so a sheet with
        // form fields on it draws them.
        let document = self.open[at].doc;
        self.drawing_form = self
            .forms
            .iter()
            .find(|(doc, _, _)| *doc == document)
            .map(|(_, handle, _)| *handle);
        if let Some(pos) = self.open[at].pages.iter().position(|(i, _)| *i == index) {
            let entry = self.open[at].pages.remove(pos);
            let handle = entry.1;
            self.open[at].pages.push(entry);
            return Some(handle);
        }
        let handle = unsafe { self.bindings.FPDF_LoadPage(document, index as i32) };
        if handle.is_null() {
            return None;
        }
        while self.open[at].pages.len() >= PAGE_CACHE {
            let (_, old) = self.open[at].pages.remove(0);
            unsafe { self.bindings.FPDF_ClosePage(old) };
        }
        self.open[at].pages.push((index, handle));
        Some(handle)
    }

    /// Renders a rectangle of the page's raster at `scale` pixels per point.
    /// `x0`/`y0` are the top-left of that rectangle inside the full raster.
    fn raster(
        &mut self,
        page: FPDF_PAGE,
        scale: f64,
        x0: f64,
        y0: f64,
        width: u32,
        height: u32,
    ) -> Option<egui::ColorImage> {
        if width == 0 || height == 0 {
            return None;
        }
        let mut buf = vec![0u8; width as usize * height as usize * 4];
        unsafe {
            let bitmap = self.bindings.FPDFBitmap_CreateEx(
                width as i32,
                height as i32,
                FPDFBITMAP_BGRA as i32,
                buf.as_mut_ptr() as *mut c_void,
                (width * 4) as i32,
            );
            if bitmap.is_null() {
                return None;
            }
            self.bindings.FPDFBitmap_FillRect(
                bitmap,
                0,
                0,
                width as i32,
                height as i32,
                0xFFFF_FFFFu32 as _,
            );
            let matrix = FS_MATRIX {
                a: scale as f32,
                b: 0.0,
                c: 0.0,
                d: scale as f32,
                e: -x0 as f32,
                f: -y0 as f32,
            };
            let clip = FS_RECTF {
                left: 0.0,
                top: 0.0,
                right: width as f32,
                bottom: height as f32,
            };
            // REVERSE_BYTE_ORDER turns pdfium's native BGRA into the RGBA that
            // the GPU upload wants, which saves a full pass over every pixel.
            self.bindings.FPDF_RenderPageBitmapWithMatrix(
                bitmap,
                page,
                &matrix,
                &clip,
                (FPDF_ANNOT | FPDF_REVERSE_BYTE_ORDER) as i32,
            );
            // Form fields are a pass of their own in pdfium, and only happen
            // when it has a form environment. The matrix above is a scale and
            // a shift, which is exactly what this call takes in pieces.
            if let Some(form) = self.drawing_form {
                let full_w = (self.bindings.FPDF_GetPageWidthF(page) as f64 * scale).round();
                let full_h = (self.bindings.FPDF_GetPageHeightF(page) as f64 * scale).round();
                self.bindings.FPDF_FFLDraw(
                    form,
                    bitmap,
                    page,
                    -(x0.round() as i32),
                    -(y0.round() as i32),
                    full_w as i32,
                    full_h as i32,
                    0,
                    (FPDF_ANNOT | FPDF_REVERSE_BYTE_ORDER) as i32,
                );
            }
            self.bindings.FPDFBitmap_Destroy(bitmap);
        }
        Some(egui::ColorImage::from_rgba_unmultiplied(
            [width as usize, height as usize],
            &buf,
        ))
    }

    fn tile(&mut self, key: TileKey) -> Option<egui::ColorImage> {
        let id = key.doc;
        let size = self.size(id, key.page)?;
        let scale = bucket_scale(key.bucket);
        let (x0, y0, w, h) = tile_extent(size, key.bucket, key.tx, key.ty)?;
        let page = self.page(id, key.page)?;
        self.raster(page, scale, x0, y0, w, h)
    }

    /// Dynamic Fill: draws the part of the sheet somebody is looking at, turns
    /// it into walls and space, and finds the shape they clicked inside.
    ///
    /// The drawing is only ever looked at through the window they can see. That
    /// is not a shortcut — it is what makes an escape detectable. A fill that
    /// reaches the edge of what was looked at has either found an opening or
    /// covers more than the screen, and either way the honest answer is to say
    /// so rather than to widen the search until something closes.
    /// Cuts the symbol somebody drew a box round out of its sheet.
    ///
    /// Rendered a little finer than a screen pixel so a thin line does not fall
    /// between two cells and disappear, and capped so a box drawn round half
    /// the drawing does not turn into an enormous template nothing can be
    /// compared against quickly.
    fn cut_symbol(
        &mut self,
        id: u64,
        index: u32,
        area: [f64; 4],
        darkness: f32,
    ) -> Option<(takeoff::symbols::Ink, (f64, f64), f64)> {
        let size = self.size(id, index)?;
        let x0 = area[0].max(0.0);
        let y0 = area[1].max(0.0);
        let x1 = area[2].min(size.width as f64);
        let y1 = area[3].min(size.height as f64);
        let (across, down) = (x1 - x0, y1 - y0);
        // A symbol has to be big enough to be a shape and small enough to be a
        // symbol. Half a sheet is a region, not a thing to count.
        if across < 2.0 || down < 2.0 || across > size.width as f64 * 0.4 {
            return None;
        }
        const MOST: f64 = 160.0;
        let scale = (MOST / across.max(down)).min(3.0).max(0.5);
        let w = (across * scale).round().max(2.0) as u32;
        let h = (down * scale).round().max(2.0) as u32;

        let page = self.page(id, index)?;
        let image = self.raster(page, scale, x0 * scale, y0 * scale, w, h)?;
        let ink = to_ink(&image, darkness);
        Some((
            takeoff::symbols::Ink::new(w as usize, h as usize, ink),
            (across, down),
            scale,
        ))
    }

    /// Looks for the symbol on one sheet.
    fn look_for(
        &mut self,
        id: u64,
        index: u32,
        symbol: &takeoff::symbols::Ink,
        on_sheet: (f64, f64),
        scale: f64,
        alike: f32,
        darkness: f32,
    ) -> Vec<[f64; 4]> {
        let Some(size) = self.size(id, index) else {
            return Vec::new();
        };
        let w = (size.width as f64 * scale).round().max(1.0) as u32;
        let h = (size.height as f64 * scale).round().max(1.0) as u32;
        // A whole sheet at symbol resolution is a big picture; beyond this it
        // is not worth the wait, and the answer would be no better.
        if (w as u64) * (h as u64) > 40_000_000 {
            return Vec::new();
        }
        let Some(page) = self.page(id, index) else {
            return Vec::new();
        };
        let Some(image) = self.raster(page, scale, 0.0, 0.0, w, h) else {
            return Vec::new();
        };
        let sheet = takeoff::symbols::Ink::new(w as usize, h as usize, to_ink(&image, darkness));
        takeoff::symbols::find(&sheet, symbol, alike, 4000)
            .into_iter()
            .map(|s| {
                let x = s.x as f64 / scale;
                let y = s.y as f64 / scale;
                [x, y, x + on_sheet.0, y + on_sheet.1]
            })
            .collect()
    }

    fn dynamic_fill(
        &mut self,
        id: u64,
        index: u32,
        area: [f64; 4],
        seed: [f64; 2],
        strokes: &[Vec<[f64; 2]>],
        darkness: f32,
    ) -> FillOutcome {
        let Some(size) = self.size(id, index) else {
            return FillOutcome::CouldNotLook;
        };
        // Clamp to the sheet: there is nothing to look at beyond its edges.
        let x0 = area[0].max(0.0);
        let y0 = area[1].max(0.0);
        let x1 = area[2].min(size.width as f64);
        let y1 = area[3].min(size.height as f64);
        let (across, down) = (x1 - x0, y1 - y0);
        if across < 4.0 || down < 4.0 {
            return FillOutcome::CouldNotLook;
        }

        // Fine enough to see a wall, coarse enough to be instant. A cell is
        // roughly a third of a sheet point, which on quarter inch scale is
        // about an inch and a half of building.
        const MOST_CELLS: f64 = 2_600_000.0;
        let wanted = 3.0f64;
        let scale = (MOST_CELLS / (across * down)).sqrt().min(wanted).max(0.4);
        let w = (across * scale).round().max(1.0) as u32;
        let h = (down * scale).round().max(1.0) as u32;

        let Some(page) = self.page(id, index) else {
            return FillOutcome::CouldNotLook;
        };
        let Some(image) = self.raster(page, scale, x0 * scale, y0 * scale, w, h) else {
            return FillOutcome::CouldNotLook;
        };

        let mut mask = takeoff::fill::Mask::new(w as usize, h as usize);
        let cut = (darkness.clamp(0.05, 0.99) * 255.0) as u32;
        for (i, pixel) in image.pixels.iter().enumerate() {
            // Plain average rather than a weighted luminance: a drawing is ink
            // on paper, and a coloured markup somebody has already put on it
            // should stop a fill just as a black line does.
            let grey = (pixel.r() as u32 + pixel.g() as u32 + pixel.b() as u32) / 3;
            if grey < cut {
                mask.solid[i] = true;
            }
        }

        // The lines somebody drew to close an opening, at the same resolution.
        for stroke in strokes {
            for pair in stroke.windows(2) {
                mask.draw_line(
                    ((pair[0][0] - x0) * scale, (pair[0][1] - y0) * scale),
                    ((pair[1][0] - x0) * scale, (pair[1][1] - y0) * scale),
                    2.0,
                );
            }
        }

        let sx = ((seed[0] - x0) * scale).round();
        let sy = ((seed[1] - y0) * scale).round();
        if sx < 0.0 || sy < 0.0 || sx >= w as f64 || sy >= h as f64 {
            return FillOutcome::OnALine;
        }

        let back = |p: &[f64; 2]| [x0 + p[0] / scale, y0 + p[1] / scale];
        match takeoff::fill::flood(&mask, (sx as usize, sy as usize), takeoff::fill::MOST) {
            takeoff::fill::Fill::OnALine => FillOutcome::OnALine,
            takeoff::fill::Fill::Escaped(why) => FillOutcome::Escaped(why.why()),
            takeoff::fill::Fill::Found(region) => FillOutcome::Found {
                outline: region.outline.iter().map(back).collect(),
                holes: region
                    .holes
                    .iter()
                    .map(|h| h.iter().map(back).collect())
                    .collect(),
                // From the cells that were filled, in square sheet points.
                area: region.cells as f64 / (scale * scale),
                enclosed: region.hole_cells as f64 / (scale * scale),
                not_drawn: region.holes_not_drawn,
                resolution: 1.0 / scale,
            },
        }
    }

    fn whole(&mut self, id: u64, index: u32, target_px: f64) -> Option<(f32, egui::ColorImage)> {
        let size = self.size(id, index)?;
        let longest = size.width.max(size.height) as f64;
        if longest <= 0.0 {
            return None;
        }
        let scale = (target_px / longest).min(2.0);
        let w = (size.width as f64 * scale).round().max(1.0) as u32;
        let h = (size.height as f64 * scale).round().max(1.0) as u32;
        let page = self.page(id, index)?;
        let image = self.raster(page, scale, 0.0, 0.0, w, h)?;
        Some((scale as f32, image))
    }

    fn preview(&mut self, id: u64, index: u32) -> Option<(f32, egui::ColorImage)> {
        self.whole(id, index, PREVIEW_PX)
    }

    fn thumb(&mut self, id: u64, index: u32) -> Option<egui::ColorImage> {
        self.whole(id, index, THUMB_PX).map(|(_, image)| image)
    }

    fn label(&mut self, id: u64, index: u32) -> SheetLabel {
        let Some(size) = self.size(id, index) else {
            return SheetLabel::default();
        };
        let Some(page) = self.page(id, index) else {
            return SheetLabel::default();
        };
        // Drawing sheets are often laid out around the origin rather than up
        // from (0,0), so character positions have to be measured from the
        // page's own box, not from zero.
        let mut area = FS_RECTF {
            left: 0.0,
            top: size.height,
            right: size.width,
            bottom: 0.0,
        };
        unsafe { self.bindings.FPDF_GetPageBoundingBox(page, &mut area) };
        let text = unsafe { self.bindings.FPDFText_LoadPage(page) };
        if text.is_null() {
            return SheetLabel::default();
        }
        let runs = self.runs(text, area.left, area.top.max(area.bottom));
        unsafe { self.bindings.FPDFText_ClosePage(text) };
        if std::env::var("RIVET_DEBUG_LABEL").ok().as_deref() == Some(&index.to_string()) {
            let (mut ax0, mut ay0, mut ax1, mut ay1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for r in &runs {
                ax0 = ax0.min(r.area.x0); ay0 = ay0.min(r.area.y0);
                ax1 = ax1.max(r.area.x1); ay1 = ay1.max(r.area.y1);
            }
            eprintln!("-- page {index}: {} runs, median size {:.1}, page {}x{}, text bounds {ax0:.0},{ay0:.0} .. {ax1:.0},{ay1:.0}",
                runs.len(), median_size(&runs), size.width, size.height);
            for r in runs.iter().take(6) {
                eprintln!("   first: size {:5.1} box {:.0},{:.0}..{:.0},{:.0} {:?}", r.size, r.area.x0, r.area.y0, r.area.x1, r.area.y1, tidy(&r.text));
            }
            for r in &runs {
                let cx = (r.area.x0 + r.area.x1) * 0.5 / size.width;
                let cy = (r.area.y0 + r.area.y1) * 0.5 / size.height;
                if cx > 0.75 && cy > 0.8 {
                    eprintln!("   size {:5.1} at {cx:.3},{cy:.3}  {:?}", r.size, tidy(&r.text));
                }
            }
        }
        let mut label = label_from_runs(&runs, size);
        label.grid = grid_from_runs(&runs, size);
        label
    }

    /// Finds every place a phrase appears on one sheet.
    ///
    /// The search is done here rather than with pdfium's own, because a drawing
    /// is not prose: `W12x26` has to be findable as a whole word without
    /// `W12x26A` answering, and a sheet's text arrives with the newlines and
    /// stray spacing that CAD output leaves behind. Doing it over the
    /// characters themselves is the only way to be sure what matched.
    fn find(
        &mut self,
        id: u64,
        index: u32,
        needle: &str,
        whole_words: bool,
        match_case: bool,
    ) -> Vec<Hit> {
        let Some(size) = self.size(id, index) else {
            return Vec::new();
        };
        let Some(page) = self.page(id, index) else {
            return Vec::new();
        };
        let text = unsafe { self.bindings.FPDFText_LoadPage(page) };
        if text.is_null() {
            return Vec::new();
        }

        let count = unsafe { self.bindings.FPDFText_CountChars(text) }.max(0);
        let mut chars: Vec<char> = Vec::with_capacity(count as usize);
        let mut boxes: Vec<Box2> = Vec::with_capacity(count as usize);
        for i in 0..count {
            let code = unsafe { self.bindings.FPDFText_GetUnicode(text, i) };
            let Some(ch) = char::from_u32(code) else { continue };
            let (mut l, mut r, mut b, mut t) = (0.0, 0.0, 0.0, 0.0);
            let ok = unsafe {
                self.bindings
                    .FPDFText_GetCharBox(text, i, &mut l, &mut r, &mut b, &mut t)
            };
            chars.push(ch);
            boxes.push(if ok != 0 {
                Box2 {
                    x0: l.min(r) as f32,
                    y0: b.min(t) as f32,
                    x1: l.max(r) as f32,
                    y1: b.max(t) as f32,
                }
            } else {
                // A character with no box — a soft hyphen, say — still counts
                // for matching, so it keeps its place with an empty one.
                Box2 { x0: 0.0, y0: 0.0, x1: 0.0, y1: 0.0 }
            });
        }

        let hay: Vec<char> = if match_case {
            chars.clone()
        } else {
            chars.iter().flat_map(|c| c.to_lowercase()).collect()
        };
        // Folding case can change how many characters there are, and a
        // position in the folded text would then point at the wrong place in
        // the real one. When that happens the search falls back to the
        // characters as they are rather than highlighting the wrong words.
        let hay = if hay.len() == chars.len() { hay } else { chars.clone() };
        let pins: Vec<char> = if match_case {
            needle.chars().collect()
        } else {
            needle.chars().flat_map(|c| c.to_lowercase()).collect()
        };

        let mut hits = Vec::new();
        if !pins.is_empty() && hay.len() >= pins.len() {
            let mut at = 0usize;
            while at + pins.len() <= hay.len() {
                if hay[at..at + pins.len()] == pins[..] {
                    let end = at + pins.len();
                    if !whole_words || is_whole_word(&hay, at, end) {
                        if let Some(hit) =
                            self.hit(page, &chars, &boxes, at, end, size)
                        {
                            hits.push(hit);
                        }
                        at = end;
                        continue;
                    }
                }
                at += 1;
            }
        }

        unsafe { self.bindings.FPDFText_ClosePage(text) };
        hits.into_iter()
            .map(|mut hit| {
                hit.page = index;
                hit
            })
            .collect()
    }

    /// Turns a matched range into something the list can show and the canvas
    /// can point at.
    fn hit(
        &self,
        page: FPDF_PAGE,
        chars: &[char],
        boxes: &[Box2],
        from: usize,
        to: usize,
        size: PageSize,
    ) -> Option<Hit> {
        // The box around the match, in the page's own space.
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for b in &boxes[from..to] {
            if b.x1 <= b.x0 && b.y1 <= b.y0 {
                continue;
            }
            x0 = x0.min(b.x0);
            y0 = y0.min(b.y0);
            x1 = x1.max(b.x1);
            y1 = y1.max(b.y1);
        }
        if x0 > x1 || y0 > y1 {
            return None;
        }

        // Through pdfium's own transform, so a rotated sheet lands where the
        // screen actually draws it rather than where the page stores it.
        let to_sheet = |x: f32, y: f32| -> (f64, f64) {
            let (mut dx, mut dy) = (0, 0);
            unsafe {
                self.bindings.FPDF_PageToDevice(
                    page,
                    0,
                    0,
                    size.width.round() as i32,
                    size.height.round() as i32,
                    0,
                    x as f64,
                    y as f64,
                    &mut dx,
                    &mut dy,
                )
            };
            (dx as f64, dy as f64)
        };
        let (ax, ay) = to_sheet(x0, y1);
        let (bx, by) = to_sheet(x1, y0);
        let area = [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)];

        // Enough either side to tell two matches apart, without turning the
        // list into a wall of text.
        const AROUND: usize = 34;
        let start = from.saturating_sub(AROUND);
        let stop = (to + AROUND).min(chars.len());
        let before: String = squash(&chars[start..from]);
        let matched: String = chars[from..to].iter().collect();
        let after: String = squash(&chars[to..stop]);
        let mut context = String::new();
        if start > 0 {
            context.push('…');
        }
        context.push_str(&before);
        let at = context.len()..context.len() + matched.len();
        context.push_str(&matched);
        context.push_str(&after);
        if stop < chars.len() {
            context.push('…');
        }

        Some(Hit {
            page: 0,
            context,
            at,
            area,
        })
    }

    /// Walks the page's characters and groups them into runs of text that were
    /// drawn together, keeping each run's box and font size.
    fn runs(&self, text: FPDF_TEXTPAGE, origin_x: f32, origin_y: f32) -> Vec<Run> {
        let count = unsafe { self.bindings.FPDFText_CountChars(text) }.max(0);
        let mut runs: Vec<Run> = Vec::new();
        let mut current: Option<Run> = None;
        for i in 0..count.min(MAX_CHARS) {
            let code = unsafe { self.bindings.FPDFText_GetUnicode(text, i) };
            let (mut l, mut r, mut b, mut t) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
            let ok = unsafe {
                self.bindings
                    .FPDFText_GetCharBox(text, i, &mut l, &mut r, &mut b, &mut t)
            };
            let ch = char::from_u32(code).unwrap_or('\u{fffd}');
            let breaks = ch == '\n' || ch == '\r' || ok == 0;
            if breaks {
                if let Some(run) = current.take() {
                    runs.push(run);
                }
                continue;
            }
            // Character boxes arrive in PDF space (origin bottom left); the rest
            // of the program measures from the top left corner of the sheet.
            let boxx = Box2 {
                x0: l as f32 - origin_x,
                y0: origin_y - t as f32,
                x1: r as f32 - origin_x,
                y1: origin_y - b as f32,
            };
            // FPDFText_GetFontSize reports the raw Tf size, which on CAD output
            // is routinely 1.0 with all the scaling in the text matrix. The
            // character's own box is the only honest measure of how big it looks.
            let size = (boxx.x1 - boxx.x0).max(boxx.y1 - boxx.y0);
            // A space carries no useful box; it joins the run it sits inside
            // without disturbing the run's measurements.
            if ch == ' ' || ch == '\u{a0}' || ch == '\t' {
                if let Some(run) = current.as_mut() {
                    run.text.push(' ');
                }
                continue;
            }
            // Punctuation is tiny next to the letters it belongs to. Judged on
            // size it would split "A0.01" into three pieces, so it is only ever
            // asked to be close by.
            let punctuation = !ch.is_alphanumeric();
            match current.as_mut() {
                Some(run) if run.continues(&boxx, size, punctuation) => {
                    run.push(ch, &boxx, punctuation)
                }
                _ => {
                    if let Some(run) = current.take() {
                        runs.push(run);
                    }
                    current = Some(Run::new(ch, boxx, size));
                }
            }
        }
        if let Some(run) = current.take() {
            runs.push(run);
        }
        runs
    }
}

const MAX_CHARS: i32 = 60_000;

/// More line-work than this on one sheet is a scanned drawing traced into
/// dust, and no plugin is helped by being handed all of it.
const MOST_LINES_FOR_A_PLUGIN: usize = 150_000;

/// A sheet as a plugin sees it.
#[derive(Clone, Debug, Default)]
pub struct Reading {
    /// The sheet's width and height, in points.
    pub size: (f64, f64),
    pub words: Vec<plugin_api::Word>,
    pub lines: Vec<plugin_api::Line>,
    pub cut_short: bool,
}

fn push_line(out: &mut Reading, a: [f64; 2], b: [f64; 2], width: f64, dashed: bool) {
    // A zero-length piece is a dot, and a dot is not a line.
    if (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01 {
        return;
    }
    out.lines.push(plugin_api::Line { a, b, width, dashed });
}

#[derive(Clone, Copy, Debug)]
struct Box2 {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

#[derive(Clone, Debug)]
struct Run {
    text: String,
    area: Box2,
    size: f32,
}

impl Run {
    fn new(ch: char, area: Box2, size: f32) -> Run {
        let mut text = String::new();
        text.push(ch);
        Run { text, area, size }
    }

    fn continues(&self, next: &Box2, size: f32, punctuation: bool) -> bool {
        if !punctuation && (size - self.size).abs() > self.size.max(0.5) * 0.35 {
            return false;
        }
        let gap_x = next.x0 - self.area.x1;
        let gap_y = next.y0 - self.area.y1;
        let reach = self.size.max(1.0) * 2.0;
        let overlaps_rows = next.y0 < self.area.y1 && next.y1 > self.area.y0;
        let overlaps_cols = next.x0 < self.area.x1 && next.x1 > self.area.x0;
        (overlaps_rows && gap_x.abs() < reach) || (overlaps_cols && gap_y.abs() < reach)
    }

    fn push(&mut self, ch: char, area: &Box2, punctuation: bool) {
        self.text.push(ch);
        if !punctuation {
            self.size = self.size.max((area.x1 - area.x0).max(area.y1 - area.y0));
        }
        self.area.x0 = self.area.x0.min(area.x0);
        self.area.y0 = self.area.y0.min(area.y0);
        self.area.x1 = self.area.x1.max(area.x1);
        self.area.y1 = self.area.y1.max(area.y1);
    }
}

/// A sheet number is short, starts with letters and ends in digits: `S2.1`,
/// `A101`, `E-100`, `M1.02A`.
pub fn looks_like_sheet_number(s: &str) -> bool {
    let s = s.trim();
    if s.len() < 2 || s.len() > 10 || !s.is_ascii() {
        return false;
    }
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_alphabetic() {
        i += 1;
    }
    if i == 0 || i > 3 || !s[..i].chars().all(|c| c.is_ascii_uppercase()) {
        return false;
    }
    if i < b.len() && (b[i] == b'-' || b[i] == b'.') {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
        i += 1;
    }
    if i == digits_start {
        return false;
    }
    if i < b.len() && b[i].is_ascii_uppercase() {
        i += 1;
    }
    i == b.len()
}

fn tidy(s: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for ch in s.trim().chars() {
        if ch.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(ch);
    }
    out
}

/// Sheet titles wrap: "ABBREVIATIONS, SYMBOLS," on one line and "LEGENDS &
/// SCHEDULES" on the next. Gathers the lines stacked with the chosen one so the
/// sidebar shows the whole title.
fn stacked_title(runs: &[Run], chosen: &Run) -> String {
    let mut lines: Vec<&Run> = runs
        .iter()
        .filter(|r| {
            (r.size - chosen.size).abs() <= chosen.size * 0.12
                && r.area.x0 < chosen.area.x1
                && r.area.x1 > chosen.area.x0
                && could_be_a_sheet_title(&tidy(&r.text))
        })
        .collect();
    lines.sort_by(|a, b| {
        a.area
            .y0
            .partial_cmp(&b.area.y0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let here = lines
        .iter()
        .position(|r| std::ptr::eq(*r, chosen))
        .unwrap_or(0);
    let mut first = here;
    while first > 0 {
        let gap = lines[first].area.y0 - lines[first - 1].area.y1;
        if gap > chosen.size * 1.2 || gap < -chosen.size {
            break;
        }
        first -= 1;
    }
    let mut last = here;
    while last + 1 < lines.len() {
        let gap = lines[last + 1].area.y0 - lines[last].area.y1;
        if gap > chosen.size * 1.2 || gap < -chosen.size {
            break;
        }
        last += 1;
    }
    let joined: Vec<String> = lines[first..=last].iter().map(|r| tidy(&r.text)).collect();
    let text = tidy(&joined.join(" "));
    text.chars().take(72).collect()
}

fn median_size(runs: &[Run]) -> f32 {
    let mut sizes: Vec<f32> = runs.iter().map(|r| r.size).collect();
    if sizes.is_empty() {
        return 0.0;
    }
    sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sizes[sizes.len() / 2]
}

/// Rejects the field values that sit beside the sheet title in a title block —
/// `08/17/2026`, `19003`, `DVU/NJP`, `JOB NO.:` — so they cannot be mistaken
/// for it.
fn could_be_a_sheet_title(text: &str) -> bool {
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    if text.len() < 4 || text.len() > 60 || letters * 2 < text.len() {
        return false;
    }
    if text.contains(':') {
        return false;
    }
    // Initials like DVU/NJP; a real title would have a space before any slash.
    if text.contains('/') && !text.contains(' ') {
        return false;
    }
    true
}

/// The grid bubbles among a sheet's text runs.
///
/// Everything on the sheet is offered; `takeoff::grid` decides what is a grid
/// and what is not, and on most detail sheets the answer is nothing.
fn grid_from_runs(runs: &[Run], size: PageSize) -> takeoff::grid::Grid {
    let marks: Vec<takeoff::grid::Mark> = runs
        .iter()
        .filter_map(|run| {
            let label = run.text.trim().to_string();
            (!label.is_empty()).then(|| takeoff::grid::Mark {
                label,
                at: [
                    ((run.area.x0 + run.area.x1) * 0.5) as f64,
                    ((run.area.y0 + run.area.y1) * 0.5) as f64,
                ],
            })
        })
        .collect();
    takeoff::grid::read(
        &marks,
        [size.width as f64, size.height as f64],
        takeoff::grid::HowClose::default(),
    )
}

fn label_from_runs(runs: &[Run], size: PageSize) -> SheetLabel {
    let median = median_size(runs);

    // The sheet number lives in the very corner of the sheet and is drawn far
    // larger than the body text around it. Both tests matter: without the size
    // test, a specification page happily offers up something like "B3110".
    let floor = (median * 1.5).max(9.0);
    let mut number: Option<&Run> = None;
    for run in runs {
        let cx = (run.area.x0 + run.area.x1) * 0.5;
        let cy = (run.area.y0 + run.area.y1) * 0.5;
        if cx < size.width * 0.80 || cy < size.height * 0.85 || run.size < floor {
            continue;
        }
        let text = tidy(&run.text);
        if !looks_like_sheet_number(&text) {
            continue;
        }
        let better = match number {
            None => true,
            Some(best) => {
                run.size > best.size * 1.02
                    || ((run.size - best.size).abs() <= best.size * 0.02
                        && run.area.y1 > best.area.y1)
            }
        };
        if better {
            number = Some(run);
        }
    }

    // The title sits directly above the number in the same column of the title
    // block. Take the largest text in that band, and the highest one on a tie,
    // which skips the job number / date / drawn-by rows underneath it.
    let mut title: Option<&Run> = None;
    if let Some(num) = number {
        let top = num.area.y0 - size.height * 0.20;
        for run in runs {
            let cx = (run.area.x0 + run.area.x1) * 0.5;
            if std::ptr::eq(run, num)
                || cx < size.width * 0.88
                || run.area.y1 > num.area.y0
                || run.area.y0 < top
                || run.size < num.size * 0.3
            {
                continue;
            }
            if !could_be_a_sheet_title(&tidy(&run.text)) {
                continue;
            }
            let better = match title {
                None => true,
                Some(best) => {
                    run.size > best.size * 1.02
                        || ((run.size - best.size).abs() <= best.size * 0.02
                            && run.area.y0 < best.area.y0)
                }
            };
            if better {
                title = Some(run);
            }
        }
    }

    match number {
        Some(n) => SheetLabel {
            number: tidy(&n.text),
            title: title.map(|t| stacked_title(runs, t)).unwrap_or_default(),
            // Filled in by the caller, which has the runs to read it from.
            grid: Default::default(),
        },
        // Specification pages have no title block; their first line of text
        // ("SECTION 26 05 19") is a far better label than "Page 437".
        None => {
            let first = runs
                .iter()
                .map(|r| tidy(&r.text))
                .find(|t| t.len() >= 6)
                .unwrap_or_default();
            SheetLabel {
                number: String::new(),
                title: first.chars().take(48).collect(),
                grid: Default::default(),
            }
        }
    }
}

// ---- changing the words on a sheet -------------------------------------------

/// What a text object is, for the finder: pieces of text in the page's order.
const OBJECT_TEXT: u32 = 1;
/// Text drawn invisibly: the searchable words OCR lays under a scan. Not
/// something to edit, because editing it changes nothing anybody sees.
const INVISIBLE: i32 = 3;
/// Save only what changed, after the file as it was.
const FPDF_INCREMENTAL: u32 = 1;

/// A piece of text on a page, with the handles to change it by.
struct Piece {
    run: crate::textedit::Run,
    object: FPDF_PAGEOBJECT,
    font: FPDF_FONT,
    embedded: bool,
}

impl Engine {
    /// Every piece of visible text on a page.
    fn pieces(&self, page: FPDF_PAGE) -> Vec<Piece> {
        let b = &self.bindings;
        let mut out = Vec::new();
        let words = unsafe { b.FPDFText_LoadPage(page) };
        let count = unsafe { b.FPDFPage_CountObjects(page) }.max(0);
        for i in 0..count {
            let object = unsafe { b.FPDFPage_GetObject(page, i) };
            if object.is_null() || unsafe { b.FPDFPageObj_GetType(object) } as u32 != OBJECT_TEXT {
                continue;
            }
            if unsafe { b.FPDFTextObj_GetTextRenderMode(object) } as i32 == INVISIBLE {
                continue;
            }
            // Its words.
            let text = unsafe {
                let bytes = b.FPDFTextObj_GetText(object, words, std::ptr::null_mut(), 0) as usize;
                if bytes < 2 {
                    continue;
                }
                let mut buffer = vec![0u16; bytes / 2];
                b.FPDFTextObj_GetText(object, words, buffer.as_mut_ptr() as *mut _, bytes as _);
                while buffer.last() == Some(&0) {
                    buffer.pop();
                }
                String::from_utf16_lossy(&buffer)
            };
            if text.trim().is_empty() {
                continue;
            }
            // Where, which way, and how big.
            let mut m = FS_MATRIX { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };
            unsafe { b.FPDFPageObj_GetMatrix(object, &mut m) };
            let (a, bb, c, d) = (m.a as f64, m.b as f64, m.c as f64, m.d as f64);
            let across = (a * a + bb * bb).sqrt();
            let high = (c * c + d * d).sqrt();
            // Mirrored text, or text squashed flat, is left alone.
            if across < 1e-6 || high < 1e-6 || a * d - bb * c <= 0.0 {
                continue;
            }
            let along = [a / across, bb / across];
            let mut size = 0f32;
            unsafe { b.FPDFTextObj_GetFontSize(object, &mut size) };
            let size = size as f64 * high;
            if size <= 0.1 {
                continue;
            }
            let origin = [m.e as f64, m.f as f64];
            let (mut l, mut bo, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
            unsafe { b.FPDFPageObj_GetBounds(object, &mut l, &mut bo, &mut r, &mut t) };
            let start = origin[0] * along[0] + origin[1] * along[1];
            let furthest = [[l, bo], [r, bo], [r, t], [l, t]]
                .iter()
                .map(|p| p[0] as f64 * along[0] + p[1] as f64 * along[1])
                .fold(f64::MIN, f64::max);
            let (mut cr, mut cg, mut cb, mut ca) = (0u32, 0u32, 0u32, 0u32);
            unsafe { b.FPDFPageObj_GetFillColor(object, &mut cr, &mut cg, &mut cb, &mut ca) };
            // Its font.
            let font = unsafe { b.FPDFTextObj_GetFont(object) };
            let name = if font.is_null() {
                String::new()
            } else {
                unsafe {
                    let n = b.FPDFFont_GetBaseFontName(font, std::ptr::null_mut(), 0);
                    let mut buffer = vec![0u8; n.max(1)];
                    b.FPDFFont_GetBaseFontName(font, buffer.as_mut_ptr() as *mut _, n);
                    while buffer.last() == Some(&0) {
                        buffer.pop();
                    }
                    String::from_utf8_lossy(&buffer).to_string()
                }
            };
            let lower = name.to_lowercase();
            let weight = if font.is_null() { 400 } else { unsafe { b.FPDFFont_GetWeight(font) } };
            let mut slant = 0i32;
            if !font.is_null() {
                unsafe { b.FPDFFont_GetItalicAngle(font, &mut slant) };
            }
            let embedded = !font.is_null() && unsafe { b.FPDFFont_GetIsEmbedded(font) } == 1;
            out.push(Piece {
                run: crate::textedit::Run {
                    object: i as usize,
                    text,
                    origin,
                    along,
                    size,
                    advance: (furthest - start).max(0.0),
                    colour: [cr.min(255) as u8, cg.min(255) as u8, cb.min(255) as u8],
                    bold: weight >= 600 || lower.contains("bold") || lower.contains("black") || lower.contains("heavy"),
                    italic: slant != 0 || lower.contains("italic") || lower.contains("oblique"),
                    font: name,
                },
                object,
                font,
                embedded,
            });
        }
        if !words.is_null() {
            unsafe { b.FPDFText_ClosePage(words) };
        }
        out
    }

    /// The paragraphs on a sheet of a file, read fresh from disk.
    fn paragraphs_on(&mut self, path: &std::path::Path, page: u32) -> Result<Vec<crate::textedit::Found>, String> {
        let one = self.open_alone(path)?;
        let handle = unsafe { self.bindings.FPDF_LoadPage(one.doc, page as i32) };
        if handle.is_null() {
            self.close_alone(one);
            return Err("that sheet could not be read".into());
        }
        let runs: Vec<crate::textedit::Run> = self.pieces(handle).into_iter().map(|p| p.run).collect();
        unsafe { self.bindings.FPDF_ClosePage(handle) };
        self.close_alone(one);
        Ok(crate::textedit::paragraphs(&runs))
    }

    /// A page drawn small enough to compare, the same size every time.
    fn look_at(&mut self, page: FPDF_PAGE) -> Option<egui::ColorImage> {
        let (w, h) = unsafe {
            (self.bindings.FPDF_GetPageWidthF(page) as f64, self.bindings.FPDF_GetPageHeightF(page) as f64)
        };
        let scale = (1600.0 / w.max(h).max(1.0)).min(3.0);
        let (pw, ph) = ((w * scale).ceil() as u32, (h * scale).ceil() as u32);
        self.raster(page, scale, 0.0, 0.0, pw.max(1), ph.max(1))
    }

    /// Writes a paragraph back into a sheet, changed. Comes back with the
    /// whole file as it should now be, and a sentence saying what was done.
    /// Nothing is written to disk here; the window does that.
    fn rewrite_paragraph(
        &mut self,
        path: &std::path::Path,
        page: u32,
        found: &crate::textedit::Found,
        rewrite: &crate::textedit::Rewrite,
    ) -> Result<(Vec<u8>, String), String> {
        // Which of the sheet's fonts are in the file whole. pdfium gives a
        // font's name without the tag that says it was cut down, so that is
        // read from the file itself.
        let whole = std::fs::read(path)
            .map(|bytes| crate::textedit::whole_fonts(&pdf::Document::from_bytes(bytes), page))
            .unwrap_or_default();
        let one = self.open_alone(path)?;
        let result = self.rewrite_in(&one, page, found, rewrite, &whole);
        self.close_alone(one);
        result
    }

    fn rewrite_in(
        &mut self,
        one: &Aside,
        page_index: u32,
        found: &crate::textedit::Found,
        rewrite: &crate::textedit::Rewrite,
        whole: &std::collections::HashSet<String>,
    ) -> Result<(Vec<u8>, String), String> {
        use crate::textedit::{self as te, Family};
        let page = unsafe { self.bindings.FPDF_LoadPage(one.doc, page_index as i32) };
        if page.is_null() {
            return Err("that sheet could not be read".into());
        }
        let result = (|| {
            // The bindings live in a box that stays put for the engine's
            // life, so holding them by pointer while the engine draws is safe
            // and saves threading them through every call.
            let b: &dyn PdfiumLibraryBindings = unsafe { &*(&*self.bindings as *const dyn PdfiumLibraryBindings) };
            let pieces = self.pieces(page);

            // The paragraph's own pieces, found again by their words and
            // where they start.
            let mut used = vec![false; pieces.len()];
            let mut mine: Vec<usize> = Vec::new();
            for (words, at) in &found.pieces {
                let hit = pieces.iter().enumerate().position(|(i, p)| {
                    !used[i]
                        && p.run.text == *words
                        && (p.run.origin[0] - at[0]).abs() < 0.6
                        && (p.run.origin[1] - at[1]).abs() < 0.6
                });
                match hit {
                    Some(i) => {
                        used[i] = true;
                        mine.push(i);
                    }
                    None => {
                        return Err("The words on the sheet have changed since they were read. Click them again.".to_string())
                    }
                }
            }
            let first = &pieces[mine[0]];

            // The font. Its own, when it can set every letter; otherwise the
            // standard one nearest it, and a sentence saying so.
            let letters: String = rewrite.text.chars().filter(|c| !c.is_whitespace()).collect();
            let mut note = String::new();
            let own = if rewrite.family == Family::Keep && !first.font.is_null() {
                let had: std::collections::HashSet<char> = pieces
                    .iter()
                    .filter(|p| p.run.font == first.run.font)
                    .flat_map(|p| p.run.text.chars())
                    .collect();
                // An embedded font is only trusted with letters it hasn't set
                // yet when the file shows it was put in whole.
                let cut_down = first.embedded && !whole.contains(&first.run.font);
                let settable = |c: char| {
                    let mut w = 0f32;
                    let known = b.is_true(unsafe {
                        b.FPDFFont_GetGlyphWidth(first.font, c as u32, rewrite.size as f32, &mut w)
                    });
                    known && (!cut_down || had.contains(&c))
                };
                let missing: Vec<char> = letters.chars().filter(|c| !settable(*c)).collect();
                if missing.is_empty() {
                    Some(first.font)
                } else {
                    let shown: String = {
                        let mut seen = Vec::new();
                        for c in missing {
                            if !seen.contains(&c) {
                                seen.push(c);
                            }
                        }
                        seen.into_iter().take(4).collect()
                    };
                    note = format!(
                        " Set in {}: its own font here has no \"{shown}\".",
                        Family::nearest(&first.run.font).name()
                    );
                    None
                }
            } else {
                None
            };
            let font = match own {
                Some(f) => f,
                None => {
                    let family = if rewrite.family == Family::Keep {
                        Family::nearest(&first.run.font)
                    } else {
                        rewrite.family
                    };
                    let name = family.standard(rewrite.bold, rewrite.italic);
                    let f = unsafe { b.FPDFText_LoadStandardFont(one.doc, name) };
                    if f.is_null() {
                        return Err(format!("{name} could not be loaded"));
                    }
                    f
                }
            };
            let size = rewrite.size.max(1.0);
            let width_of = |s: &str| -> Option<f64> {
                let mut total = 0f64;
                for c in s.chars() {
                    let mut w = 0f32;
                    let ok = b.is_true(unsafe { b.FPDFFont_GetGlyphWidth(font, c as u32, size as f32, &mut w) });
                    if !ok {
                        return None;
                    }
                    total += w as f64;
                }
                Some(total)
            };
            if let Some(bad) = letters.chars().find(|c| width_of(&c.to_string()).is_none()) {
                return Err(format!("\"{bad}\" isn't a letter the font can set. Try another font, or leave it out."));
            }
            let spaces = width_of(" ").is_some();
            let space = width_of(" ").unwrap_or(size * 0.28);
            let measure = |s: &str| -> f64 {
                if spaces {
                    width_of(s).unwrap_or(0.0)
                } else {
                    let words: Vec<&str> = s.split(' ').filter(|w| !w.is_empty()).collect();
                    words.iter().map(|w| width_of(w).unwrap_or(0.0)).sum::<f64>()
                        + space * words.len().saturating_sub(1) as f64
                }
            };
            let lines = te::wrap(&rewrite.text, rewrite.width.max(size), &measure);
            let widths: Vec<f64> = lines.iter().map(|l| measure(l)).collect();
            let spacing = if found.lines.len() > 1 {
                found.spacing * size / found.size.max(0.1)
            } else {
                size * 1.2
            };
            let starts = te::origins(found, &widths, rewrite.align, rewrite.width, spacing);

            // Out with the old words. The sheet without them, as pdfium had it,
            // is what the result has to match everywhere else.
            for i in &mine {
                unsafe { b.FPDFPage_RemoveObject(page, pieces[*i].object) };
            }
            let before = self.look_at(page).ok_or("the sheet could not be drawn")?;

            // In with the new: a line at a time, or a word at a time when the
            // font has no space to put between them.
            let mut placed: Vec<(String, [f64; 2])> = Vec::new();
            for (line, start) in lines.iter().zip(&starts) {
                if line.is_empty() {
                    continue;
                }
                let bits: Vec<(String, f64)> = if spaces {
                    vec![(line.clone(), 0.0)]
                } else {
                    let mut x = 0.0;
                    let mut out = Vec::new();
                    for word in line.split(' ').filter(|w| !w.is_empty()) {
                        out.push((word.to_string(), x));
                        x += width_of(word).unwrap_or(0.0) + space;
                    }
                    out
                };
                for (words, offset) in bits {
                    let at = [start[0] + found.along[0] * offset, start[1] + found.along[1] * offset];
                    let object = unsafe { b.FPDFPageObj_CreateTextObj(one.doc, font, size as f32) };
                    if object.is_null() {
                        return Err("pdfium would not make the new text".into());
                    }
                    unsafe {
                        b.FPDFText_SetText_str(object, &words);
                        b.FPDFPageObj_SetFillColor(
                            object,
                            rewrite.colour[0] as u32,
                            rewrite.colour[1] as u32,
                            rewrite.colour[2] as u32,
                            255,
                        );
                        let m = FS_MATRIX {
                            a: found.along[0] as f32,
                            b: found.along[1] as f32,
                            c: found.up[0] as f32,
                            d: found.up[1] as f32,
                            e: at[0] as f32,
                            f: at[1] as f32,
                        };
                        b.FPDFPageObj_SetMatrix(object, &m);
                        b.FPDFPage_InsertObject(page, object);
                    }
                    placed.push((words, at));
                }
            }
            if !b.is_true(unsafe { b.FPDFPage_GenerateContent(page) }) {
                return Err("pdfium could not write the sheet back".into());
            }
            for i in &mine {
                unsafe { b.FPDFPageObj_Destroy(pieces[*i].object) };
            }
            let bytes = self.save_incrementally(one.doc)?;

            // Read what was written, take the new words back out, and draw it.
            // Anything different from the sheet before is something this
            // would have broken, and then nothing is changed.
            let check = unsafe { b.FPDF_LoadMemDocument64(&bytes, None) };
            if check.is_null() {
                return Err("the changed sheet would not open again, so nothing was changed".into());
            }
            let verdict = (|| {
                let again = unsafe { b.FPDF_LoadPage(check, page_index as i32) };
                if again.is_null() {
                    return Err("the changed sheet would not open again, so nothing was changed".to_string());
                }
                let mut left = placed.clone();
                for p in self.pieces(again) {
                    if let Some(k) = left.iter().position(|(w, at)| {
                        p.run.text == *w && (p.run.origin[0] - at[0]).abs() < 0.6 && (p.run.origin[1] - at[1]).abs() < 0.6
                    }) {
                        left.remove(k);
                        unsafe { b.FPDFPage_RemoveObject(again, p.object) };
                    }
                }
                let after = self.look_at(again);
                unsafe { b.FPDF_ClosePage(again) };
                if !left.is_empty() {
                    return Err("the new words didn't come back as they were written, so nothing was changed".to_string());
                }
                let after = after.ok_or("the sheet could not be drawn")?;
                if !same_picture(&before, &after) {
                    return Err(
                        "Writing the words back would have changed other things on this sheet, so \
                         nothing was changed. The drawing is as it was."
                            .to_string(),
                    );
                }
                Ok(())
            })();
            unsafe { b.FPDF_CloseDocument(check) };
            verdict?;
            let now = lines.iter().filter(|l| !l.is_empty()).count();
            let was = found.lines.len();
            // Nothing below a note moves out of its way, here or in any PDF
            // editor. So a note that has grown says so, rather than leaving
            // somebody to find its last line printed over the next note.
            let grew = if now > was {
                format!(
                    " It runs to {now} lines now where it had {was}, and the new {} may sit on \
                     whatever is under it. Shorten it, or Undo.",
                    if now - was == 1 { "one" } else { "ones" }
                )
            } else {
                String::new()
            };
            let said = format!(
                "The words are changed in the drawing: {now} line{}.{note}{grew}",
                if now == 1 { "" } else { "s" }
            );
            Ok((bytes, said))
        })();
        unsafe { self.bindings.FPDF_ClosePage(page) };
        result
    }

    /// The file saved as the original with what changed after it, so the
    /// drawing as it was is still in the file.
    fn save_incrementally(&self, document: FPDF_DOCUMENT) -> Result<Vec<u8>, String> {
        #[repr(C)]
        struct Writer {
            version: std::os::raw::c_int,
            write_block: unsafe extern "C" fn(*mut Writer, *const c_void, std::os::raw::c_ulong) -> std::os::raw::c_int,
            into: *mut Vec<u8>,
        }
        unsafe extern "C" fn write_block(me: *mut Writer, data: *const c_void, size: std::os::raw::c_ulong) -> std::os::raw::c_int {
            if me.is_null() || data.is_null() || (*me).into.is_null() {
                return 0;
            }
            (*(*me).into).extend_from_slice(std::slice::from_raw_parts(data as *const u8, size as usize));
            1
        }
        let mut out: Vec<u8> = Vec::new();
        let mut writer = Writer { version: 1, write_block, into: &mut out as *mut Vec<u8> };
        let ok = unsafe {
            self.bindings.FPDF_SaveAsCopy(
                document,
                &mut writer as *mut Writer as *mut pdfium_render::prelude::FPDF_FILEWRITE,
                FPDF_INCREMENTAL as _,
            )
        };
        if !self.bindings.is_true(ok) || out.is_empty() {
            return Err("pdfium could not save the sheet".into());
        }
        Ok(out)
    }
}

/// Whether two drawings of a sheet are the same to the eye: a handful of
/// pixels may differ where anti-aliasing lands differently, and nothing more.
fn same_picture(a: &egui::ColorImage, b: &egui::ColorImage) -> bool {
    if a.size != b.size {
        return false;
    }
    let different = a
        .pixels
        .iter()
        .zip(&b.pixels)
        .filter(|(p, q)| {
            let (p, q) = (p.to_array(), q.to_array());
            (0..3).any(|k| (p[k] as i32 - q[k] as i32).abs() > 60)
        })
        .count();
    different <= (a.pixels.len() / 5000).max(12)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_that_is_not_a_pdf_is_never_called_locked() {
        // pdfium says 3 (format) for a bid file or anything else that isn't a PDF.
        let t = open_trouble(3, false);
        assert!(!t.wants_password);
        assert!(t.said.contains("not a PDF"), "{}", t.said);
        assert!(!open_trouble(1, false).wants_password);
        assert!(!open_trouble(2, false).wants_password);
        assert!(!open_trouble(5, false).wants_password);
    }

    #[test]
    fn only_a_password_error_asks_for_a_password() {
        let none = open_trouble(4, false);
        assert!(none.wants_password);
        assert_eq!(none.said, "this drawing set is locked");
        let wrong = open_trouble(4, true);
        assert!(wrong.wants_password);
        assert_eq!(wrong.said, "that password was not accepted");
    }

    const LETTER: PageSize = PageSize { width: 612.0, height: 792.0 };

    #[test]
    fn a_letter_page_at_one_to_one_has_short_tiles_on_its_right_and_bottom() {
        assert_eq!(tile_extent(LETTER, 0, 0, 0), Some((0.0, 0.0, 512, 512)));
        assert_eq!(tile_extent(LETTER, 0, 1, 0), Some((512.0, 0.0, 100, 512)));
        assert_eq!(tile_extent(LETTER, 0, 0, 1), Some((0.0, 512.0, 512, 280)));
        assert_eq!(tile_extent(LETTER, 0, 1, 1), Some((512.0, 512.0, 100, 280)));
        assert_eq!(tile_extent(LETTER, 0, 2, 0), None);
    }

    #[test]
    fn an_edge_tile_is_drawn_from_all_of_its_own_image() {
        // The bug on the Iron Workers quote: the right-hand tile's image is 100
        // pixels wide, and the view drew 100/512ths of it across the whole
        // column. Drawn whole, it is the page.
        let (_, _, w, h) = tile_extent(LETTER, 0, 1, 1).unwrap();
        assert_eq!(tile_uv((w, h), [w as usize, h as usize]), [1.0, 1.0]);
        // And a texture that really is 512 square is still drawn in part.
        let [u, v] = tile_uv((w, h), [512, 512]);
        assert!((u - 100.0 / 512.0).abs() < 1e-6 && (v - 280.0 / 512.0).abs() < 1e-6);
    }

    #[test]
    fn sheet_numbers_are_told_apart_from_ordinary_words() {
        for good in ["S2.1", "A101", "E-100", "M1.02A", "S001", "A-201"] {
            assert!(looks_like_sheet_number(good), "{good}");
        }
        for bad in [
            "SECTION",
            "1",
            "FOUNDATION PLAN",
            "2026",
            "ABCD1",
            "s2.1",
            "12'-6\"",
        ] {
            assert!(!looks_like_sheet_number(bad), "{bad}");
        }
    }

    #[test]
    fn buckets_never_upscale_by_more_than_a_hair() {
        for zoom in [0.05f32, 0.3, 0.51, 1.0, 1.05, 1.9, 4.0, 9.0] {
            let b = bucket_for_zoom(zoom);
            let s = bucket_scale(b) as f32;
            assert!(s >= zoom * 0.93, "zoom {zoom} rendered at {s}");
        }
    }

    #[test]
    fn a_zoom_just_over_a_power_of_two_does_not_quadruple_the_work() {
        assert_eq!(bucket_for_zoom(1.0), 0);
        assert_eq!(bucket_for_zoom(1.03), 0);
        assert_eq!(bucket_for_zoom(1.5), 1);
    }
}
