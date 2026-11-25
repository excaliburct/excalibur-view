//! Scale and number formatting, the PDF way.
//!
//! A measurement markup carries its own scale in a `/Measure` dictionary — the
//! same one Acrobat uses, which is why Bluebeam's measurements read correctly in
//! both. Hyperview reads and writes exactly that, so `12'-3 1/2"` comes out of the
//! file's own rules rather than out of an assumption made here.

use pdf::{dict, Dict, Name, Object};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Fraction {
    #[default]
    Decimal,
    Fraction,
    Round,
    Truncate,
}

impl Fraction {
    fn read(name: &str) -> Fraction {
        match name {
            "F" => Fraction::Fraction,
            "R" => Fraction::Round,
            "T" => Fraction::Truncate,
            _ => Fraction::Decimal,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Fraction::Decimal => "D",
            Fraction::Fraction => "F",
            Fraction::Round => "R",
            Fraction::Truncate => "T",
        }
    }
}

/// One step of a unit chain: feet, then inches, then sixteenths.
#[derive(Clone, Debug)]
pub struct Format {
    pub unit: String,
    /// Multiplier from the previous step's unit into this one.
    pub conversion: f64,
    pub style: Fraction,
    /// Decimal places as a power of ten, or the denominator of a fraction.
    pub precision: u32,
    pub fixed_denominator: bool,
    pub prefix: String,
    pub suffix: String,
    pub thousands: String,
    pub decimal_point: String,
}

impl Default for Format {
    fn default() -> Format {
        Format {
            unit: String::new(),
            conversion: 1.0,
            style: Fraction::Decimal,
            precision: 100,
            fixed_denominator: true,
            prefix: " ".into(),
            suffix: String::new(),
            thousands: String::new(),
            decimal_point: ".".into(),
        }
    }
}

/// The most characters read from a number format's unit or separator. A
/// real one is a few characters ("ft", "in", "sq ft").
pub const MOST_TEXT: usize = 32; // WHAT: thirty-two characters. WHY: security review M1: a megabyte unit referenced from every slot can't exhaust memory

/// The most characters read from a measure's scale ratio (`/R`).
pub const MOST_RATIO: usize = 64; // WHAT: sixty-four characters. WHY: "1/4\" = 1'-0\"" and the longest real ones fit with room to spare

/// A text string's first `most` characters, decoding no more of it than
/// that needs: a byte-order mark and up to four bytes a character.
fn short_text(object: Option<&Object>, most: usize) -> Option<String> {
    let bytes = object?.as_string()?; // WHAT: the string's bytes, borrowed. WHY: never copied whole
    let head = &bytes[..bytes.len().min(2 + most.saturating_mul(4))]; // WHAT: only as much as `most` characters can take. WHY: a huge string costs nothing beyond this
    Some(pdf::text::decode(head).chars().take(most).collect()) // WHAT: decoded and cut to `most` characters. WHY: the bound holds whatever the encoding
}

impl Format {
    pub fn read(dict: &Dict) -> Format {
        let text = |key: &str| short_text(dict.get(key), MOST_TEXT).unwrap_or_default(); // WHAT: a unit or separator, at most 32 characters. WHY: security review M1: a huge string is never copied whole
        // The separator between the number and its unit defaults to a space.
        // Writing /PS() explicitly, as Bluebeam does for feet and inches, is
        // what removes it — which is the difference between `20'` and `20 '`.
        let prefix = match dict.get("PS") {
            Some(value) => short_text(Some(value), MOST_TEXT).unwrap_or_default(), // WHAT: the separator, bounded. WHY: likewise
            None => " ".to_string(),
        };
        Format {
            unit: text("U"),
            conversion: dict.get("C").and_then(|o| o.as_f64()).unwrap_or(1.0),
            style: dict
                .get("F")
                .and_then(|o| o.as_name())
                .map(|n| Fraction::read(n.as_str()))
                .unwrap_or_default(),
            precision: dict
                .get("D")
                .and_then(|o| o.as_i64())
                .unwrap_or(100)
                .clamp(1, 1_000_000) as u32,
            fixed_denominator: dict
                .get("FD")
                .and_then(|o| o.as_bool())
                .unwrap_or(false),
            prefix,
            suffix: text("SS"),
            thousands: text("RT"),
            decimal_point: {
                let rd = text("RD");
                if rd.is_empty() {
                    ".".into()
                } else {
                    rd
                }
            },
        }
    }

    pub fn write(&self) -> Dict {
        let mut d = dict! {
            "Type" => Object::name("NumberFormat"),
            "U" => Object::text(&self.unit),
            "C" => Object::real(self.conversion),
        };
        if self.style != Fraction::Decimal {
            d.set(Name::new("F"), Object::name(self.style.as_str()));
        }
        d.set(Name::new("D"), Object::Int(self.precision as i64));
        d.set(Name::new("FD"), Object::Bool(self.fixed_denominator));
        d.set(Name::new("PS"), Object::text(&self.prefix));
        d.set(Name::new("SS"), Object::text(&self.suffix));
        d
    }

    /// Renders one step. `whole_only` is set for every step but the last,
    /// because the remainder is handed on rather than shown here.
    fn render(&self, value: f64, whole_only: bool) -> String {
        let body = if whole_only {
            self.group(value.trunc().abs())
        } else {
            match self.style {
                Fraction::Fraction => return self.render_fraction(value.abs()),
                Fraction::Truncate => self.group(value.abs().trunc()),
                _ => {
                    // Revu rounds to the precision and then drops trailing
                    // zeros: an area of 50.098 sf is written `50.1 sf`, not
                    // `50.10 sf`, and a zero is written `0`. Checked against
                    // forty six markups out of a real marked up sheet.
                    let places = places_of(self.precision);
                    let text = trim_zeros(&format!("{:.*}", places, value.abs()));
                    self.punctuate(&text)
                }
            }
        };
        format!("{body}{}{}{}", self.prefix, self.unit, self.suffix)
    }

    fn render_fraction(&self, value: f64) -> String {
        let denominator = self.precision.max(1);
        let scaled = (value * denominator as f64).round() as i64;
        let whole = scaled / denominator as i64;
        let mut numerator = (scaled % denominator as i64) as u32;
        let mut over = denominator;
        if numerator > 0 && !self.fixed_denominator {
            let g = gcd(numerator, over);
            numerator /= g;
            over /= g;
        }
        let mut body = self.group(whole as f64);
        if numerator > 0 {
            body.push_str(&format!(" {numerator}/{over}"));
        }
        format!("{body}{}{}{}", self.prefix, self.unit, self.suffix)
    }

    /// Rounds a value to what this format can actually show, so the rounding
    /// happens once and in the right place.
    fn settle(&self, value: f64) -> f64 {
        match self.style {
            Fraction::Fraction => {
                let d = self.precision.max(1) as f64;
                (value * d).round() / d
            }
            Fraction::Truncate => value.trunc(),
            _ => {
                let d = 10f64.powi(places_of(self.precision) as i32);
                (value * d).round() / d
            }
        }
    }

    fn group(&self, value: f64) -> String {
        self.punctuate(&format!("{}", value as i64))
    }

    fn punctuate(&self, text: &str) -> String {
        let text = text.replace('.', &self.decimal_point);
        if self.thousands.is_empty() {
            return text;
        }
        let (whole, rest) = match text.split_once(&self.decimal_point) {
            Some((a, b)) => (a.to_string(), Some(b.to_string())),
            None => (text.clone(), None),
        };
        let mut grouped = String::new();
        for (i, ch) in whole.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                grouped.push_str(&self.thousands.chars().rev().collect::<String>());
            }
            grouped.push(ch);
        }
        let mut out: String = grouped.chars().rev().collect();
        if let Some(rest) = rest {
            out.push_str(&self.decimal_point);
            out.push_str(&rest);
        }
        out
    }
}

/// Drops the trailing zeros from a decimal, and the point with them.
fn trim_zeros(text: &str) -> String {
    if !text.contains('.') {
        return text.to_string();
    }
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

fn places_of(precision: u32) -> usize {
    match precision {
        0 | 1 => 0,
        p => (p as f64).log10().round().max(0.0) as usize,
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a.max(1)
}

/// The whole `/Measure` dictionary.
#[derive(Clone, Debug)]
pub struct Measure {
    /// The scale as words, e.g. `0.25 in = 1 ft' in"`.
    pub ratio: String,
    /// Points to the primary unit.
    pub x: Vec<Format>,
    pub distance: Vec<Format>,
    pub area: Vec<Format>,
    pub angle: Vec<Format>,
    pub volume: Vec<Format>,
}

/// The most number formats read from any one list of a `/Measure`. A real
/// one has one to three (feet, then inches); a file listing thousands is
/// read no further than this.
pub const MOST_FORMATS: usize = 8; // WHAT: eight per list. WHY: security review M1: a hostile list of a million entries can't cost a million reads

impl Measure {
    /// A `/Measure` whose number formats are all written inline.
    pub fn read(dict: &Dict) -> Option<Measure> {
        Measure::from_lists(dict, |key| {
            dict.get(key).and_then(|o| o.as_array()).map(|a| a.iter().take(MOST_FORMATS).filter_map(|o| o.as_dict()).map(Format::read).collect()).unwrap_or_default() // WHAT: up to eight inline formats. WHY: as before, bounded
        })
    }

    /// A `/Measure` read from `doc`, its format lists and the formats in them
    /// followed through any references. Nothing is copied whole: each format
    /// is read where it is.
    pub fn read_in(doc: &pdf::Document, dict: &Dict) -> Option<Measure> {
        let formats = |list: &[Object]| -> Vec<Format> {
            list.iter()
                .take(MOST_FORMATS) // WHAT: eight at most. WHY: security review M1
                .filter_map(|o| match o {
                    Object::Ref(_) => doc.follow(o).as_dict().map(Format::read), // WHAT: a format kept as its own object, read in place. WHY: some exporters write them so; the object is shared, never cloned
                    inline => inline.as_dict().map(Format::read), // WHAT: an inline format. WHY: the usual case
                })
                .collect()
        };
        Measure::from_lists(dict, |key| match dict.get(key) {
            Some(Object::Ref(_)) => doc.at(dict, key).as_array().map(formats).unwrap_or_default(), // WHAT: a list kept as its own object. WHY: read through its reference
            Some(list) => list.as_array().map(formats).unwrap_or_default(), // WHAT: an inline list. WHY: borrowed, not copied
            None => Vec::new(), // WHAT: no such list. WHY: most measures have only some
        })
    }

    /// A measure from its format lists, `None` without an X list.
    fn from_lists(dict: &Dict, list: impl Fn(&str) -> Vec<Format>) -> Option<Measure> {
        let x = list("X");
        if x.is_empty() {
            return None;
        }
        Some(Measure {
            ratio: short_text(dict.get("R"), MOST_RATIO).unwrap_or_default(), // WHAT: the scale's words, at most 64 characters. WHY: security review M1
            x,
            distance: list("D"),
            area: list("A"),
            angle: list("T"),
            volume: list("V"),
        })
    }

    pub fn write(&self) -> Dict {
        let array = |list: &Vec<Format>| {
            Object::Array(list.iter().map(|f| Object::Dict(f.write())).collect())
        };
        dict! {
            "Type" => Object::name("Measure"),
            "Subtype" => Object::name("RL"),
            "R" => Object::text(&self.ratio),
            "X" => array(&self.x),
            "D" => array(&self.distance),
            "A" => array(&self.area),
            "T" => array(&self.angle),
            "V" => array(&self.volume),
        }
    }

    /// WHAT: The primary unit's name as the scale writes it ("ft", "in", "m").
    /// WHY: a length in the primary unit means nothing without it; the takeoff
    /// is sent in feet, so the unit is what it is converted from.
    #[must_use]
    pub fn primary_unit(&self) -> &str {
        // WHAT: the first step's unit, none when there is none. WHY: the length is in that unit (per_point).
        self.x.first().map_or("", |f| f.unit.as_str())
    }

    /// WHAT: How many feet one of the primary unit is; `None` for a unit this
    /// doesn't know. WHY: a sheet scaled in inches or metres sent its lengths
    /// as "feet" before (`per_point` is in the primary unit);
    /// an unknown unit is said, never guessed.
    #[must_use]
    pub fn feet_per_unit(&self) -> Option<f64> {
        // WHAT: looked up by name. WHY: one table, below.
        feet_in(self.primary_unit())
    }

    /// How many of the primary unit one point of paper is worth.
    pub fn per_point(&self) -> f64 {
        self.x.first().map(|f| f.conversion).unwrap_or(1.0)
    }

    /// A length in points, as the drawing would read it.
    pub fn length(&self, points: f64) -> String {
        chain(&self.distance, points * self.per_point())
    }

    /// An area in square points.
    pub fn area(&self, square_points: f64) -> String {
        let per = self.per_point();
        chain(&self.area, square_points * per * per)
    }

    pub fn volume(&self, cubic_points: f64) -> String {
        let per = self.per_point();
        chain(&self.volume, cubic_points * per * per * per)
    }

    pub fn angle(&self, degrees: f64) -> String {
        chain(&self.angle, degrees)
    }

    /// The length as a plain number in the primary unit, for totalling.
    pub fn length_value(&self, points: f64) -> f64 {
        points * self.per_point()
    }

    /// The other way round: how many points of paper a real length is worth.
    ///
    /// What a grid spaced in feet, or a shape sketched to typed dimensions,
    /// needs in order to be drawn at all.
    pub fn points_for(&self, primary_units: f64) -> f64 {
        let per = self.per_point();
        if per.abs() < 1e-12 {
            return 0.0;
        }
        primary_units / per
    }

    /// WHAT: The scale as real inches of building to an inch of paper (48 at
    /// 1/4" = 1'-0", 100 at 1:100), when its unit is a length this knows.
    /// WHY: one number to check a scale by, whatever unit it is written in.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn paper_ratio(&self) -> Option<f64> {
        self.feet_per_unit().map(|feet| self.per_point() * feet * 12.0 * 72.0) // WHAT: units a point, in feet, in inches, an inch of paper. WHY: as above
    }

    /// WHAT: Whether this scale can be measured with: every conversion a
    /// number above zero and short of a billion, the scale between
    /// `LEAST_SCALE` and `MOST_SCALE` when its unit is known, and its words
    /// saying the same scale as its numbers where they can be read. The
    /// reason when it can't. WHY: a scale is sent by any program or seat;
    /// one of zero, below zero, infinite or a million times too big made
    /// every length on the takeoff nothing, negative or absurd (review M1).
    ///
    /// # Errors
    ///
    /// Why the scale can't be trusted, in words.
    pub fn trusted(&self) -> Result<(), String> {
        let every = self.x.iter().chain(&self.distance).chain(&self.area).chain(&self.volume); // WHAT: every number format that converts a measurement. WHY: each is multiplied into a quantity
        if let Some(bad) = every.map(|f| f.conversion).find(|c| !c.is_finite() || *c <= 0.0 || *c >= 1e9) {
            return Err(format!("its conversion {bad} isn't a number above zero and short of a billion")); // WHAT: refused, saying which. WHY: as above
        }
        if let Some(ratio) = self.paper_ratio() {
            if !(LEAST_SCALE..=MOST_SCALE).contains(&ratio) {
                return Err(format!("it is 1:{ratio:.4}, outside the scales a drawing is drawn at (1:{LEAST_SCALE} to 1:{MOST_SCALE})")); // WHAT: refused. WHY: no sheet is drawn at that
            }
            if let Some(said) = said_ratio(&self.ratio) {
                if (ratio / said - 1.0).abs() > 0.02 {
                    return Err(format!("its words say {} (1:{said:.4}) but its numbers are 1:{ratio:.4}", self.ratio)); // WHAT: refused. WHY: a label that lies would be shown beside wrong numbers
                }
            }
        }
        Ok(())
    }
}

/// WHAT: The largest a drawing is drawn, as real inches to an inch of paper:
/// a detail a hundred times full size. WHY: the floor `Measure::trusted` holds a scale to.
pub const LEAST_SCALE: f64 = 0.01; // WHAT: 1:0.01. WHY: larger than any detail on a shop drawing

/// WHAT: The smallest a drawing is drawn: a site map at 1:1,000,000. WHY: the
/// ceiling `Measure::trusted` holds a scale to.
pub const MOST_SCALE: f64 = 1_000_000.0; // WHAT: 1:1,000,000. WHY: smaller than any map a fabricator is sent

/// WHAT: The scale a scale's words say, as real inches to an inch of paper,
/// when they read as one: `1/4" = 1'-0"`, `0.25 in = 1 ft' in"`, `1" = 10'`,
/// `1:100`, or any of those after "calibrated, ". `None` when they don't
/// read (a bare "calibrated", or one that says how far off it is). WHY: the
/// words are checked against the numbers only where they can be read.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn said_ratio(words: &str) -> Option<f64> {
    let text = words.trim(); // WHAT: the words. WHY: read below
    let text = text.strip_prefix("calibrated,").map_or(text, str::trim); // WHAT: a calibration that landed on a named scale says it after a comma. WHY: that name is checked
    if let Some((paper, real)) = text.split_once('=') {
        let (paper, real) = (inches_in(paper)?, inches_in(real)?); // WHAT: both sides in inches. WHY: their ratio is the scale
        return (paper > 0.0).then(|| real / paper); // WHAT: real to paper. WHY: as `paper_ratio`
    }
    let (paper, real) = text.split_once(':')?; // WHAT: a metric ratio, 1:100. WHY: the other way scales are written
    let (paper, real) = (number(paper.trim())?, number(real.trim())?); // WHAT: its two numbers. WHY: their ratio is the scale
    (paper > 0.0).then(|| real / paper)
}

/// WHAT: A length written in feet and inches as inches: `1/4"`, `1'-0"`,
/// `1 1/2"`, `0.25 in`, `1 ft' in"`, `10'`. `None` for anything else, a
/// number with no unit after it included. WHY: `said_ratio` reads both sides of a scale.
fn inches_in(text: &str) -> Option<f64> {
    let (mut total, mut pending, mut any, mut waiting) = (0.0, 0.0, false, false); // WHAT: inches so far, a number waiting for its unit, whether a unit was seen, and whether a number waits. WHY: "1 1/2\"" is two numbers then a unit
    for token in text.split(|c: char| c.is_whitespace() || c == '-').filter(|t| !t.is_empty()) {
        let at = token.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '/')).unwrap_or(token.len()); // WHAT: where the number ends. WHY: the rest is its unit
        let (digits, unit) = token.split_at(at); // WHAT: the number and its unit. WHY: read apart
        if !digits.is_empty() {
            pending += number(digits)?; // WHAT: added to what waits for a unit. WHY: "1" then "1/2\""
            waiting = true; // WHAT: a number waits. WHY: one with no unit after it says nothing
        }
        let per = if unit.is_empty() {
            continue; // WHAT: a number alone. WHY: its unit comes next
        } else if unit.starts_with("ft") || unit.starts_with('\'') {
            12.0 // WHAT: feet. WHY: twelve inches each
        } else if unit.starts_with("in") || unit.starts_with('"') {
            1.0 // WHAT: inches. WHY: as they are
        } else {
            return None; // WHAT: a unit not read. WHY: never guessed
        };
        total += pending * per; // WHAT: counted in. WHY: its unit is known
        (pending, any, waiting) = (0.0, true, false); // WHAT: nothing waiting, a unit seen. WHY: the next number starts afresh
    }
    (any && !waiting).then_some(total) // WHAT: the inches, when every number had its unit. WHY: "1 = 10" says nothing
}

/// WHAT: A number as a scale writes it: `48`, `0.25`, `1/4`. WHY: `inches_in` and `said_ratio` read them.
fn number(text: &str) -> Option<f64> {
    match text.split_once('/') {
        Some((top, bottom)) => {
            let (top, bottom): (f64, f64) = (top.parse().ok()?, bottom.parse().ok()?); // WHAT: the fraction's two numbers. WHY: divided
            (bottom.abs() > 0.0).then(|| top / bottom) // WHAT: the fraction, never over zero. WHY: a broken one is no number
        }
        None => text.parse::<f64>().ok().filter(|n| n.is_finite()), // WHAT: a plain number. WHY: as written
    }
}

/// Walks a chain of formats: feet, then inches, then sixteenths.
///
/// The rounding happens **first**, in the smallest unit, and the answer is then
/// broken back up. Rounding each step on the way down is what produces
/// `11'-12"` for a length a hair under twelve feet, and a fabricator who cuts
/// to that number has a problem.
fn chain(formats: &[Format], value: f64) -> String {
    if formats.is_empty() {
        return pdf::write::number(value);
    }
    let negative = value < 0.0;

    // How many of the smallest unit there are in one of each larger unit.
    let mut scale = Vec::with_capacity(formats.len());
    let mut running = 1.0;
    for format in formats {
        running *= format.conversion;
        scale.push(running);
    }
    let smallest = *scale.last().unwrap();
    let last = formats.last().unwrap();

    let total = last.settle(value.abs() * smallest);
    let mut left = total;
    let mut out = String::new();
    for (i, format) in formats.iter().enumerate() {
        if i + 1 == formats.len() {
            out.push_str(&format.render(left, false));
        } else {
            let per = smallest / scale[i];
            let whole = (left / per).floor();
            left -= whole * per;
            out.push_str(&format.render(whole, true));
        }
    }
    if negative && total != 0.0 {
        out.insert(0, '-');
    }
    out
}

/// Builds the measure dictionary for a scale given as real inches of building
/// per inch of paper: 48 for quarter inch scale.
pub fn imperial(ratio: f64, label: &str, denominator: u32) -> Measure {
    let per_point_feet = ratio / 72.0 / 12.0;
    Measure {
        ratio: label.to_string(),
        x: vec![Format {
            unit: "'".into(),
            conversion: per_point_feet,
            style: Fraction::Decimal,
            precision: 100,
            prefix: String::new(),
            ..Default::default()
        }],
        distance: vec![
            Format {
                unit: "'".into(),
                conversion: 1.0,
                style: Fraction::Fraction,
                precision: 1,
                fixed_denominator: true,
                prefix: String::new(),
                suffix: "-".into(),
                ..Default::default()
            },
            Format {
                unit: "\"".into(),
                conversion: 12.0,
                style: Fraction::Fraction,
                precision: denominator.max(1),
                fixed_denominator: false,
                prefix: String::new(),
                ..Default::default()
            },
        ],
        area: vec![Format {
            unit: "sf".into(),
            conversion: 1.0,
            precision: 100,
            ..Default::default()
        }],
        angle: vec![Format {
            unit: "\u{b0}".into(),
            conversion: 1.0,
            precision: 100,
            prefix: String::new(),
            ..Default::default()
        }],
        volume: vec![Format {
            unit: "cu ft".into(),
            conversion: 1.0,
            precision: 100,
            ..Default::default()
        }],
    }
}

/// Builds the measure dictionary for a metric scale given as a ratio: 50 for
/// 1:50. Lengths read in millimetres, areas in square metres.
pub fn metric(ratio: f64, label: &str) -> Measure {
    // One point of paper is ratio/72 inches of building, which is
    // ratio/72*25.4 millimetres.
    let mm_per_point = ratio / 72.0 * 25.4;
    Measure {
        ratio: label.to_string(),
        x: vec![Format {
            unit: "mm".into(),
            conversion: mm_per_point,
            style: Fraction::Decimal,
            precision: 10,
            ..Default::default()
        }],
        distance: vec![Format {
            unit: "mm".into(),
            conversion: 1.0,
            style: Fraction::Decimal,
            precision: 10,
            ..Default::default()
        }],
        area: vec![Format {
            unit: "m\u{b2}".into(),
            conversion: 1.0 / 1_000_000.0,
            style: Fraction::Decimal,
            precision: 100,
            ..Default::default()
        }],
        angle: vec![Format {
            unit: "\u{b0}".into(),
            conversion: 1.0,
            precision: 100,
            prefix: String::new(),
            ..Default::default()
        }],
        volume: vec![Format {
            unit: "m\u{b3}".into(),
            conversion: 1.0 / 1_000_000_000.0,
            precision: 100,
            ..Default::default()
        }],
    }
}

/// WHAT: How many feet one `unit` is, by the names a scale writes ("ft",
/// "in", "yd", "mi", "m", "cm", "mm", "km", their marks and words, any
/// capitals); `None` for one it doesn't know. WHY: the takeoff's lengths go
/// out in feet whatever the sheet was scaled in.
#[must_use]
pub fn feet_in(unit: &str) -> Option<f64> {
    // WHAT: the unit, trimmed, lower case. WHY: "FT" and "ft " are feet.
    let name = unit.trim().to_ascii_lowercase();
    let feet = match name.as_str() {
        // WHAT: feet. WHY: one foot is a foot.
        "ft" | "'" | "feet" | "foot" | "ft'" => 1.0,
        // WHAT: inches. WHY: twelve to the foot.
        "in" | "\"" | "inch" | "inches" => 1.0 / 12.0,
        // WHAT: yards. WHY: three feet.
        "yd" | "yard" | "yards" => 3.0,
        // WHAT: miles. WHY: 5,280 feet.
        "mi" | "mile" | "miles" => 5280.0,
        // WHAT: metres. WHY: a foot is 0.3048 m exactly.
        "m" | "meter" | "metre" | "meters" | "metres" => 1.0 / 0.3048,
        // WHAT: centimetres. WHY: 30.48 to the foot.
        "cm" => 1.0 / 30.48,
        // WHAT: millimetres. WHY: 304.8 to the foot.
        "mm" => 1.0 / 304.8,
        // WHAT: kilometres. WHY: a thousand metres.
        "km" => 1000.0 / 0.3048,
        // WHAT: a unit not known. WHY: said by the caller, never taken as feet.
        _ => return None,
    };
    // WHAT: the feet. WHY: the caller converts with it.
    Some(feet)
}

#[cfg(test)]
mod trust_tests {
    use super::*;

    #[test]
    fn a_scale_is_trusted_only_with_real_numbers_in_range_and_words_that_agree() {
        assert_eq!(imperial(48.0, "1/4\" = 1'-0\"", 16).trusted(), Ok(())); // WHAT: quarter inch. WHY: the everyday scale
        assert_eq!(imperial(48.0, "calibrated, 1/4\" = 1'-0\"", 16).trusted(), Ok(())); // WHAT: a calibration on a named scale. WHY: the viewer's words
        assert_eq!(imperial(47.1, "calibrated (1.9% off 1/4\" = 1'-0\")", 16).trusted(), Ok(())); // WHAT: a calibration a little off. WHY: its words aren't a scale to check
        assert_eq!(imperial(120.0, "1\" = 10'", 16).trusted(), Ok(())); // WHAT: an engineer's scale. WHY: feet alone
        assert_eq!(imperial(48.0, "0.25 in = 1 ft' in\"", 16).trusted(), Ok(())); // WHAT: Bluebeam's way of writing it. WHY: read too
        assert_eq!(metric(100.0, "1:100").trusted(), Ok(())); // WHAT: metric. WHY: read too
        assert!(imperial(96.0, "1/4\" = 1'-0\"", 16).trusted().unwrap_err().contains("its words say")); // WHAT: words that lie. WHY: refused
        assert!(metric(50.0, "1:100").trusted().is_err()); // WHAT: likewise in metric. WHY: refused
        for bad in [0.0, -48.0 / 864.0, f64::INFINITY, f64::NAN, 1e12] {
            let mut m = imperial(48.0, "calibrated", 16); // WHAT: a scale with no words to check. WHY: only the number is under test
            m.x[0].conversion = bad; // WHAT: the bad feet a point. WHY: each refused
            assert!(m.trusted().is_err(), "{bad} taken"); // WHAT: refused. WHY: nothing, negative, infinite or absurd
        }
        for bad in [0.0, -12.0, f64::INFINITY] {
            let mut m = imperial(48.0, "1/4\" = 1'-0\"", 16); // WHAT: a good scale. WHY: only its inches format is broken
            m.distance[1].conversion = bad; // WHAT: the inches a foot, broken. WHY: multiplied into every length it shows
            assert!(m.trusted().unwrap_err().contains("isn't a number above zero"), "{bad} inches a foot taken"); // WHAT: refused. WHY: review M1: every conversion is checked, not only the first
        }
        let mut tiny = imperial(48.0, "calibrated", 16); // WHAT: a scale a hundred thousand times full size. WHY: outside the range
        tiny.x[0].conversion = 1e-9; // WHAT: tiny. WHY: refused
        assert!(tiny.trusted().unwrap_err().contains("outside the scales")); // WHAT: refused. WHY: as above
        assert_eq!(said_ratio("calibrated"), None); // WHAT: nothing to read. WHY: not checked
        assert_eq!(said_ratio("1 = 10"), None); // WHAT: no units. WHY: never guessed
        assert!(said_ratio("1 1/2\" = 1'-0\"").is_some_and(|r| (r - 8.0).abs() < 1e-9)); // WHAT: a mixed number. WHY: read whole
        assert_eq!(said_ratio("1/0\" = 1'"), None); // WHAT: over zero. WHY: no number
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf::Reader;

    /// The measure dictionary out of his own tool chest, verbatim.
    const HIS: &str = "<</Type/Measure/Subtype/RL/R(0.25 in = 1 ft' in\")\
/X[<</Type/NumberFormat/U(')/C 0.05555556/F/F/D 1/FD true/SS()>>]\
/D[<</Type/NumberFormat/U(')/C 1/F/F/D 1/FD true/PS()/SS(-)>>\
<</Type/NumberFormat/U(\")/C 12/F/F/D 16/FD false/PS()/SS()>>]\
/A[<</Type/NumberFormat/U(sf)/C 1/D 100/FD true/SS()>>]\
/T[<</Type/NumberFormat/U(\\260)/C 1/D 100/FD true/PS()/SS()>>]\
/V[<</Type/NumberFormat/U(cu ft)/C 1/D 100/FD true/SS()>>]\
/TargetUnitConversion 0.001157407>>";

    fn his() -> Measure {
        let dict = Reader::new(HIS.as_bytes())
            .object()
            .unwrap()
            .as_dict()
            .unwrap()
            .clone();
        Measure::read(&dict).unwrap()
    }

    #[test]
    fn the_space_before_a_unit_comes_from_the_file_not_from_us() {
        // Bluebeam writes /PS() for feet and inches, which removes the space,
        // and leaves /PS out for square feet, which keeps it.
        let m = his();
        assert_eq!(m.length(360.0), "20'-0\"", "no space before the tick mark");
        assert_eq!(m.area(360.0 * 360.0), "400 sf", "but one before sf");
    }

    #[test]
    fn his_own_scale_reads_out_of_the_file() {
        let m = his();
        assert_eq!(m.ratio, "0.25 in = 1 ft' in\"");
        // Quarter inch scale: one inch of paper is four feet.
        assert!((m.per_point() * 72.0 - 4.0).abs() < 1e-4, "{}", m.per_point() * 72.0);
    }

    #[test]
    fn lengths_come_out_in_feet_and_inches_the_way_the_drawing_reads() {
        let m = his();
        // 5 inches of paper at quarter inch scale is twenty feet.
        assert_eq!(m.length(360.0), "20'-0\"");
        assert_eq!(m.length(360.0 + 9.0), "20'-6\"");
        assert_eq!(m.length(0.0), "0'-0\"");
    }

    #[test]
    fn a_half_inch_shows_as_a_fraction_not_a_decimal() {
        let m = his();
        // 20'-6 1/2" is 246.5 inches; at 48:1 that is 246.5/48 inches of paper.
        let points = 246.5 / 48.0 * 72.0;
        assert_eq!(m.length(points), "20'-6 1/2\"");
    }

    #[test]
    fn rounding_up_to_a_whole_foot_does_not_produce_twelve_inches() {
        let m = his();
        // A hair under twelve feet must read 12'-0", never 11'-12".
        let points = (144.0 - 0.01) / 48.0 * 72.0;
        assert_eq!(m.length(points), "12'-0\"");
    }

    #[test]
    fn areas_and_volumes_use_their_own_units() {
        let m = his();
        // A square five inches on a side is twenty feet square: 400 sf.
        assert_eq!(m.area(360.0 * 360.0), "400 sf");
        assert_eq!(m.angle(90.0), "90\u{b0}");
    }

    #[test]
    fn a_measure_dictionary_survives_being_written_and_read_again() {
        let before = his();
        let dict = before.write();
        let after = Measure::read(&dict).unwrap();
        assert_eq!(after.ratio, before.ratio);
        assert!((after.per_point() - before.per_point()).abs() < 1e-12);
        assert_eq!(after.length(360.0 + 9.0), before.length(360.0 + 9.0));
    }

    #[test]
    fn a_scale_built_from_a_ratio_measures_the_same_as_his() {
        let mine = imperial(48.0, "1/4\" = 1'-0\"", 16);
        assert_eq!(mine.length(360.0), "20'-0\"");
        assert_eq!(mine.length(246.5 / 48.0 * 72.0), "20'-6 1/2\"");
        assert_eq!(mine.area(360.0 * 360.0), "400 sf");
    }

    #[test]
    fn an_eighth_is_not_reported_as_two_sixteenths() {
        let m = imperial(48.0, "1/4\" = 1'-0\"", 16);
        let points = (12.0 + 0.125) / 48.0 * 72.0;
        assert_eq!(m.length(points), "1'-0 1/8\"");
    }

    #[test]
    fn a_negative_length_keeps_its_sign_and_nothing_else_odd() {
        let m = his();
        assert_eq!(m.length(-360.0), "-20'-0\"");
    }

    #[test]
    fn decimal_places_follow_the_precision_that_was_written() {
        let f = Format {
            unit: "sf".into(),
            precision: 100,
            ..Default::default()
        };
        assert_eq!(f.render(19.756, false), "19.76 sf");
        assert_eq!(f.render(50.098068, false), "50.1 sf", "trailing zeros go");
        assert_eq!(f.render(0.0, false), "0 sf");
        let whole = Format {
            unit: "sf".into(),
            precision: 1,
            ..Default::default()
        };
        assert_eq!(whole.render(19.756, false), "20 sf");
    }

    #[test]
    fn thousands_separators_are_used_when_the_file_asks_for_them() {
        let f = Format {
            unit: "sf".into(),
            precision: 1,
            thousands: ",".into(),
            ..Default::default()
        };
        assert_eq!(f.render(1234567.0, false), "1,234,567 sf");
    }
}
