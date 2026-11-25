//! The tablet manifest (docs/TABLET-MANIFEST.md, schema 1): what a job shows
//! on the shop's and the field's tablets, as Fabwire sends it and the server
//! keeps it.
//!
//! The shapes, the checks every manifest must pass, and the rule for which
//! sets and files a tablet account may reach live here, in the wire crate,
//! so the server that enforces them and the app that draws them read one
//! definition. The same examples (`docs/tablet-manifest-vectors.json`) are
//! tested here and in Fabwire.

use std::collections::BTreeSet; // WHAT: sorted sets of ids. WHY: what a tablet reaches compares the same on both sides.

use serde::{Deserialize, Serialize}; // WHAT: the wire's shapes. WHY: JSON in from Fabwire, out to the tablets.

/// The schema this build reads and writes.
pub const SCHEMA: u32 = 1; // WHAT: schema 1. WHY: anything else is refused, never half-read.

/// The tiles a job can show.
pub const SECTIONS: [&str; 5] = ["drawings", "shipping", "traveler", "loading", "documents"]; // WHAT: the five sections. WHY: the only words a manifest may turn on.
/// The disciplines a sheet can be (Fabwire's document control).
pub const DISCIPLINES: [&str; 5] = [
    "anchor_bolts",
    "structural",
    "misc",
    "erection",
    "joist_deck",
]; // WHAT: the five. WHY: the Drawings filters come from them.
/// A shipping ticket's boxes, in the form's order.
pub const BOXES: [&str; 3] = ["loaded_by", "driver", "received_by"]; // WHAT: the three boxes. WHY: the ticket's signatures.
/// Where a load can stand.
pub const STATUSES: [&str; 5] = ["loading", "loaded", "in_transit", "delivered", "returned"]; // WHAT: the five. WHY: shown in words and colour.

/// The longest text any field may hold.
pub const LONGEST: usize = 200; // WHAT: 200 characters. WHY: room for any title; none for a flood.
/// The most sheets one job may list.
pub const MOST_SHEETS: usize = 5000; // WHAT: five thousand. WHY: past any job's released set.
/// The most pinned documents.
pub const MOST_DOCUMENTS: usize = 500; // WHAT: five hundred. WHY: the office pins a few; this is far past it.
/// The most loads.
pub const MOST_LOADS: usize = 200; // WHAT: two hundred. WHY: a week's loads are a handful.
/// The most piece lines on one load.
pub const MOST_PIECES: usize = 2000; // WHAT: two thousand. WHY: past any trailer.
/// The most "do this now" items.
pub const MOST_TODO: usize = 20; // WHAT: twenty. WHY: a banner shows one; the rest wait.
/// The most piece marks on one sheet.
pub const MOST_MARKS: usize = 200; // WHAT: two hundred. WHY: past any sheet's marks.

/// A job's manifest, as Fabwire sends it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)] // WHAT: a field this schema doesn't name is refused. WHY: a newer sender never has something quietly dropped
pub struct Manifest {
    pub schema: u32,        // WHAT: the schema, 1. WHY: checked first.
    pub job: Job,           // WHAT: the job's words. WHY: the cards and the "This job" box.
    pub sections: Sections, // WHAT: which tiles each audience gets. WHY: the office decides in Fabwire.
    #[serde(default)]
    pub sheets: Vec<Sheet>, // WHAT: the sheets released for fabrication. WHY: the only drawings a tablet shows.
    #[serde(default)]
    pub documents: Vec<Document>, // WHAT: what the office pinned. WHY: the Documents tile.
    #[serde(default)]
    pub loads: Vec<Load>, // WHAT: this week's loads. WHY: the Shipping tile and the sign flow.
    #[serde(default)]
    pub traveler: Option<Traveler>, // WHAT: the traveler's summary, if it has one. WHY: the Traveler tile.
    #[serde(default)]
    pub loading: Option<Loading>, // WHAT: the loading log's summary, if any. WHY: the Loading tile.
    #[serde(default)]
    pub todo: Vec<Todo>, // WHAT: "do this now", most urgent first. WHY: the banner and the job card.
}

/// The job's words.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)] // WHAT: strict. WHY: as the manifest
pub struct Job {
    pub number: String, // WHAT: the job number ("2601"). WHY: gold on every card.
    pub name: String,   // WHAT: its name. WHY: the card's title.
    #[serde(default)]
    pub town: String, // WHAT: where it is. WHY: on the card.
    #[serde(default)]
    pub gc: String, // WHAT: the general contractor. WHY: "This job".
    #[serde(default)]
    pub site: String, // WHAT: the gate or yard. WHY: "This job" and where loads go.
    #[serde(default)]
    pub office: String, // WHAT: who to call in the office. WHY: "This job".
}

/// Which sections each audience gets.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)] // WHAT: strict. WHY: as the manifest
pub struct Sections {
    #[serde(default)]
    pub shop: Vec<String>, // WHAT: the shop's sections. WHY: the office turns each on or off.
    #[serde(default)]
    pub field: Vec<String>, // WHAT: the field's. WHY: likewise.
}

/// A sheet released for fabrication.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)] // WHAT: strict. WHY: as the manifest
pub struct Sheet {
    pub set: String,   // WHAT: its drawing set on this server. WHY: what opens.
    pub sheet: String, // WHAT: the sheet number ("S-101"). WHY: the list is by number.
    #[serde(default)]
    pub title: String, // WHAT: its title. WHY: under the number.
    pub rev: String,   // WHAT: its revision. WHY: "Rev 3" on the row and the viewer.
    pub released: String, // WHAT: the day it was released, YYYY-MM-DD. WHY: released-only: a sheet without one is refused.
    pub discipline: String, // WHAT: one of DISCIPLINES. WHY: the filters.
    #[serde(default)]
    pub marks: Vec<String>, // WHAT: piece marks on it. WHY: "B12" finds its sheets.
}

/// A document the office pinned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)] // WHAT: strict. WHY: as the manifest
pub struct Document {
    pub kind: String, // WHAT: "file" or "set". WHY: which kind of id.
    pub id: String,   // WHAT: the file or set on this server. WHY: what opens.
    pub name: String, // WHAT: what it's called. WHY: the row.
    #[serde(rename = "for")]
    pub audience: Vec<String>, // WHAT: "shop", "field" or both. WHY: pinned for one or both.
}

/// A load and its ticket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)] // WHAT: strict. WHY: as the manifest
pub struct Load {
    pub number: u32, // WHAT: the load's number. WHY: "Load 2".
    #[serde(default)]
    pub ticket: String, // WHAT: the ticket's number ("2601-02"). WHY: on the ticket screen.
    pub set: String, // WHAT: the ticket's drawing set. WHY: its sign-off lines are on it, and "See the ticket as printed" opens it.
    pub status: String, // WHAT: one of STATUSES. WHY: words and colour.
    #[serde(default)]
    pub ships: String, // WHAT: the day it ships, YYYY-MM-DD or "". WHY: Today / This week.
    #[serde(default)]
    pub weight_lb: u64, // WHAT: its weight. WHY: on the card and the pad.
    #[serde(default)]
    pub capacity_lb: u64, // WHAT: the trailer's capacity. WHY: the weight bar.
    #[serde(default)]
    pub trailer: String, // WHAT: the trailer ("Flatbed 48'"). WHY: on the card.
    #[serde(default)]
    pub destination: String, // WHAT: where it goes. WHY: "Deliver to".
    #[serde(default)]
    pub pieces: Vec<Piece>, // WHAT: its piece lines. WHY: the ticket's table.
    #[serde(default)]
    pub boxes: Vec<TicketBox>, // WHAT: its signature boxes. WHY: the signatures panel.
}

/// One piece line on a ticket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)] // WHAT: strict. WHY: as the manifest
pub struct Piece {
    pub mark: String, // WHAT: the piece mark. WHY: the first column.
    pub qty: u32,     // WHAT: how many. WHY: the pieces count.
    #[serde(default)]
    pub description: String, // WHAT: what it is. WHY: the row.
    #[serde(default)]
    pub length: String, // WHAT: its length as written. WHY: the row.
    #[serde(default)]
    pub weight_lb: u64, // WHAT: the line's weight. WHY: the row.
}

/// One signature box on a ticket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)] // WHAT: strict. WHY: as the manifest
pub struct TicketBox {
    #[serde(rename = "box")]
    pub which: String, // WHAT: one of BOXES. WHY: which box.
    #[serde(default, rename = "ref")]
    pub reference: String, // WHAT: its sign-off line's reference on the ticket's set, "" once filled. WHY: the tablet signs that line.
    #[serde(default)]
    pub signer: String, // WHAT: who filled it, when filled. WHY: shown signed.
    #[serde(default)]
    pub signed: String, // WHAT: when. WHY: shown signed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<BoxLine>, // WHAT: the live line, written by the server for a tablet only. WHY: a box signed a moment ago shows signed; refused in what Fabwire sends.
}

/// The live sign-off line behind a box, as the server has it now.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)] // WHAT: strict. WHY: as the manifest
pub struct BoxLine {
    pub id: String,     // WHAT: the line's id. WHY: what signing names.
    pub status: String, // WHAT: open, signed or withdrawn. WHY: what to show.
    #[serde(default)]
    pub signer: String, // WHAT: the name written. WHY: shown.
    #[serde(default)]
    pub signed: String, // WHAT: when, RFC 3339. WHY: shown.
}

/// The traveler's summary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)] // WHAT: strict. WHY: as the manifest
pub struct Traveler {
    pub set: String, // WHAT: the traveler's drawing set. WHY: opened from the tile until phase 2.
    #[serde(default)]
    pub pieces: u32, // WHAT: pieces on it. WHY: the tile.
    #[serde(default)]
    pub complete: u32, // WHAT: pieces done. WHY: the tile.
    #[serde(default)]
    pub open_stations: u32, // WHAT: stations still to sign. WHY: the tile's chip.
}

/// The loading log's summary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)] // WHAT: strict. WHY: as the manifest
pub struct Loading {
    pub set: String, // WHAT: the loading log's set. WHY: opened from the tile until phase 2.
    #[serde(default)]
    pub load: u32, // WHAT: the load being loaded. WHY: the tile.
    #[serde(default)]
    pub weight_lb: u64, // WHAT: on it so far. WHY: the bar.
    #[serde(default)]
    pub capacity_lb: u64, // WHAT: the trailer's capacity. WHY: the bar.
    #[serde(default)]
    pub ready_pieces: u32, // WHAT: pieces ready to load. WHY: the chip.
}

/// One "do this now" item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)] // WHAT: strict. WHY: as the manifest
pub struct Todo {
    pub section: String, // WHAT: the section it opens. WHY: the banner's button.
    #[serde(default)]
    pub load: Option<u32>, // WHAT: the load it's about, if any. WHY: "Open ticket" opens that ticket.
    pub words: String, // WHAT: what to do, in words. WHY: the banner.
    #[serde(default)]
    pub detail: String, // WHAT: one more line. WHY: under the words.
}

/// Why a manifest was refused: a code from docs/TABLET-MANIFEST.md §4 and a sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid {
    pub code: &'static str, // WHAT: the stable code. WHY: both sides test the same cases by it.
    pub message: String,    // WHAT: the sentence. WHY: said to whoever reads the refusal.
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message) // WHAT: "code: sentence". WHY: the refusal's message starts with its code (§2).
    }
}

fn invalid(code: &'static str, message: impl Into<String>) -> Invalid {
    Invalid {
        code,
        message: message.into(),
    } // WHAT: one refusal. WHY: short to write below.
}

/// Text checked: at most LONGEST characters and no control characters.
fn text(what: &str, value: &str) -> Result<(), Invalid> {
    if value.chars().count() > LONGEST || value.chars().any(char::is_control) {
        return Err(invalid(
            "too_long",
            format!("{what} must be at most {LONGEST} characters, with no control characters."),
        )); // WHAT: refused. WHY: shown on a screen as typed.
    }
    Ok(())
}

/// Whether `day` reads YYYY-MM-DD with a real month and day.
#[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
pub fn is_day(day: &str) -> bool {
    let b = day.as_bytes(); // WHAT: its bytes. WHY: checked by place.
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false; // WHAT: not the shape. WHY: "11/20/2026" is not a day here.
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit); // WHAT: a run all digits. WHY: each part.
    if !(digits(0..4) && digits(5..7) && digits(8..10)) {
        return false; // WHAT: letters in it. WHY: not a day.
    }
    let month: u32 = day[5..7].parse().unwrap_or(0); // WHAT: the month. WHY: 1 to 12.
    let date: u32 = day[8..10].parse().unwrap_or(0); // WHAT: the day of the month. WHY: 1 to 31.
    (1..=12).contains(&month) && (1..=31).contains(&date) // WHAT: in range. WHY: enough to refuse nonsense; the calendar is Fabwire's.
}

/// Checks a manifest against docs/TABLET-MANIFEST.md §4 (the parts that need
/// no server: ids are the server's to check).
///
/// # Errors
///
/// The first rule it breaks, with its code.
#[allow(clippy::too_many_lines)] // WHAT: one function for every rule. WHY: the contract's §4 read top to bottom in one place
pub fn validate(m: &Manifest) -> Result<(), Invalid> {
    if m.schema != SCHEMA {
        return Err(invalid(
            "schema",
            format!(
                "This server reads tablet manifest schema {SCHEMA}, not {}.",
                m.schema
            ),
        )); // WHAT: refused. WHY: never half-read.
    }
    for (what, value) in [
        ("The job's number", &m.job.number),
        ("The job's name", &m.job.name),
        ("The town", &m.job.town),
        ("The GC", &m.job.gc),
        ("The site", &m.job.site),
        ("The office contact", &m.job.office),
    ] {
        text(what, value)?; // WHAT: each of the job's words. WHY: shown on the cards.
    }
    for section in m.sections.shop.iter().chain(&m.sections.field) {
        if !SECTIONS.contains(&section.as_str()) {
            return Err(invalid(
                "bad_section",
                format!("\"{section}\" is not a tablet section."),
            )); // WHAT: refused. WHY: only the five exist.
        }
    }
    if m.sheets.len() > MOST_SHEETS
        || m.documents.len() > MOST_DOCUMENTS
        || m.loads.len() > MOST_LOADS
        || m.todo.len() > MOST_TODO
    {
        return Err(invalid(
            "too_many",
            "A tablet manifest lists more than it may.",
        )); // WHAT: refused. WHY: past any real job.
    }
    for sheet in &m.sheets {
        if !is_day(&sheet.released) {
            return Err(invalid("not_released", format!("Sheet {} has no release date: only sheets released for fabrication go to the tablets.", sheet.sheet)));
            // WHAT: refused. WHY: the released-only rule.
        }
        if !DISCIPLINES.contains(&sheet.discipline.as_str()) {
            return Err(invalid(
                "bad_discipline",
                format!("\"{}\" is not a discipline.", sheet.discipline),
            )); // WHAT: refused. WHY: the filters know five.
        }
        if sheet.marks.len() > MOST_MARKS {
            return Err(invalid(
                "too_many",
                format!("Sheet {} lists more marks than it may.", sheet.sheet),
            )); // WHAT: refused. WHY: past any sheet.
        }
        for (what, value) in [
            ("A set id", &sheet.set),
            ("A sheet number", &sheet.sheet),
            ("A sheet title", &sheet.title),
            ("A revision", &sheet.rev),
        ] {
            text(what, value)?; // WHAT: each word. WHY: shown.
        }
        for mark in &sheet.marks {
            text("A piece mark", mark)?; // WHAT: each mark. WHY: searched.
        }
    }
    for doc in &m.documents {
        if doc.kind != "file" && doc.kind != "set" {
            return Err(invalid(
                "unknown_field",
                format!("\"{}\" is not a kind of document.", doc.kind),
            )); // WHAT: refused. WHY: a file or a set only.
        }
        if doc.audience.is_empty() || doc.audience.iter().any(|a| a != "shop" && a != "field") {
            return Err(invalid(
                "bad_audience",
                format!(
                    "\"{}\" must be pinned for the shop, the field or both.",
                    doc.name
                ),
            )); // WHAT: refused. WHY: nobody else has a tablet.
        }
        text("A document's id", &doc.id)?; // WHAT: its id. WHY: kept.
        text("A document's name", &doc.name)?; // WHAT: its name. WHY: shown.
    }
    for load in &m.loads {
        if !STATUSES.contains(&load.status.as_str()) {
            return Err(invalid(
                "bad_status",
                format!("\"{}\" is not where a load can stand.", load.status),
            )); // WHAT: refused. WHY: shown in words.
        }
        if load.pieces.len() > MOST_PIECES {
            return Err(invalid(
                "too_many",
                format!("Load {} lists more pieces than it may.", load.number),
            )); // WHAT: refused. WHY: past any trailer.
        }
        for (what, value) in [
            ("A ticket number", &load.ticket),
            ("A ticket's set", &load.set),
            ("A ship day", &load.ships),
            ("A trailer", &load.trailer),
            ("A destination", &load.destination),
        ] {
            text(what, value)?; // WHAT: each word. WHY: shown.
        }
        for piece in &load.pieces {
            for (what, value) in [
                ("A piece mark", &piece.mark),
                ("A description", &piece.description),
                ("A length", &piece.length),
            ] {
                text(what, value)?; // WHAT: each word. WHY: shown.
            }
        }
        let mut seen = BTreeSet::new(); // WHAT: boxes named so far. WHY: each once.
        for b in &load.boxes {
            if !BOXES.contains(&b.which.as_str()) || !seen.insert(b.which.as_str()) {
                return Err(invalid(
                    "bad_box",
                    format!(
                        "Load {} names box \"{}\" that isn't on a ticket, or twice.",
                        load.number, b.which
                    ),
                )); // WHAT: refused. WHY: three boxes, once each.
            }
            if b.line.is_some() {
                return Err(invalid(
                    "unknown_field",
                    "A box's live line is written by this server, never sent to it.",
                )); // WHAT: refused. WHY: only the server says what is signed.
            }
            for (what, value) in [
                ("A box's reference", &b.reference),
                ("A signer", &b.signer),
                ("A signed day", &b.signed),
            ] {
                text(what, value)?; // WHAT: each word. WHY: shown or matched.
            }
        }
    }
    for item in &m.todo {
        if !SECTIONS.contains(&item.section.as_str()) {
            return Err(invalid(
                "bad_section",
                format!("\"{}\" is not a tablet section.", item.section),
            )); // WHAT: refused. WHY: the banner opens a section.
        }
        text("A to-do's words", &item.words)?; // WHAT: its words. WHY: shown.
        text("A to-do's detail", &item.detail)?; // WHAT: its detail. WHY: shown.
    }
    for set in [
        m.traveler.as_ref().map(|t| &t.set),
        m.loading.as_ref().map(|l| &l.set),
    ]
    .into_iter()
    .flatten()
    {
        text("A set id", set)?; // WHAT: the traveler's and loading log's ids. WHY: kept.
    }
    Ok(())
}

/// Which tablet audiences a person is in: their groups named Shop and Field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Audience {
    pub shop: bool,  // WHAT: in a group called Shop. WHY: gets the shop's sections.
    pub field: bool, // WHAT: in a group called Field. WHY: gets the field's.
}

impl Audience {
    /// The audience of somebody in `groups` (names, case aside).
    #[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
    pub fn of<S: AsRef<str>>(groups: &[S]) -> Audience {
        let named = |n: &str| {
            groups
                .iter()
                .any(|g| g.as_ref().trim().eq_ignore_ascii_case(n))
        }; // WHAT: a group by name. WHY: case aside, as folder rules compare them.
        Audience {
            shop: named("shop"),
            field: named("field"),
        } // WHAT: both checked. WHY: somebody can be in both.
    }

    /// Whether a section is on for this audience. Drawings is on for anyone in either.
    #[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
    pub fn sees_section(self, m: &Manifest, section: &str) -> bool {
        let on = |list: &[String]| section == "drawings" || list.iter().any(|s| s == section); // WHAT: on in a list, drawings always. WHY: "AISC, always".
        (self.shop && on(&m.sections.shop)) || (self.field && on(&m.sections.field))
        // WHAT: for either audience they're in. WHY: both count.
    }

    /// Whether a pinned document is for this audience, its section on.
    #[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
    pub fn sees_document(self, m: &Manifest, doc: &Document) -> bool {
        let pinned = |who: &str| doc.audience.iter().any(|a| a == who); // WHAT: pinned for that audience. WHY: below.
        (self.shop && pinned("shop") && m.sections.shop.iter().any(|s| s == "documents"))
            || (self.field && pinned("field") && m.sections.field.iter().any(|s| s == "documents"))
        // WHAT: pinned for them and Documents on for them. WHY: both must hold for the same audience.
    }
}

/// Whether a manifest holds nothing for any tablet: the one Fabwire pushes when a job leaves active.
#[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
pub fn is_empty(m: &Manifest) -> bool {
    m.sections.shop.is_empty()
        && m.sections.field.is_empty()
        && m.sheets.is_empty()
        && m.documents.is_empty()
        && m.loads.is_empty()
        && m.traveler.is_none()
        && m.loading.is_none()
        && m.todo.is_empty() // WHAT: nothing in any part. WHY: a tablet has nothing to show of the job, so it drops off the job list
}

/// Whether somebody in `audience` (the office is both) may sign a ticket's box of kind `role`.
/// Received by and Driver are signed on site, by the field or the office; Loaded by by the shop, the field or the office.
/// A line with no kind (asked before kinds existed) is open to anyone who reaches its set, as it always was.
#[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
pub fn may_sign(role: &str, audience: Audience) -> bool {
    match role {
        "loaded_by" => audience.shop || audience.field, // WHAT: the shop, the field or the office. WHY: the owner's rule: loading is the shop's, and the field may load too
        "driver" | "received_by" => audience.field, // WHAT: the field or the office. WHY: the owner's rule: only people on site take a delivery or sign for the driver
        _ => true, // WHAT: no kind, or one that isn't a ticket box. WHY: the old rule (anyone who reaches the set) stands for lines that never had one
    }
}

/// What a tablet account in `audience` may reach of a job (§5), before the
/// folder rules: the sets and the files, sorted.
#[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
pub fn reaches(m: &Manifest, audience: Audience) -> (BTreeSet<String>, BTreeSet<String>) {
    let (mut sets, mut files) = (BTreeSet::new(), BTreeSet::new()); // WHAT: what they reach. WHY: built below.
    if !audience.shop && !audience.field {
        return (sets, files); // WHAT: nothing. WHY: a tablet account in neither group reaches nothing of a job with a manifest.
    }
    sets.extend(m.sheets.iter().map(|s| s.set.clone())); // WHAT: every released sheet. WHY: Drawings is always on.
    if audience.sees_section(m, "shipping") {
        sets.extend(m.loads.iter().map(|l| l.set.clone())); // WHAT: the tickets. WHY: Shipping on.
    }
    if let Some(t) = m
        .traveler
        .as_ref()
        .filter(|_| audience.sees_section(m, "traveler"))
    {
        sets.insert(t.set.clone()); // WHAT: the traveler. WHY: Traveler on.
    }
    if let Some(l) = m
        .loading
        .as_ref()
        .filter(|_| audience.sees_section(m, "loading"))
    {
        sets.insert(l.set.clone()); // WHAT: the loading log. WHY: Loading on.
    }
    for doc in m.documents.iter().filter(|d| audience.sees_document(m, d)) {
        if doc.kind == "set" {
            sets.insert(doc.id.clone()); // WHAT: a pinned set. WHY: pinned for them.
        } else {
            files.insert(doc.id.clone()); // WHAT: a pinned file. WHY: likewise.
        }
    }
    (sets, files)
}

/// A job as the tablet's job list shows it (`GET /tablet`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabletJob {
    pub project: String, // WHAT: the project's id. WHY: opened by it.
    pub number: String,  // WHAT: the job number. WHY: gold on the card.
    pub name: String,    // WHAT: its name. WHY: the card's title.
    #[serde(default)]
    pub town: String, // WHAT: where. WHY: on the card.
    #[serde(default)]
    pub shop: bool, // WHAT: shown to them as the shop. WHY: "Shop" on the card.
    #[serde(default)]
    pub field: bool, // WHAT: as the field. WHY: "Field" on the card.
    #[serde(default)]
    pub todo: Vec<String>, // WHAT: what needs doing, in words. WHY: under the name; empty is "Nothing to do".
    #[serde(default)]
    pub updated: String, // WHAT: when the office last changed it. WHY: "Up to date with the office".
}

/// What `PUT /projects/{id}/tablet` answers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabletSaved {
    pub project: String, // WHAT: the project. WHY: said back.
    pub version: i64, // WHAT: how many times it has changed. WHY: a program sees its push counted.
    pub changed: bool, // WHAT: whether this push changed it. WHY: the same manifest again changes nothing.
}

/// What the server answers for one job's manifest (`GET /projects/{id}/tablet`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabletView {
    pub project: String, // WHAT: the project. WHY: said back.
    pub version: i64,    // WHAT: its version. WHY: the tablet redraws only on a change.
    pub updated: String, // WHAT: when the office last changed it. WHY: shown.
    #[serde(default)]
    pub shop: bool, // WHAT: this person as the shop. WHY: the job card.
    #[serde(default)]
    pub field: bool, // WHAT: as the field. WHY: likewise.
    pub manifest: Manifest, // WHAT: the manifest, cut to what this person reaches. WHY: everything the tablet draws.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> serde_json::Value {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/tablet-manifest-vectors.json"); // WHAT: the shared file. WHY: Fabwire tests the same one
        serde_json::from_str(
            &std::fs::read_to_string(path).expect("docs/tablet-manifest-vectors.json is there"),
        )
        .expect("it is JSON") // WHAT: read. WHY: every case below comes from it
    }

    #[test]
    fn every_valid_example_reads_passes_and_writes_back_the_same() {
        let all = vectors(); // WHAT: the examples. WHY: the contract
        for case in all["valid"].as_array().unwrap() {
            let m: Manifest = serde_json::from_value(case["manifest"].clone()).expect("reads"); // WHAT: read. WHY: every valid one must
            validate(&m).unwrap_or_else(|e| panic!("{}: {e}", case["name"])); // WHAT: passes. WHY: valid
            assert_eq!(
                serde_json::to_value(&m).unwrap(),
                case["manifest"],
                "{} writes back unchanged",
                case["name"]
            ); // WHAT: written back as JSON. WHY: nothing lost or added
        }
    }

    #[test]
    fn every_invalid_example_is_refused_with_its_code() {
        let all = vectors(); // WHAT: the examples. WHY: the contract
        for case in all["invalid"].as_array().unwrap() {
            let code = match serde_json::from_value::<Manifest>(case["manifest"].clone()) {
                Err(_) => "unknown_field", // WHAT: it didn't read. WHY: an unknown or mistyped field is refused as it is read
                Ok(m) => validate(&m).err().map_or("accepted", |e| e.code), // WHAT: the validator's code. WHY: compared below
            };
            assert_eq!(code, case["code"].as_str().unwrap(), "{}", case["name"]);
            // WHAT: the same code. WHY: both sides refuse alike
        }
    }

    #[test]
    fn what_each_audience_reaches_is_as_the_examples_say() {
        let all = vectors(); // WHAT: the examples. WHY: the contract
        for case in all["reaches"].as_array().unwrap() {
            let name = case["manifest"].as_str().unwrap(); // WHAT: which manifest. WHY: looked up
            let m: Manifest = serde_json::from_value(
                all["valid"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|v| v["name"] == name)
                    .unwrap()["manifest"]
                    .clone(),
            )
            .unwrap(); // WHAT: it. WHY: read
            let audience = match case["audience"].as_str().unwrap() {
                "shop" => Audience {
                    shop: true,
                    field: false,
                },
                "field" => Audience {
                    shop: false,
                    field: true,
                },
                "both" => Audience {
                    shop: true,
                    field: true,
                },
                _ => Audience::default(),
            }; // WHAT: the audience. WHY: each of four
            let (sets, files) = reaches(&m, audience); // WHAT: what they reach. WHY: compared
            let want = |k: &str| {
                case[k]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect::<BTreeSet<_>>()
            }; // WHAT: the example's list. WHY: compared
            assert_eq!(sets, want("sets"), "{name} {}", case["audience"]); // WHAT: the same sets. WHY: the rule agrees
            assert_eq!(files, want("files"), "{name} {}", case["audience"]); // WHAT: the same files. WHY: likewise
        }
    }

    #[test]
    fn a_live_line_sent_in_is_refused() {
        let all = vectors(); // WHAT: the examples. WHY: a valid one changed
        let mut m: Manifest = serde_json::from_value(all["valid"][0]["manifest"].clone()).unwrap(); // WHAT: the steel job. WHY: has loads
        m.loads[0].boxes[2].line = Some(BoxLine {
            id: "x".into(),
            status: "signed".into(),
            signer: "x".into(),
            signed: String::new(),
        }); // WHAT: a sender claiming a box signed. WHY: only the server says so
        assert_eq!(validate(&m).unwrap_err().code, "unknown_field"); // WHAT: refused. WHY: as above
    }

    #[test]
    fn audiences_come_from_the_shop_and_field_groups_case_aside() {
        assert_eq!(
            Audience::of(&["SHOP", "Welders"]),
            Audience {
                shop: true,
                field: false
            }
        ); // WHAT: Shop only. WHY: case aside
        assert_eq!(
            Audience::of(&[" field ", "shop"]),
            Audience {
                shop: true,
                field: true
            }
        ); // WHAT: both. WHY: spaces aside
        assert_eq!(Audience::of::<&str>(&[]), Audience::default()); // WHAT: neither. WHY: no groups
    }
}
