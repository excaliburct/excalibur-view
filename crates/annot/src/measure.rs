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

impl Format {
    pub fn read(dict: &Dict) -> Format {
        let text = |key: &str| dict.get(key).and_then(|o| o.as_text()).unwrap_or_default();
        // The separator between the number and its unit defaults to a space.
        // Writing /PS() explicitly, as Bluebeam does for feet and inches, is
        // what removes it — which is the difference between `20'` and `20 '`.
        let prefix = match dict.get("PS") {
            Some(value) => value.as_text().unwrap_or_default(),
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

impl Measure {
    pub fn read(dict: &Dict) -> Option<Measure> {
        let list = |key: &str| -> Vec<Format> {
            dict.get(key)
                .and_then(|o| o.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|o| o.as_dict())
                        .map(Format::read)
                        .collect()
                })
                .unwrap_or_default()
        };
        let x = list("X");
        if x.is_empty() {
            return None;
        }
        Some(Measure {
            ratio: dict.get("R").and_then(|o| o.as_text()).unwrap_or_default(),
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
