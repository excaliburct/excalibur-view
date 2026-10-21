//! The contract between Hyperview and a plugin.
//!
//! A plugin is a WebAssembly module. Hyperview runs it in a sandbox: it can
//! not open files, reach the network, see the screen or touch the drawing.
//! It is handed a description of the sheets and markups as JSON — the words
//! printed on the sheet, its line-work, its scale, the takeoff — and it hands
//! back what it found and what it suggests, also as JSON. Nothing a plugin
//! says changes the drawing until somebody clicks the fix it offers, and a
//! fix is one of the [`Action`]s below, carried out by Hyperview itself.
//!
//! That shape is deliberate. It is what lets one office run its own tools
//! without those tools ever being in anybody else's copy of the program, and
//! without a plugin being able to do anything worse than be wrong.
//!
//! # The exports
//!
//! A module exports its `memory` and three functions:
//!
//! - `hv_alloc(len: i32) -> i32` — room for `len` bytes, for Hyperview to
//!   write the input into.
//! - `hv_manifest() -> i64` — the [`Manifest`] as JSON.
//! - `hv_run(ptr: i32, len: i32) -> i64` — runs one command. The input at
//!   `ptr` is an [`Input`]; the answer is an [`Answer`].
//!
//! The two that answer return the answer's address in the high 32 bits and
//! its length in the low 32. With the `guest` feature, [`plugin!`] writes all
//! of this.
//!
//! # Coordinates
//!
//! Everything on a sheet is in **sheet space**: points (1/72 inch of paper),
//! measured right and down from the top left corner of the sheet as it is
//! shown on screen. Real lengths are in feet and real areas in square feet,
//! whatever units the sheet is set up in.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The version of this contract. A plugin built for a later one than the
/// program knows is refused with a reason rather than run and misread.
pub const ABI: u32 = 1;

// ---- what a plugin says about itself ----------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Manifest {
    /// Short and permanent: `acme-estimating`. Letters, digits and dashes.
    pub id: String,
    /// What the Plugins menu calls it.
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "one")]
    pub abi: u32,
    pub commands: Vec<Command>,
    /// Things the person running it can set, shown above the results and
    /// handed back in [`Input::settings`].
    #[serde(default)]
    pub settings: Vec<Setting>,
}

fn one() -> u32 {
    1
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Command {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub scope: Scope,
    /// What it needs read off the sheets. Reading the line-work of a big set
    /// takes a while, so a command that only needs words says so.
    #[serde(default)]
    pub needs: Needs,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// The sheet on screen.
    #[default]
    Sheet,
    /// Every sheet in the drawing.
    Drawing,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Needs {
    #[serde(default)]
    pub words: bool,
    #[serde(default)]
    pub lines: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Setting {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub help: String,
    pub kind: SettingKind,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SettingKind {
    Number {
        default: f64,
        #[serde(default)]
        min: Option<f64>,
        #[serde(default)]
        max: Option<f64>,
        #[serde(default)]
        unit: String,
    },
    Text {
        #[serde(default)]
        default: String,
    },
    Toggle {
        #[serde(default)]
        default: bool,
    },
}

impl Setting {
    #[must_use] // WHAT: a caller that drops the value gets a warning. WHY: asking for a default and ignoring it is always a mistake.
    pub fn default_value(&self) -> serde_json::Value {
        match &self.kind {
            SettingKind::Number { default, .. } => serde_json::json!(default),
            SettingKind::Text { default } => serde_json::json!(default),
            SettingKind::Toggle { default } => serde_json::json!(default),
        }
    }
}

// ---- what a plugin is handed ------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Input {
    /// Which of the manifest's commands to run.
    pub command: String,
    /// The drawing's file name, for titles.
    #[serde(default)]
    pub drawing: String,
    /// The sheet on screen when the command was run.
    #[serde(default)]
    pub current_page: u32,
    #[serde(default)]
    pub sheets: Vec<Sheet>,
    #[serde(default)]
    pub markups: Vec<Markup>,
    /// By setting id. A setting the person has not touched has its default.
    #[serde(default)]
    pub settings: BTreeMap<String, serde_json::Value>,
    /// The office's custom columns, by position: "LBS Per FT" and the rest.
    #[serde(default)]
    pub columns: Vec<String>,
}

impl Input {
    #[must_use] // WHAT: a caller that drops the number gets a warning. WHY: reading a setting has no other effect.
    pub fn number(&self, setting: &str, otherwise: f64) -> f64 {
        self.settings
            .get(setting)
            .and_then(serde_json::Value::as_f64) // WHAT: the setting as a number, if it is one. WHY: names the method straight, no closure in between.
            .unwrap_or(otherwise)
    }

    #[must_use] // WHAT: a caller that drops the answer gets a warning. WHY: reading a setting has no other effect.
    pub fn toggle(&self, setting: &str, otherwise: bool) -> bool {
        self.settings
            .get(setting)
            .and_then(serde_json::Value::as_bool) // WHAT: the setting as on or off, if it is one. WHY: names the method straight, no closure in between.
            .unwrap_or(otherwise)
    }

    #[must_use] // WHAT: a caller that drops the text gets a warning. WHY: reading a setting has no other effect.
    pub fn text(&self, setting: &str) -> String {
        self.settings
            .get(setting)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    }

    #[must_use] // WHAT: a caller that drops the sheet gets a warning. WHY: looking a sheet up has no other effect.
    pub fn sheet(&self, page: u32) -> Option<&Sheet> {
        self.sheets.iter().find(|s| s.page == page)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Sheet {
    /// Zero-based.
    pub page: u32,
    /// "S-201 Second Floor Framing Plan", or "Page 4" when it has no label.
    pub name: String,
    /// In points.
    pub width: f64,
    pub height: f64,
    /// The scale set on the sheet, if one is.
    #[serde(default)]
    pub scale: Option<Scale>,
    #[serde(default)]
    pub words: Vec<Word>,
    #[serde(default)]
    pub lines: Vec<Line>,
    /// True when the line-work was cut short because the sheet had more of
    /// it than is sensible to hand over.
    #[serde(default)]
    pub lines_cut_short: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Scale {
    /// Real length per length of paper: 48 for 1/4" = 1'-0", 50 for 1:50.
    pub ratio: f64,
    /// As the sheet says it: `1/4" = 1'-0"`, or `calibrated (2.1% off 1/4" = 1'-0")`.
    #[serde(default)]
    pub text: String,
}

impl Scale {
    /// Feet of building per point of paper.
    #[must_use] // WHAT: a caller that drops the figure gets a warning. WHY: working it out has no other effect.
    pub fn feet_per_point(&self) -> f64 {
        self.ratio / 72.0 / 12.0
    }
}

/// A run of text as it is printed on the sheet.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    /// `[left, top, right, bottom]` in sheet space.
    pub area: [f64; 4],
    /// How tall the characters are, in points.
    #[serde(default)]
    pub size: f64,
}

/// A straight piece of the sheet's drawing.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Line {
    pub a: [f64; 2],
    pub b: [f64; 2],
    /// Pen width on the sheet, in points. A heavy line on a framing plan is
    /// usually a member; a hairline is usually a dimension or a leader.
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub dashed: bool,
}

impl Line {
    #[must_use] // WHAT: a caller that drops the length gets a warning. WHY: measuring has no other effect.
    pub fn length(&self) -> f64 {
        ((self.b[0] - self.a[0]).powi(2) + (self.b[1] - self.a[1]).powi(2)).sqrt()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Markup {
    /// Stable for the markup's life, and what an [`Action`] names it by.
    pub id: String,
    pub page: u32,
    /// `length`, `polylength`, `area`, `volume`, `count`, `diameter`,
    /// `radius`, `angle`, or `markup` for one that measures nothing.
    pub kind: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub comments: String,
    #[serde(default)]
    pub author: String,
    /// The shape, in sheet space.
    #[serde(default)]
    pub points: Vec<[f64; 2]>,
    /// One piece's length in feet, up the slope if it has one. None when the
    /// sheet has no scale.
    #[serde(default)]
    pub length: Option<f64>,
    /// Square feet.
    #[serde(default)]
    pub area: Option<f64>,
    #[serde(default)]
    pub count: f64,
    /// How many pieces this one markup stands for.
    #[serde(default = "one_f")]
    pub quantity: f64,
    /// Rise over run, when it has a pitch.
    #[serde(default)]
    pub slope: Option<[f64; 2]>,
    /// The custom columns, in the order [`Input::columns`] names them.
    #[serde(default)]
    pub columns: Vec<String>,
    /// For all its pieces, from the unit weight in its columns.
    #[serde(default)]
    pub pounds: Option<f64>,
}

fn one_f() -> f64 {
    1.0
}

// ---- how the input travels ----------------------------------------------------

/// The first bytes of a packed input.
const PACKED: &[u8; 4] = b"HVIN";

/// The input as it is handed over: JSON for everything but each sheet's words
/// and lines, which follow it packed as numbers. A plugin runs in an
/// interpreter, and an interpreter reads a number from four bytes about
/// twenty times faster than from text — which, for a sheet with a hundred
/// thousand lines on it, is the difference between a moment and a wait.
///
/// An input too big to count in the packed form's four-byte lengths goes as
/// plain JSON instead, words and lines and all, which [`unpack`] reads as it
/// stands: slower, but whole.
#[must_use] // WHAT: a caller that drops the bytes gets a warning. WHY: packing has no other effect.
pub fn pack(input: &Input) -> Vec<u8> {
    // WHAT: the packed form, or plain JSON when a count does not fit. WHY: a length that doesn't fit four bytes must never be cut down, or the plugin would read the rest of the input as garbage.
    packed(input).unwrap_or_else(|| serde_json::to_vec(input).unwrap_or_default())
}

/// [`pack`]'s packed form, or `None` when a length does not fit in the four
/// bytes the form gives it.
fn packed(input: &Input) -> Option<Vec<u8>> {
    let light = Input {
        command: input.command.clone(),
        drawing: input.drawing.clone(),
        current_page: input.current_page,
        sheets: input
            .sheets
            .iter()
            .map(|s| Sheet {
                page: s.page,
                name: s.name.clone(),
                width: s.width,
                height: s.height,
                scale: s.scale.clone(),
                words: Vec::new(),
                lines: Vec::new(),
                lines_cut_short: s.lines_cut_short,
            })
            .collect(),
        markups: input.markups.clone(),
        settings: input.settings.clone(),
        columns: input.columns.clone(),
    };
    let json = serde_json::to_vec(&light).unwrap_or_default();
    let bulk: usize = input
        .sheets
        .iter()
        .map(|s| 8 + s.words.iter().map(|w| 24 + w.text.len()).sum::<usize>() + s.lines.len() * 21)
        .sum();
    let mut out = Vec::with_capacity(8 + json.len() + bulk);
    out.extend_from_slice(PACKED);
    // WHAT: a length written as four bytes, or None when it is past what four bytes hold. WHY: a checked conversion, so a length is never silently cut down.
    let count = |out: &mut Vec<u8>, n: usize| -> Option<()> {
        out.extend_from_slice(&u32::try_from(n).ok()?.to_le_bytes()); // WHAT: the length, little-endian. WHY: the order unpack reads it in.
        Some(()) // WHAT: it fitted. WHY: the caller goes on.
    };
    count(&mut out, json.len())?; // WHAT: how long the JSON part is. WHY: unpack takes exactly that many bytes as JSON.
    out.extend_from_slice(&json);
    let f = |out: &mut Vec<u8>, v: f64| out.extend_from_slice(&narrow(v).to_le_bytes()); // WHAT: one number as four bytes. WHY: the packed form carries numbers as f32; narrow rounds to the nearest one rather than chopping.
    for sheet in &input.sheets {
        count(&mut out, sheet.words.len())?; // WHAT: how many words the sheet has. WHY: unpack reads that many.
        for w in &sheet.words {
            for v in w.area {
                f(&mut out, v);
            }
            f(&mut out, w.size);
            count(&mut out, w.text.len())?; // WHAT: how many bytes the word's text is. WHY: unpack takes that many as its text.
            out.extend_from_slice(w.text.as_bytes());
        }
        count(&mut out, sheet.lines.len())?; // WHAT: how many lines the sheet has. WHY: unpack reads that many.
        for l in &sheet.lines {
            for v in [l.a[0], l.a[1], l.b[0], l.b[1], l.width] {
                f(&mut out, v);
            }
            out.push(u8::from(l.dashed)); // WHAT: dashed as one byte, 1 or 0. WHY: From says it can't go wrong, where `as` would not.
        }
    }
    Some(out) // WHAT: the packed input. WHY: every length fitted.
}

/// The nearest f32 to `v`, ties to even, the way the hardware rounds. A
/// finite number past f32's range comes out as f32's largest of that sign,
/// not as infinity: a point off the edge of everything is still a point.
/// Infinity stays infinity, and not-a-number stays not-a-number.
fn narrow(v: f64) -> f32 {
    if v.is_nan() { // WHAT: not-a-number. WHY: it has no bits worth rounding.
        return f32::NAN; // WHAT: not-a-number in, not-a-number out. WHY: there is nothing nearer to round it to.
    }
    let bits = v.to_bits(); // WHAT: the number's 64 bits. WHY: it is rounded by its parts, with no lossy cast anywhere.
    let sign = if bits >> 63 == 1 { 0x8000_0000u32 } else { 0 }; // WHAT: the sign bit, in f32's place. WHY: rounding works on the size; the sign goes back on at the end.
    let field = (bits >> 52) & 0x7ff; // WHAT: the 11-bit exponent field. WHY: says how big the number is.
    if field == 0x7ff { // WHAT: the exponent field all ones. WHY: that is infinity or not-a-number, never an ordinary number.
        return f32::from_bits(sign | f32::INFINITY.to_bits()); // WHAT: infinity keeps its sign. WHY: NaN was handled above, so all-ones here is infinity.
    }
    if field == 0 { // WHAT: the exponent field all zeros. WHY: zero, or an f64 subnormal, far below the smallest f32.
        return f32::from_bits(sign); // WHAT: zero, or an f64 too tiny for any f32. WHY: below about 1e-308 the nearest f32 is zero.
    }
    let exp = i64::try_from(field).unwrap_or(0) - 1023; // WHAT: the power of two, unbiased. WHY: an 11-bit field always fits; the fallback is never taken.
    let sig = (1u64 << 52) | (bits & ((1u64 << 52) - 1)); // WHAT: the 53-bit significand with its leading one. WHY: what gets rounded down to f32's 24 bits.
    // WHAT: where the f32 exponent starts, and how many low bits to round away. WHY: a normal f32 keeps 24 bits; below f32's smallest normal it keeps fewer, the way subnormals work.
    let (base, shift) = if exp >= -126 { (u64::try_from(exp + 126).unwrap_or(u64::MAX) << 23, 29) } else { (0, u32::try_from(-97 - exp).unwrap_or(63).min(63)) };
    if exp > 127 { // WHAT: past f32's largest power of two. WHY: no f32 is that big.
        return f32::from_bits(sign | f32::MAX.to_bits()); // WHAT: too big for an f32: its largest instead. WHY: a coordinate turned to infinity would break every sum it touches.
    }
    let kept = sig >> shift; // WHAT: the bits that stay. WHY: the f32's significand before rounding.
    let rest = sig & ((1u64 << shift) - 1); // WHAT: the bits that go. WHY: decide which way to round.
    let half = 1u64 << (shift - 1); // WHAT: exactly half of the last kept bit. WHY: the line between rounding down and up.
    let up = rest > half || (rest == half && kept & 1 == 1); // WHAT: whether to round up. WHY: nearest, ties to even, as IEEE 754 says.
    let total = base + kept + u64::from(up); // WHAT: the f32's bits without the sign. WHY: a carry out of the significand bumps the exponent by itself.
    match u32::try_from(total) { // WHAT: the bits as a u32, checked. WHY: f32::from_bits takes a u32, and the check doubles as the overflow test.
        Ok(b) if b < f32::INFINITY.to_bits() => f32::from_bits(sign | b), // WHAT: the rounded number. WHY: it is in range.
        _ => f32::from_bits(sign | f32::MAX.to_bits()), // WHAT: it rounded up past the largest f32: the largest instead. WHY: the same as too big above.
    }
}

/// The other way round. Plain JSON is read as it stands.
///
/// # Errors
///
/// Why the input could not be read: it is cut short, or its JSON does not
/// read.
pub fn unpack(bytes: &[u8]) -> Result<Input, String> {
    if !bytes.starts_with(PACKED) {
        return serde_json::from_slice(bytes).map_err(|e| e.to_string());
    }
    let mut at = 4usize;
    let short = || "the input is cut short".to_string();
    let take = |at: &mut usize, n: usize| -> Result<&[u8], String> {
        let slice = bytes.get(*at..*at + n).ok_or_else(short)?;
        *at += n;
        Ok(slice)
    };
    let u32_at = |at: &mut usize| -> Result<u32, String> {
        let b = take(at, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let f32_at = |at: &mut usize| -> Result<f64, String> {
        let b = take(at, 4)?;
        Ok(f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))) // WHAT: the four bytes as a number, widened to f64. WHY: From says it can never lose anything, where `as` would not.
    };
    let json_len = u32_at(&mut at)? as usize;
    let mut input: Input = serde_json::from_slice(take(&mut at, json_len)?).map_err(|e| e.to_string())?;
    for sheet in &mut input.sheets { // WHAT: each sheet, to fill in its words and lines. WHY: the same loop, written the plain way.
        let words = u32_at(&mut at)? as usize;
        sheet.words.reserve(words);
        for _ in 0..words {
            let area = [f32_at(&mut at)?, f32_at(&mut at)?, f32_at(&mut at)?, f32_at(&mut at)?];
            let size = f32_at(&mut at)?;
            let len = u32_at(&mut at)? as usize;
            let text = String::from_utf8_lossy(take(&mut at, len)?).into_owned();
            sheet.words.push(Word { text, area, size });
        }
        let lines = u32_at(&mut at)? as usize;
        sheet.lines.reserve(lines);
        for _ in 0..lines {
            let (ax, ay, bx, by, width) = (f32_at(&mut at)?, f32_at(&mut at)?, f32_at(&mut at)?, f32_at(&mut at)?, f32_at(&mut at)?);
            let dashed = take(&mut at, 1)?[0] != 0;
            sheet.lines.push(Line { a: [ax, ay], b: [bx, by], width, dashed });
        }
    }
    Ok(input)
}

// ---- what a plugin hands back -----------------------------------------------

/// What `hv_run` returns: the findings, or the reason there are none.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    Done(Output),
    Failed(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Output {
    pub title: String,
    /// A sentence or two at the top.
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub tables: Vec<Table>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Worth knowing.
    Note,
    /// Worth a look before the number goes out.
    Check,
    /// The number is wrong until this is dealt with.
    Problem,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Finding {
    pub level: Level,
    pub message: String,
    #[serde(default)]
    pub page: Option<u32>,
    /// Where on the sheet, `[left, top, right, bottom]` in sheet space.
    /// Hyperview outlines it while the results are open.
    #[serde(default)]
    pub area: Option<[f64; 4]>,
    /// Markups it is about, by id.
    #[serde(default)]
    pub markups: Vec<String>,
    #[serde(default)]
    pub fixes: Vec<Fix>,
}

impl Finding {
    pub fn new(level: Level, message: impl Into<String>) -> Finding {
        Finding {
            level,
            message: message.into(),
            page: None,
            area: None,
            markups: Vec::new(),
            fixes: Vec::new(),
        }
    }

    #[must_use] // WHAT: a caller that drops the finding gets a warning. WHY: it takes the finding and hands back a new one with the sheet added, so dropping it loses the finding.
    pub fn on(mut self, page: u32) -> Finding {
        self.page = Some(page);
        self
    }

    #[must_use] // WHAT: a caller that drops the finding gets a warning. WHY: it takes the finding and hands back a new one with the place added, so dropping it loses the finding.
    pub fn at(mut self, area: [f64; 4]) -> Finding {
        self.area = Some(area);
        self
    }

    #[must_use] // WHAT: a caller that drops the finding gets a warning. WHY: it takes the finding and hands back a new one with the markup added, so dropping it loses the finding.
    pub fn about(mut self, markup: impl Into<String>) -> Finding {
        self.markups.push(markup.into());
        self
    }

    #[must_use] // WHAT: a caller that drops the finding gets a warning. WHY: it takes the finding and hands back a new one with the button added, so dropping it loses the finding.
    pub fn fix(mut self, label: impl Into<String>, actions: Vec<Action>) -> Finding {
        self.fixes.push(Fix {
            label: label.into(),
            actions,
        });
        self
    }
}

/// One button: a label and what it does.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Fix {
    pub label: String,
    pub actions: Vec<Action>,
}

/// Everything a plugin can ask for. Hyperview carries each one out itself,
/// only when somebody clicks, and each click is one step on the Undo list.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Action {
    /// Sets a sheet's scale. `ratio` as in [`Scale::ratio`].
    SetSheetScale { page: u32, ratio: f64, text: String },
    SetSubject { markup: String, subject: String },
    /// Writes one custom column, counted from zero.
    SetColumn { markup: String, column: u8, value: String },
    SetQuantity { markup: String, quantity: f64 },
    /// A pitch, as rise over run. A rise of nothing takes it off.
    SetSlope { markup: String, rise: f64, run: f64 },
    /// Takes off a length along the given points, with a subject. When the
    /// office's tool chest has a tool by that subject, the new markup is
    /// made with that tool, columns and all.
    AddLength { page: u32, points: Vec<[f64; 2]>, subject: String },
    /// Counts one item at each point, with a subject: one count markup per
    /// point, exactly as a person clicking would make them. When the office's
    /// chest has a count tool by that subject, it is made with that tool.
    AddCount { page: u32, points: Vec<[f64; 2]>, subject: String },
    /// Takes off the area inside the outline, with a subject.
    AddArea { page: u32, points: Vec<[f64; 2]>, subject: String },
    /// A revision cloud round `area`, carrying a note: "look here".
    AddCloud { page: u32, area: [f64; 4], note: String },
    /// Words in a box on the sheet.
    AddText { page: u32, area: [f64; 4], text: String },
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Table {
    pub title: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// A last line, set apart.
    #[serde(default)]
    pub totals: Vec<String>,
}

impl Table {
    /// The table as CSV, for a spreadsheet.
    #[must_use] // WHAT: a caller that drops the CSV gets a warning. WHY: writing it has no other effect.
    pub fn to_csv(&self) -> String {
        let quote = |s: &String| {
            if s.contains([',', '"', '\n']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.clone()
            }
        };
        let mut out = String::new();
        let mut line = |cells: &Vec<String>| {
            out.push_str(&cells.iter().map(quote).collect::<Vec<_>>().join(","));
            out.push('\n');
        };
        line(&self.columns);
        for row in &self.rows {
            line(row);
        }
        if !self.totals.is_empty() {
            line(&self.totals);
        }
        out
    }
}

// ---- the plugin's side -------------------------------------------------------

/// The exports, for a plugin written in Rust:
///
/// ```ignore
/// plugin_api::plugin!(manifest, run);
/// fn manifest() -> plugin_api::Manifest { ... }
/// fn run(input: plugin_api::Input) -> Result<plugin_api::Output, String> { ... }
/// ```
#[cfg(feature = "guest")]
#[macro_export]
macro_rules! plugin {
    ($manifest:path, $run:path) => {
        #[no_mangle]
        pub extern "C" fn hv_alloc(len: i32) -> i32 {
            $crate::guest::alloc(len)
        }
        #[no_mangle]
        pub extern "C" fn hv_manifest() -> i64 {
            $crate::guest::give(&$manifest())
        }
        #[no_mangle]
        pub extern "C" fn hv_run(ptr: i32, len: i32) -> i64 {
            $crate::guest::run(ptr, len, $run)
        }
    };
}

#[cfg(feature = "guest")]
pub mod guest {
    //! What [`crate::plugin!`] expands to. A plugin is instantiated fresh for
    //! every run and thrown away after, so nothing here frees anything.

    use super::*;

    pub fn alloc(len: i32) -> i32 {
        let mut room: Vec<u8> = Vec::with_capacity(len.max(0) as usize);
        let at = room.as_mut_ptr() as usize as i32;
        std::mem::forget(room);
        at
    }

    pub fn give<T: Serialize>(value: &T) -> i64 {
        let bytes = serde_json::to_vec(value).unwrap_or_else(|e| {
            serde_json::to_vec(&Answer::Failed(format!("could not write the answer: {e}")))
                .unwrap_or_default()
        });
        let at = bytes.as_ptr() as usize as u32 as i64;
        let len = bytes.len() as u32 as i64;
        std::mem::forget(bytes);
        (at << 32) | len
    }

    pub fn run(ptr: i32, len: i32, run: impl FnOnce(Input) -> Result<Output, String>) -> i64 {
        // SAFETY: Hyperview wrote exactly `len` bytes at `ptr`, into room it
        // asked `alloc` for.
        let bytes = unsafe { std::slice::from_raw_parts(ptr as usize as *const u8, len.max(0) as usize) };
        let answer = match unpack(bytes) {
            Err(e) => Answer::Failed(format!("the input could not be read: {e}")),
            Ok(input) => match run(input) {
                Ok(output) => Answer::Done(output),
                Err(why) => Answer::Failed(why),
            },
        };
        give(&answer)
    }
}

// ---- the file a plugin travels in -------------------------------------------

/// The first line of a plugin file.
pub const MAGIC: &str = "HVPLUGIN1";

/// The second line of a plugin file: who signed what.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Header {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub name: String,
    /// SHA-256 of the WebAssembly, in hex.
    pub sha256: String,
    pub bytes: u64,
    /// Which of the program's trusted keys signed it.
    pub key: String,
    /// Ed25519 over [`signing_payload`], in hex.
    pub signature: String,
}

/// The exact bytes a plugin's signature is over. `publish.py sign-plugin`
/// builds the same string; a test holds the two together.
#[must_use] // WHAT: a caller that drops the bytes gets a warning. WHY: building them has no other effect.
pub fn signing_payload(id: &str, version: &str, sha256: &str) -> Vec<u8> {
    format!("hyperview-plugin/1\n{id}\n{version}\n{}\n", sha256.to_lowercase()).into_bytes()
}

/// Splits a plugin file into its header and its WebAssembly. Checks the
/// shape only; whether to believe it is [`hub::plugin`]'s question.
///
/// # Errors
///
/// The reason, in words for the person, when the file is not a plugin file,
/// is cut short, holds more than its header says, or has a header that does
/// not read.
pub fn open(file: &[u8]) -> Result<(Header, &[u8]), String> {
    let first = file
        .iter()
        .position(|b| *b == b'\n')
        .ok_or("That is not an Excalibur View plugin.")?;
    if file[..first].strip_suffix(b"\r").unwrap_or(&file[..first]) != MAGIC.as_bytes() {
        return Err("That is not an Excalibur View plugin.".into());
    }
    let rest = &file[first + 1..];
    let second = rest
        .iter()
        .position(|b| *b == b'\n')
        .ok_or("The plugin file is cut short.")?;
    let header: Header = serde_json::from_slice(&rest[..second])
        .map_err(|e| format!("The plugin file's header could not be read: {e}"))?;
    let wasm = &rest[second + 1..];
    let held = u64::try_from(wasm.len()).unwrap_or(u64::MAX); // WHAT: how many bytes of WebAssembly the file holds, as the header counts them. WHY: a checked conversion; a size past u64 (which no file can be) reads as the largest, so it still never matches.
    if held != header.bytes { // WHAT: the file holds a different amount than its header says. WHY: a file cut short or added to is refused before anything trusts it.
        // WHAT: which way it is off, and by how much. WHY: the difference is counted without signs, so a file longer than it says can't wrap round into a nonsense number.
        let (by, which) = if held < header.bytes { (header.bytes - held, "short of") } else { (held - header.bytes, "more than") };
        return Err(format!("The plugin file is {by} bytes {which} what it says it holds.")); // WHAT: the refusal, in words. WHY: says plainly what is wrong with the file.
    }
    Ok((header, wasm))
}

/// Puts a plugin file together. What the signing tools write.
#[must_use] // WHAT: a caller that drops the file gets a warning. WHY: putting it together has no other effect.
pub fn seal(header: &Header, wasm: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(wasm.len() + 512);
    out.extend_from_slice(MAGIC.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(&serde_json::to_vec(header).unwrap_or_default());
    out.push(b'\n');
    out.extend_from_slice(wasm);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(wasm: &[u8]) -> Header {
        Header {
            id: "shop-tools".into(),
            version: "1.0.0".into(),
            name: "Shop Tools".into(),
            sha256: "ab".repeat(32),
            bytes: wasm.len() as u64,
            key: "test".into(),
            signature: "00".repeat(64),
        }
    }

    #[test]
    fn a_sealed_plugin_opens_to_what_went_in() {
        let wasm = b"\0asm\x01\0\0\0 and the rest";
        let file = seal(&header(wasm), wasm);
        let (h, w) = open(&file).unwrap();
        assert_eq!(h, header(wasm));
        assert_eq!(w, wasm);
    }

    #[test]
    fn anything_else_is_refused_with_a_reason() {
        assert!(open(b"MZ\x90\0 a program").is_err());
        let wasm = b"\0asm\x01\0\0\0";
        let mut file = seal(&header(wasm), wasm);
        file.truncate(file.len() - 2);
        assert!(open(&file).unwrap_err().contains("short"));
    }

    #[test]
    fn a_packed_input_comes_back_as_it_went() {
        let input = Input {
            command: "scales".into(),
            current_page: 1,
            sheets: vec![
                Sheet {
                    page: 0,
                    name: "S-201".into(),
                    width: 2592.0,
                    words: vec![Word { text: "W12X26 — TYP".into(), area: [1.5, 2.0, 30.25, 9.0], size: 7.0 }],
                    lines: vec![Line { a: [0.0, 1.0], b: [100.5, 1.0], width: 1.4, dashed: true }],
                    ..Default::default()
                },
                Sheet { page: 1, name: "S-202".into(), ..Default::default() },
            ],
            markups: vec![Markup { id: "a".into(), kind: "length".into(), quantity: 3.0, ..Default::default() }],
            ..Default::default()
        };
        let back = unpack(&pack(&input)).unwrap();
        assert_eq!(back.command, "scales");
        assert_eq!(back.sheets[0].words, input.sheets[0].words);
        assert!((back.sheets[0].lines[0].width - 1.4).abs() < 1e-6);
        assert!(back.sheets[0].lines[0].dashed);
        assert_eq!(back.sheets[1].name, "S-202");
        assert_eq!(back.markups[0].quantity, 3.0);
        assert!(unpack(&pack(&input)[..30]).is_err());
        // Plain JSON still reads.
        assert_eq!(unpack(br#"{"command":"x"}"#).unwrap().command, "x");
    }

    #[test]
    fn a_number_narrows_to_the_nearest_f32() {
        // WHAT: numbers whose nearest f32 Rust's own parser gives exactly. WHY: an independent answer to check the rounding against.
        for v in [0.0, -0.0, 1.0, 1.4, -1.4, 0.1, 2592.0, 100.5, 1e-40, -1e-44, 3.4e38, 1.0 + f64::EPSILON, 123_456_789.123] {
            assert_eq!(narrow(v).to_bits(), v.to_string().parse::<f32>().unwrap().to_bits(), "{v}"); // WHAT: bit for bit the same. WHY: rounding, sign and subnormals all right.
        }
        let one = f64::from(f32::from_bits(0x3f80_0000)); // WHAT: 1.0. WHY: the start of the ties below.
        let ulp = f64::from(f32::EPSILON); // WHAT: one f32 step at 1.0. WHY: the ties sit half a step past it.
        assert_eq!(narrow(one + ulp / 2.0).to_bits(), 0x3f80_0000); // WHAT: a tie goes to the even neighbour, down here. WHY: ties to even, not always up.
        assert_eq!(narrow(one + ulp * 1.5).to_bits(), 0x3f80_0002); // WHAT: a tie goes to the even neighbour, up here. WHY: the same rule the other way.
        assert_eq!(narrow(f64::from(f32::from_bits(1))).to_bits(), 1); // WHAT: the smallest f32 survives. WHY: subnormals are kept, not flushed.
    }

    #[test]
    fn a_number_past_f32_saturates_and_the_specials_stay() {
        assert_eq!(narrow(1e300), f32::MAX); // WHAT: too big gives the largest f32. WHY: not infinity, which would break sums.
        assert_eq!(narrow(-1e300), f32::MIN); // WHAT: the same on the negative side. WHY: the sign is kept.
        assert_eq!(narrow(f64::from(f32::MAX) * (1.0 + f64::EPSILON)), f32::MAX); // WHAT: just past the largest still gives the largest. WHY: rounding up past the top saturates too.
        assert_eq!(narrow(f64::INFINITY), f32::INFINITY); // WHAT: infinity stays infinity. WHY: it was never a finite point.
        assert_eq!(narrow(f64::NEG_INFINITY), f32::NEG_INFINITY); // WHAT: and keeps its sign. WHY: the same.
        assert!(narrow(f64::NAN).is_nan()); // WHAT: not-a-number stays so. WHY: there is nothing to round it to.
        assert_eq!(narrow(1e-300).to_bits(), 0); // WHAT: far too tiny gives zero. WHY: the nearest f32.
    }

    #[test]
    fn the_plain_json_pack_falls_back_to_reads_back() {
        let input = Input { command: "x".into(), ..Default::default() }; // WHAT: a small input. WHY: one that packs.
        assert!(pack(&input).starts_with(PACKED)); // WHAT: packs in the fast form. WHY: everything fits.
        assert_eq!(unpack(&serde_json::to_vec(&input).unwrap()).unwrap().command, "x"); // WHAT: the fallback form reads back. WHY: what pack sends when a length won't fit.
    }

    #[test]
    fn a_plugin_file_longer_than_it_says_is_refused_without_a_negative_count() {
        let wasm = b"\0asm\x01\0\0\0"; // WHAT: eight bytes of WebAssembly. WHY: a small plugin.
        let mut file = seal(&header(wasm), wasm); // WHAT: a sealed file. WHY: one that opens.
        file.extend_from_slice(b"xyz"); // WHAT: three bytes too many. WHY: the case that used to give a negative number.
        assert_eq!(open(&file).unwrap_err(), "The plugin file is 3 bytes more than what it says it holds."); // WHAT: says by how much, the right way round. WHY: no wrapped or negative count.
        file.truncate(file.len() - 5); // WHAT: now two bytes too few. WHY: the other way.
        assert_eq!(open(&file).unwrap_err(), "The plugin file is 2 bytes short of what it says it holds."); // WHAT: the shortfall. WHY: the same message as before for a short file.
    }

    #[test]
    fn the_signed_text_is_fixed_to_the_byte() {
        assert_eq!(
            signing_payload("a-b", "1.2.3", "ABCDEF"),
            b"hyperview-plugin/1\na-b\n1.2.3\nabcdef\n".to_vec()
        );
    }

    #[test]
    fn actions_read_the_way_a_plugin_writes_them() {
        let text = r#"[{"do":"set_sheet_scale","page":2,"ratio":48,"text":"1/4\" = 1'-0\""},
                       {"do":"add_length","page":0,"points":[[1,2],[3,4]],"subject":"W12x26"}]"#;
        let actions: Vec<Action> = serde_json::from_str(text).unwrap();
        assert_eq!(
            actions[0],
            Action::SetSheetScale { page: 2, ratio: 48.0, text: "1/4\" = 1'-0\"".into() }
        );
        assert!(matches!(&actions[1], Action::AddLength { subject, .. } if subject == "W12x26"));
    }

    #[test]
    fn a_table_goes_to_csv_with_its_commas_kept_in_their_cells() {
        let t = Table {
            title: "x".into(),
            columns: vec!["Shape".into(), "Ends".into()],
            rows: vec![vec!["W12x26, typ".into(), "4".into()]],
            totals: vec!["Total".into(), "4".into()],
        };
        assert_eq!(t.to_csv(), "Shape,Ends\n\"W12x26, typ\",4\nTotal,4\n");
    }
}
