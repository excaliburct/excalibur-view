//! The cut list, and what to buy to cut it from.
//!
//! A markups list tells you what was picked up. A shop list tells you what to
//! buy and what to cut, which is a different question and the one a fabricator
//! actually asks. Revu will give you the first and leave you to build the
//! second in a spreadsheet; this is the second, and the nesting underneath it
//! is something Revu has never had at all.
//!
//! What it does: takes every length measurement, groups them by shape, rounds
//! each one **up** to a cutting length, counts how many of each, totals the
//! weight from the unit weight that came off the tool chest, and — when
//! somebody has said what stock length they buy — works out how many sticks
//! that is and what falls on the floor.
//!
//! Four rules, and all four are about not lying to the shop:
//!
//! * **Lengths round up, never down.** A beam measured at 24'-5 7/8" is cut at
//!   24'-6". Rounding a cut length down is how a member arrives short.
//! * **A shape with no unit weight has no weight.** It is listed, with its
//!   lengths and its count, and the weight column says so rather than guessing
//!   at a section's weight from its name.
//! * **Anything off an unscaled sheet is not on the list.** It is reported
//!   separately, because a cut list that quietly dropped six members is worse
//!   than one that says six are missing.
//! * **Nothing is nested until somebody says what the stock is.** There is no
//!   default mill length in here. A piece longer than the stock is named as
//!   one that will not come out of it, not quietly packed into a stick that
//!   cannot hold it.

use std::collections::BTreeMap;

use annot::Kind;

use crate::row::{Row, WeightColumns};

/// An inch, in feet — the usual cutting increment on an imperial job, and the
/// unit `Row::length` is already in.
pub const INCH: f64 = 1.0 / 12.0;

/// One length of one shape, and how many of them.
#[derive(Clone, Debug, PartialEq)]
pub struct Cut {
    /// The length to cut, in the primary unit — feet, on an imperial job.
    pub length: f64,
    pub count: usize,
    /// Which sheets they came off, so somebody can go and look.
    pub pages: Vec<usize>,
    /// Where each of them is on its sheet's grid: "B-4", "C-4". Empty when
    /// the sheet has no grid, which is most detail sheets.
    ///
    /// This is what turns a cut list into something a fitter can work from.
    /// "Three at 24'-6"" is a purchase order; "three at 24'-6", at B-4, C-4
    /// and D-4" is an instruction.
    pub places: Vec<String>,
}

/// Everything of one shape.
#[derive(Clone, Debug)]
pub struct Shape {
    /// What the tool chest calls it: "W12x26", "L4x4x1/4", "HSS6x6x1/4".
    pub name: String,
    /// Every distinct cutting length, longest first — which is the order a saw
    /// list wants, because the long ones decide what stock to buy.
    pub cuts: Vec<Cut>,
    /// Pounds per foot from the tool chest, when it carried one.
    pub per_foot: Option<f64>,
    /// How many pieces altogether.
    pub pieces: usize,
    /// Total length of all of them, in the primary unit.
    pub total_length: f64,
    /// What the whole lot weighs, when there is a unit weight to work it out
    /// from. `None` is not zero.
    pub pounds: Option<f64>,
}

impl Shape {
    /// The longest piece, which is what decides the stock length to buy.
    pub fn longest(&self) -> f64 {
        self.cuts.first().map(|c| c.length).unwrap_or(0.0)
    }

    /// Every grid location this shape turns up at, in order and without
    /// repeats. Empty when its sheets carry no grid.
    pub fn places(&self) -> Vec<String> {
        let mut all: Vec<String> = self
            .cuts
            .iter()
            .flat_map(|c| c.places.iter().cloned())
            .collect();
        all.sort();
        all.dedup();
        all
    }

    /// Every piece written out one per line, longest first — what the nester
    /// is handed and what a saw operator reads.
    pub fn every_piece(&self) -> Vec<f64> {
        let mut all = Vec::with_capacity(self.pieces);
        for cut in &self.cuts {
            for _ in 0..cut.count {
                all.push(cut.length);
            }
        }
        all
    }
}

/// A whole shop list.
#[derive(Clone, Debug, Default)]
pub struct ShopList {
    pub shapes: Vec<Shape>,
    pub pieces: usize,
    /// What the lot weighs, from the shapes that had a unit weight.
    pub pounds: f64,
    /// Shapes with no unit weight, by name — so the weight above is honest
    /// about what it does not include.
    pub unweighed: Vec<String>,
    /// Measurements left off for want of a scale.
    pub unscaled: usize,
    pub unscaled_pages: Vec<usize>,
}

impl ShopList {
    pub fn tons(&self) -> f64 {
        self.pounds / 2000.0
    }

    /// Whether the weight above is the whole weight.
    pub fn is_whole(&self) -> bool {
        self.unweighed.is_empty() && self.unscaled == 0
    }

    /// True when not one shape on the list carried a unit weight.
    ///
    /// The total is then zero for the worst possible reason, and printing it
    /// as `0 lb` would say the job weighs nothing.
    pub fn nothing_weighed(&self) -> bool {
        !self.shapes.is_empty() && self.unweighed.len() == self.shapes.len()
    }

    /// The total weight in words, which is not always a number.
    ///
    /// A list where nothing carried a unit weight has no weight at all — it
    /// does not weigh zero — and a list where some shapes did and some did
    /// not has a weight that is honestly only part of the job. Both say so
    /// here rather than printing a figure somebody would read as the answer.
    pub fn weight_says(&self) -> String {
        if self.nothing_weighed() {
            return "no weight — no shape on this list has a unit weight in the \
                    tool chest"
                .to_string();
        }
        let figure = format!("{:.0} lb · {:.3} tons", self.pounds, self.tons());
        if self.unweighed.is_empty() {
            figure
        } else {
            format!(
                "{figure} — part only, {} shape{} {} no unit weight",
                self.unweighed.len(),
                if self.unweighed.len() == 1 { "" } else { "s" },
                if self.unweighed.len() == 1 { "has" } else { "have" }
            )
        }
    }

    pub fn shape(&self, name: &str) -> Option<&Shape> {
        self.shapes.iter().find(|s| s.name == name)
    }

    /// What is missing from the total, in a sentence.
    pub fn what_is_missing(&self) -> String {
        let mut said = Vec::new();
        if self.unscaled > 0 {
            said.push(format!(
                "{} measurement{} on {} sheet{} with no scale",
                self.unscaled,
                if self.unscaled == 1 { "" } else { "s" },
                self.unscaled_pages.len(),
                if self.unscaled_pages.len() == 1 { "" } else { "s" }
            ));
        }
        if !self.unweighed.is_empty() {
            said.push(format!(
                "{} shape{} with no unit weight in the tool chest ({})",
                self.unweighed.len(),
                if self.unweighed.len() == 1 { "" } else { "s" },
                self.unweighed.join(", ")
            ));
        }
        if said.is_empty() {
            return String::new();
        }
        format!(
            "NOT IN THE WEIGHT: {}. Nothing here is estimated — what could not be \
             worked out is named rather than guessed at.",
            said.join("; ")
        )
    }
}

/// Rounds a length up to the next step.
///
/// **Up**, always. A beam measured at 24 feet 5 and seven eighths is cut at 24
/// feet 6 inches; a cut length rounded down is a member that arrives short and
/// a crane that waits.
pub fn cut_to(length: f64, step: f64) -> f64 {
    if step <= 0.0 || !step.is_finite() {
        return length;
    }
    // A hair of slack so a length already exactly on a step is not bumped to
    // the next one by the last bit of floating point.
    let steps = (length / step - 1e-9).ceil();
    (steps * step).max(step)
}

/// Builds a shop list from measured rows.
///
/// `step` is the cutting increment in the primary unit: [`INCH`] on an
/// imperial job, 0.01 for centimetres on a metric one.
/// The most places one markup lists for the pieces it stands for. A markup
/// standing for six beams lists its grid place six times, as it always has;
/// one claiming a million lists it this many times, not a million.
pub const MOST_PLACES_EACH: usize = 1000; // WHAT: a thousand. WHY: past any real quantity, short of a list nobody could read or hold.

/// The most pieces of one shape that are nested one by one. Nesting is
/// first-fit, so its work grows with the square of the pieces; past this a
/// shape is not nested and says so (`Nest::unnested`).
pub const MOST_NESTED: usize = 20_000; // WHAT: twenty thousand pieces. WHY: more than a job's whole schedule of one shape, few enough to nest in a moment.

/// One markup's pieces on a cut list: its cut length, how many pieces it
/// stands for, its sheet and its grid place.
type Entry = (f64, usize, usize, String); // WHAT: (length, how many, sheet, place). WHY: named, so the map that holds them reads plainly.

pub fn build(rows: &[Row], weights: WeightColumns, step: f64) -> ShopList {
    // WHAT: each shape's cuts as (length, how many, sheet, place). WHY: a markup standing for several is kept as one entry with its count, never one entry per piece: a markup claiming a billion pieces used to ask for a billion entries and close the program.
    let mut shapes: BTreeMap<String, (Vec<Entry>, Option<f64>)> = BTreeMap::new(); // WHAT: each shape's entries and its unit weight. WHY: gathered below.
    let mut out = ShopList::default();

    for row in rows {
        // Only the kinds that are a piece of stock somebody cuts. An area
        // takeoff is not something the saw cuts, and a diameter is a hole, not
        // a member — putting either on a cut list is how a shop orders the
        // wrong thing.
        if !matches!(row.kind, Kind::Length | Kind::Polylength) {
            continue;
        }
        if !row.scaled {
            out.unscaled += 1;
            if !out.unscaled_pages.contains(&row.page) {
                out.unscaled_pages.push(row.page);
            }
            continue;
        }
        let Some(length) = row.length else { continue };
        if !(length > 0.0) {
            continue;
        }
        let name = if row.subject.trim().is_empty() {
            "(no shape named)".to_string()
        } else {
            row.subject.trim().to_string()
        };
        let entry = shapes.entry(name).or_insert_with(|| (Vec::new(), None));
        // A member standing for several is several pieces to cut.
        let many = row.quantity.round().max(1.0) as usize; // WHAT: how many pieces it stands for. WHY: counted, not written out one by one.
        entry.0.push((cut_to(length, step), many, row.page, row.grid.clone())); // WHAT: one entry, with its count. WHY: as above.
        // The unit weight the tool chest carried, kept the first time it is
        // seen. Two different weights under one shape name is a tool chest
        // problem, and taking the first is at least consistent.
        if entry.1.is_none() {
            if let Some(per) = row.per_length_weight(weights) {
                if per > 0.0 {
                    entry.1 = Some(per);
                }
            }
        }
    }

    for (name, (lengths, per_foot)) in shapes {
        let mut by_length: BTreeMap<u64, (f64, usize, Vec<usize>, Vec<String>)> =
            BTreeMap::new();
        for (length, many, page, place) in &lengths {
            let key = (length * 100_000.0).round() as u64;
            let slot = by_length
                .entry(key)
                .or_insert((*length, 0, Vec::new(), Vec::new()));
            slot.1 = slot.1.saturating_add(*many); // WHAT: all the pieces it stands for. WHY: the same count as one entry per piece gave.
            if !slot.2.contains(page) {
                slot.2.push(*page);
            }
            if !place.trim().is_empty() {
                for _ in 0..(*many).min(MOST_PLACES_EACH) {
                    slot.3.push(place.clone()); // WHAT: its place once per piece, as before, up to the limit. WHY: a list of places can't be made to grow without end.
                }
            }
        }
        let mut cuts: Vec<Cut> = by_length
            .into_values()
            .map(|(length, count, mut pages, mut places)| {
                pages.sort_unstable();
                places.sort();
                Cut {
                    length,
                    count,
                    pages,
                    places,
                }
            })
            .collect();
        // Longest first: that is the order a saw list wants, because the long
        // ones decide what stock to buy.
        cuts.sort_by(|a, b| {
            b.length
                .partial_cmp(&a.length)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let pieces: usize = cuts.iter().fold(0usize, |n, c| n.saturating_add(c.count)); // WHAT: every piece of the shape. WHY: added without overflowing, whatever the counts claim.
        let total_length: f64 = cuts.iter().map(|c| c.length * c.count as f64).sum();
        let pounds = per_foot.map(|per| total_length * per);

        match pounds {
            Some(pounds) => out.pounds += pounds,
            None => out.unweighed.push(name.clone()),
        }
        out.pieces = out.pieces.saturating_add(pieces); // WHAT: added to the whole list's count. WHY: without overflowing.
        out.shapes.push(Shape {
            name,
            cuts,
            per_foot,
            pieces,
            total_length,
            pounds,
        });
    }

    // Heaviest first, then by name: the shapes that decide the tonnage are the
    // ones somebody wants at the top of the page. A shape with no weight sorts
    // as though it weighed nothing, but it is still on the list and still
    // named in `unweighed` — it is the order that treats it as zero, never the
    // arithmetic.
    out.shapes.sort_by(|a, b| {
        b.pounds
            .unwrap_or(0.0)
            .partial_cmp(&a.pounds.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });
    out.unscaled_pages.sort_unstable();
    out
}

// ---- nesting: what to buy, and what falls on the floor ----------------------

/// One stick of stock, and what comes off it.
#[derive(Clone, Debug, PartialEq)]
pub struct Stick {
    /// The pieces cut from it, in the order they come off.
    pub pieces: Vec<f64>,
    /// What is left after the last cut — the drop.
    pub drop: f64,
}

/// What a shape costs in stock.
#[derive(Clone, Debug)]
pub struct Nest {
    pub shape: String,
    /// The stock length this was worked out against. Nothing in here is a
    /// default: it is what somebody typed.
    pub stock: f64,
    /// The saw kerf taken off for each cut.
    pub kerf: f64,
    pub sticks: Vec<Stick>,
    /// Pieces that will not come out of this stock at all, longest first.
    /// They are named rather than nested, because a stick that cannot hold a
    /// piece is not a nesting problem, it is a purchasing one.
    pub too_long: Vec<f64>,
    /// Total stock bought.
    pub bought: f64,
    /// Total length actually used by pieces.
    pub used: f64,
    /// Everything that falls on the floor, kerf included.
    pub waste: f64,
    /// Pieces not nested at all, because the shape has more than
    /// [`MOST_NESTED`]. Zero for any real job; said, never hidden.
    pub unnested: usize, // WHAT: how many pieces were left out of the nesting. WHY: a list too long to nest is reported, not worked through for an hour.
}

impl Nest {
    /// How many sticks to buy.
    pub fn sticks_to_buy(&self) -> usize {
        self.sticks.len()
    }

    /// What fraction of the stock ends up in the building, 0 to 1.
    pub fn yield_of(&self) -> f64 {
        if self.bought <= 0.0 {
            return 0.0;
        }
        self.used / self.bought
    }

    /// The drops long enough to be worth putting back in the rack.
    pub fn keepers(&self, worth_keeping: f64) -> Vec<f64> {
        let mut keep: Vec<f64> = self
            .sticks
            .iter()
            .map(|s| s.drop)
            .filter(|d| *d >= worth_keeping)
            .collect();
        keep.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        keep
    }

    /// Whether every piece found a stick.
    pub fn is_whole(&self) -> bool {
        self.too_long.is_empty() && self.unnested == 0 // WHAT: nothing too long and nothing left out. WHY: either means some pieces are on no stick.
    }
}

/// Nests one shape's pieces into sticks of stock.
///
/// First-fit-decreasing: longest piece first, into the first stick it fits.
/// It is not the theoretical optimum — bin packing has no cheap optimum — but
/// it is what a saw operator does by hand, it never overfills a stick, and it
/// is stable, so the same cut list nests the same way twice. Every number that
/// comes out of it is the sum of real cuts and real kerfs; nothing here is a
/// yield factor or an allowance.
///
/// `stock` and `kerf` are in the same unit as the cuts. There is no default
/// stock length: the caller has to have been told one.
pub fn nest(shape: &Shape, stock: f64, kerf: f64) -> Nest {
    let kerf = kerf.max(0.0);
    let mut out = Nest {
        shape: shape.name.clone(),
        stock,
        kerf,
        sticks: Vec::new(),
        too_long: Vec::new(),
        bought: 0.0,
        used: 0.0,
        waste: 0.0,
        unnested: 0,
    };
    if shape.pieces > MOST_NESTED {
        out.unnested = shape.pieces; // WHAT: none nested, and how many. WHY: too many to nest one by one; said rather than tried.
        return out;
    }
    if !(stock > 0.0) {
        // Nothing is nested until somebody says what the stock is.
        out.too_long = shape.every_piece();
        return out;
    }

    // Longest first. `every_piece` already comes out longest first, because
    // the cuts are sorted that way.
    for piece in shape.every_piece() {
        if piece > stock {
            out.too_long.push(piece);
            continue;
        }
        // The first stick it fits in. A piece after the first on a stick costs
        // a kerf as well as its own length.
        let mut landed = false;
        for stick in out.sticks.iter_mut() {
            let cost = if stick.pieces.is_empty() { piece } else { piece + kerf };
            if stick.drop + 1e-9 >= cost {
                stick.drop -= cost;
                stick.pieces.push(piece);
                landed = true;
                break;
            }
        }
        if !landed {
            out.sticks.push(Stick {
                pieces: vec![piece],
                drop: stock - piece,
            });
        }
    }

    out.bought = stock * out.sticks.len() as f64;
    out.used = out
        .sticks
        .iter()
        .map(|s| s.pieces.iter().sum::<f64>())
        .sum();
    out.waste = out.bought - out.used;
    out
}

/// Nests every shape on a list against one stock length.
///
/// Shapes are bought in different lengths in real life, so this is the simple
/// case: one length for the lot. The app hands a length per shape when
/// somebody has set one.
pub fn nest_all(list: &ShopList, stock: f64, kerf: f64) -> Vec<Nest> {
    let mut left = MOST_NESTED_IN_ALL; // WHAT: how many pieces the whole list may still have nested. WHY: the limit per shape alone left sixty shapes of twenty thousand, minutes of work.
    list.shapes
        .iter()
        .map(|shape| {
            if shape.pieces > left {
                return Nest {
                    shape: shape.name.clone(), // WHAT: the shape's name. WHY: answered for, with nothing tried.
                    stock,
                    kerf: kerf.max(0.0),
                    sticks: Vec::new(),
                    too_long: Vec::new(),
                    bought: 0.0,
                    used: 0.0,
                    waste: 0.0,
                    unnested: shape.pieces, // WHAT: all of its pieces, said to be left out. WHY: never hidden, as for one shape too big to nest.
                };
            }
            left -= shape.pieces; // WHAT: this shape's pieces taken off what is left. WHY: the next shapes get the rest.
            nest(shape, stock, kerf) // WHAT: nested one by one. WHY: within the whole list's budget.
        })
        .collect()
}

/// The most pieces one list has nested one by one, all its shapes together.
pub const MOST_NESTED_IN_ALL: usize = 50_000; // WHAT: fifty thousand. WHY: more than a whole job's members; past it the remaining shapes are said to be un-nested rather than worked through for minutes.

/// What somebody has told the program about how they buy steel and cut it.
///
/// Every number in here is typed by a person. There is deliberately no default
/// stock length: "twenty feet" and "forty feet" and "sixty feet" are all
/// ordinary, they differ by shape and by supplier, and a cut list that guessed
/// would be a cut list that quietly bought the wrong steel.
#[derive(Clone, Copy, Debug)]
pub struct Buying {
    /// The cutting increment lengths are rounded up to.
    pub step: f64,
    /// The stock length bought. Zero means nobody has said, and nothing is
    /// nested until somebody does.
    pub stock: f64,
    /// The saw kerf, taken off once for every cut after the first on a stick.
    pub kerf: f64,
    /// The shortest drop worth putting back in the rack.
    pub worth_keeping: f64,
}

impl Default for Buying {
    fn default() -> Buying {
        Buying {
            step: INCH,
            stock: 0.0,
            kerf: INCH / 8.0,
            worth_keeping: 2.0,
        }
    }
}

impl Buying {
    /// Whether there is enough here to nest against.
    pub fn can_nest(&self) -> bool {
        self.stock > 0.0
    }
}

// ---- writing it out ---------------------------------------------------------

fn quote(field: &str) -> String {
    crate::export::csv_cell(field) // WHAT: quoted, and a formula written as text. WHY: a subject, a shape or a place is whatever somebody typed, and a spreadsheet must not run it.
}

/// The cut list as a spreadsheet: one line per cutting length.
///
/// The warnings ride along at the bottom as comment lines. A spreadsheet that
/// arrives without them is a spreadsheet somebody bids off without knowing six
/// members were left out.
pub fn to_csv(list: &ShopList, unit: &str, sheet: &dyn Fn(usize) -> String) -> String {
    let mut out = format!(
        "Shape,Cut length ({unit}),Pieces,Total length ({unit}),lb per {unit},Pounds,Sheets,Where\n"
    );
    for shape in &list.shapes {
        for cut in &shape.cuts {
            let sheets: Vec<String> = cut.pages.iter().map(|p| sheet(*p)).collect();
            out.push_str(&format!(
                "{},{:.4},{},{:.4},{},{},{},{}\n",
                quote(&shape.name),
                cut.length,
                cut.count,
                cut.length * cut.count as f64,
                match shape.per_foot {
                    Some(per) => format!("{per}"),
                    None => String::new(),
                },
                match shape.per_foot {
                    Some(per) => format!("{:.2}", per * cut.length * cut.count as f64),
                    None => String::new(),
                },
                quote(&sheets.join(" ")),
                // Empty when the sheet has no grid. A blank here is "the
                // drawing does not say", not "nowhere".
                quote(&cut.places.join(" ")),
            ));
        }
        out.push_str(&format!(
            "{},,{},{:.4},,{},,\n",
            quote(&format!("{} — all", shape.name)),
            shape.pieces,
            shape.total_length,
            match shape.pounds {
                Some(pounds) => format!("{pounds:.2}"),
                // Empty, never zero: the difference is the whole argument.
                None => String::new(),
            },
        ));
    }
    // No figure at all when nothing carried a unit weight: a `0.00` in this
    // cell is a spreadsheet that says the job weighs nothing.
    let (pounds, tons) = if list.nothing_weighed() {
        (String::new(), String::new())
    } else {
        (format!("{:.2}", list.pounds), format!("{:.4}", list.tons()))
    };
    out.push_str(&format!(
        "{},,{},,,{pounds},,\n",
        quote("TOTAL"),
        list.pieces
    ));
    out.push_str(&format!("{},,,,,{tons},,\n", quote("TONS")));
    let missing = list.what_is_missing();
    if !missing.is_empty() {
        out.push_str(&format!("\n# {missing}\n"));
    }
    if list.unscaled > 0 {
        let sheets: Vec<String> = list.unscaled_pages.iter().map(|p| sheet(*p)).collect();
        out.push_str(&format!(
            "# Sheets with no scale set: {}. Set a scale on them and take this list again.\n",
            sheets.join(", ")
        ));
    }
    out
}

/// The nesting as a spreadsheet: one line per stick of stock.
pub fn nest_csv(nests: &[Nest], unit: &str) -> String {
    let mut out = format!(
        "Shape,Stick,Pieces on it,Cut lengths ({unit}),Used ({unit}),Drop ({unit})\n"
    );
    for nest in nests {
        for (at, stick) in nest.sticks.iter().enumerate() {
            let lengths: Vec<String> =
                stick.pieces.iter().map(|p| format!("{p:.4}")).collect();
            let used: f64 = stick.pieces.iter().sum();
            out.push_str(&format!(
                "{},{},{},{},{:.4},{:.4}\n",
                quote(&nest.shape),
                at + 1,
                stick.pieces.len(),
                quote(&lengths.join(" + ")),
                used,
                stick.drop
            ));
        }
        out.push_str(&format!(
            "{},{},,,{:.4},{:.4}\n",
            quote(&format!("{} — all", nest.shape)),
            nest.sticks_to_buy(),
            nest.used,
            nest.waste
        ));
        for long in &nest.too_long {
            out.push_str(&format!(
                "{},,,{:.4},,\n",
                quote(&format!(
                    "{} — WILL NOT COME OUT OF {:.2}{unit} STOCK",
                    nest.shape, nest.stock
                )),
                long
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_row(subject: &str, feet: f64, page: usize, per_foot: Option<f64>) -> Row {
        let mut row = Row::blank();
        row.page = page;
        row.subject = subject.into();
        row.kind = Kind::Length;
        row.length = Some(feet);
        row.scaled = true;
        row.unit = "'".into();
        if let Some(per) = per_foot {
            row.columns[WeightColumns::default().per_length] = format!("{per}");
        }
        row
    }

    fn at(subject: &str, feet: f64, place: &str) -> Row {
        let mut row = a_row(subject, feet, 0, Some(26.0));
        row.grid = place.into();
        row
    }

    #[test]
    fn a_cut_length_always_rounds_up() {
        // A cut length rounded down is a member that arrives short and a crane
        // that waits.
        assert!((cut_to(24.49, INCH) - 24.5).abs() < 1e-9);
        assert!((cut_to(24.5, INCH) - 24.5).abs() < 1e-9, "already on a step stays there");
        assert!(cut_to(24.5001, INCH) > 24.5);
        assert!((cut_to(24.5001, INCH) - 24.5 - INCH).abs() < 1e-9);
        assert!((cut_to(0.001, INCH) - INCH).abs() < 1e-9, "nothing rounds to nothing");
    }

    #[test]
    fn the_same_shape_at_the_same_length_is_one_line_with_a_count() {
        let rows = vec![
            a_row("W12x26", 20.0, 0, Some(26.0)),
            a_row("W12x26", 20.0, 0, Some(26.0)),
            a_row("W12x26", 20.0, 1, Some(26.0)),
        ];
        let list = build(&rows, WeightColumns::default(), INCH);
        assert_eq!(list.shapes.len(), 1);
        assert_eq!(list.shapes[0].cuts.len(), 1);
        assert_eq!(list.shapes[0].cuts[0].count, 3);
        assert_eq!(list.shapes[0].cuts[0].pages, vec![0, 1]);
        assert_eq!(list.pieces, 3);
    }

    #[test]
    fn the_longest_piece_comes_first_because_it_decides_the_stock() {
        let rows = vec![
            a_row("W12x26", 10.0, 0, Some(26.0)),
            a_row("W12x26", 40.0, 0, Some(26.0)),
            a_row("W12x26", 20.0, 0, Some(26.0)),
        ];
        let list = build(&rows, WeightColumns::default(), INCH);
        let lengths: Vec<f64> = list.shapes[0].cuts.iter().map(|c| c.length).collect();
        assert_eq!(lengths, vec![40.0, 20.0, 10.0]);
        assert!((list.shapes[0].longest() - 40.0).abs() < 1e-9);
    }

    #[test]
    fn a_shape_with_no_unit_weight_is_listed_and_weighs_nothing_rather_than_zero() {
        // Guessing a section's weight from its name is exactly the kind of
        // thing that makes a bid wrong in a way nobody catches.
        let rows = vec![a_row("SOMETHING ODD", 20.0, 0, None)];
        let list = build(&rows, WeightColumns::default(), INCH);
        assert_eq!(list.shapes.len(), 1);
        assert!(list.shapes[0].pounds.is_none());
        assert_eq!(list.pounds, 0.0);
        assert_eq!(list.unweighed, vec!["SOMETHING ODD".to_string()]);
        assert!(!list.is_whole());
        assert!(list.what_is_missing().contains("SOMETHING ODD"));
    }

    #[test]
    fn a_measurement_off_an_unscaled_sheet_is_not_on_the_list_and_is_reported() {
        let mut short = a_row("W12x26", 20.0, 4, Some(26.0));
        short.scaled = false;
        let rows = vec![a_row("W12x26", 20.0, 0, Some(26.0)), short];
        let list = build(&rows, WeightColumns::default(), INCH);
        assert_eq!(list.pieces, 1, "only the one that could be measured");
        assert_eq!(list.unscaled, 1);
        assert_eq!(list.unscaled_pages, vec![4]);
        assert!(list.what_is_missing().contains("no scale"));
    }

    #[test]
    fn the_weight_is_worked_out_from_the_chests_own_pounds_per_foot() {
        // Twenty feet of a twenty-six pound section is five hundred and twenty
        // pounds, and it comes from his chest rather than from a table this
        // program looked up.
        let rows = vec![a_row("W12x26", 20.0, 0, Some(26.0))];
        let list = build(&rows, WeightColumns::default(), INCH);
        assert!((list.pounds - 520.0).abs() < 0.001, "{}", list.pounds);
        assert!((list.tons() - 0.26).abs() < 0.0001);
        assert!(list.is_whole());
        assert_eq!(list.what_is_missing(), "");
    }

    #[test]
    fn an_area_takeoff_is_not_on_a_cut_list() {
        // Square feet on a saw list is how a shop orders the wrong thing.
        let mut area = Row::blank();
        area.subject = "SLAB".into();
        area.kind = Kind::Area;
        area.scaled = true;
        area.area = Some(900.0);
        let list = build(&[area], WeightColumns::default(), INCH);
        assert!(list.shapes.is_empty());
    }

    #[test]
    fn the_heaviest_shape_is_at_the_top() {
        let rows = vec![
            a_row("L4x4", 20.0, 0, Some(6.0)),
            a_row("W12x26", 20.0, 0, Some(26.0)),
        ];
        let list = build(&rows, WeightColumns::default(), INCH);
        assert_eq!(list.shapes[0].name, "W12x26");
    }

    #[test]
    fn a_markup_with_no_shape_named_is_still_on_the_list() {
        // Dropping it would be a quantity that vanished; naming it is the
        // honest answer.
        let rows = vec![a_row("", 20.0, 0, None)];
        let list = build(&rows, WeightColumns::default(), INCH);
        assert_eq!(list.shapes.len(), 1);
        assert!(list.shapes[0].name.contains("no shape named"));
    }

    // ---- nesting ----------------------------------------------------------

    fn shape_of(name: &str, pieces: &[f64]) -> Shape {
        let rows: Vec<Row> = pieces
            .iter()
            .map(|p| a_row(name, *p, 0, Some(26.0)))
            .collect();
        build(&rows, WeightColumns::default(), INCH)
            .shapes
            .remove(0)
    }

    #[test]
    fn three_twenties_come_out_of_one_sixty_foot_stick_less_the_kerf() {
        // 20 + 20 + 20 is 60 exactly, but two cuts take two kerfs, so it does
        // not fit — and the nester is not allowed to pretend it does.
        let shape = shape_of("W12x26", &[20.0, 20.0, 20.0]);
        let tight = nest(&shape, 60.0, INCH / 8.0);
        assert_eq!(tight.sticks_to_buy(), 2, "the kerf is real steel");
        // Give it the room the kerfs need and it is one stick.
        let roomy = nest(&shape, 60.25, INCH / 8.0);
        assert_eq!(roomy.sticks_to_buy(), 1);
        assert_eq!(roomy.sticks[0].pieces.len(), 3);
    }

    #[test]
    fn nothing_is_nested_until_somebody_says_what_the_stock_is() {
        // No mill length is assumed anywhere in this program.
        let shape = shape_of("W12x26", &[20.0, 20.0]);
        let nothing = nest(&shape, 0.0, 0.0);
        assert_eq!(nothing.sticks_to_buy(), 0);
        assert_eq!(nothing.too_long.len(), 2);
        assert!(!nothing.is_whole());
    }

    #[test]
    fn a_piece_longer_than_the_stock_is_named_not_nested() {
        let shape = shape_of("W12x26", &[70.0, 20.0]);
        let it = nest(&shape, 60.0, 0.0);
        assert_eq!(it.too_long, vec![70.0]);
        assert_eq!(it.sticks_to_buy(), 1, "the twenty still gets a stick");
        assert!(!it.is_whole());
    }

    #[test]
    fn the_drop_and_the_yield_add_up_to_what_was_bought() {
        let shape = shape_of("W12x26", &[24.0, 18.0, 12.0, 10.0]);
        let it = nest(&shape, 40.0, 0.0);
        assert!((it.bought - 40.0 * it.sticks_to_buy() as f64).abs() < 1e-9);
        assert!((it.used - 64.0).abs() < 1e-9);
        assert!((it.waste - (it.bought - it.used)).abs() < 1e-9);
        assert!(it.yield_of() > 0.0 && it.yield_of() <= 1.0);
        // Every stick holds what it says it holds, and never more.
        for stick in &it.sticks {
            let on_it: f64 = stick.pieces.iter().sum();
            assert!(on_it <= 40.0 + 1e-9, "a stick was overfilled: {on_it}");
            assert!((on_it + stick.drop - 40.0).abs() < 1e-9);
        }
    }

    #[test]
    fn a_drop_worth_keeping_goes_back_in_the_rack() {
        let shape = shape_of("W12x26", &[24.0, 10.0]);
        let it = nest(&shape, 40.0, 0.0);
        // 24 and 10 on one forty foot stick leaves six feet.
        assert_eq!(it.sticks_to_buy(), 1);
        assert!((it.sticks[0].drop - 6.0).abs() < 1e-9);
        assert_eq!(it.keepers(4.0).len(), 1);
        assert!(it.keepers(8.0).is_empty(), "short ends are not keepers");
    }

    #[test]
    fn the_same_list_nests_the_same_way_twice() {
        let shape = shape_of("W12x26", &[13.0, 27.0, 9.0, 31.0, 5.0, 18.0]);
        let once = nest(&shape, 40.0, INCH / 8.0);
        let twice = nest(&shape, 40.0, INCH / 8.0);
        assert_eq!(once.sticks, twice.sticks);
    }

    #[test]
    fn every_piece_that_was_measured_comes_out_of_a_stick_somewhere() {
        // The one thing a nester must never do is lose a member.
        let pieces = [13.0, 27.0, 9.0, 31.0, 5.0, 18.0, 40.0];
        let shape = shape_of("W12x26", &pieces);
        let it = nest(&shape, 40.0, INCH / 8.0);
        let mut back: Vec<f64> = it
            .sticks
            .iter()
            .flat_map(|s| s.pieces.iter().copied())
            .chain(it.too_long.iter().copied())
            .collect();
        back.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut want = pieces.to_vec();
        want.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(back.len(), want.len());
        for (got, want) in back.iter().zip(&want) {
            assert!((got - want).abs() < 1e-9, "{got} vs {want}");
        }
    }

    #[test]
    fn a_markup_claiming_a_billion_pieces_is_counted_not_written_out() {
        // WHAT: one beam standing for a billion. WHY: written out one entry per piece, it asked for gigabytes and closed the program, on a seat or on the office server.
        let mut row = a_row("W12x26", 20.0, 0, Some(26.0)); // WHAT: a measured beam. WHY: a row as the takeoff makes one.
        row.quantity = 1e9; // WHAT: a billion of it. WHY: what a markup's quantity can say.
        let list = build(&[row], crate::WeightColumns::default(), INCH); // WHAT: the cut list. WHY: must come back at once.
        assert_eq!(list.pieces, 1_000_000_000, "every piece is still counted");
        let nest = nest(&list.shapes[0], 40.0, INCH / 8.0); // WHAT: nested against forty-foot stock. WHY: must not try a billion.
        assert_eq!(nest.unnested, 1_000_000_000);
        assert!(!nest.is_whole(), "a shape left un-nested is not a whole nesting");
        assert!(nest.sticks.is_empty() && nest.too_long.is_empty());
    }

    #[test]
    fn many_shapes_each_just_under_the_limit_are_not_all_nested_one_by_one() {
        // WHAT: sixty shapes, each twenty thousand pieces just over half a stick long. WHY: each piece then needs a stick of its own and is tried against every stick before it; the limit per shape left sixty of those, minutes of work, for anybody who can draw sixty markups.
        let rows: Vec<Row> = (0..60)
            .map(|n| {
                let mut row = a_row(&format!("W{n}x26"), 24.0, 0, Some(26.0)); // WHAT: a beam of its own shape. WHY: one shape each.
                row.quantity = MOST_NESTED as f64; // WHAT: as many as one shape may have nested. WHY: the most each is allowed.
                row // WHAT: the beam. WHY: one per shape.
            })
            .collect();
        let list = build(&rows, crate::WeightColumns::default(), INCH); // WHAT: the cut list. WHY: what is nested.
        let started = std::time::Instant::now(); // WHAT: now. WHY: timed.
        let nests = nest_all(&list, 40.0, INCH / 8.0); // WHAT: every shape nested. WHY: what the server's assistant and the shop window do.
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "nesting took {:?}", started.elapsed());
        assert_eq!(nests.len(), 60, "every shape is still answered for");
        assert!(nests.iter().any(|n| n.unnested > 0), "some shapes are said to be left un-nested");
        assert!(nests.iter().map(|n| n.unnested).sum::<usize>() + nests.iter().map(|n| n.sticks.iter().map(|s| s.pieces.len()).sum::<usize>()).sum::<usize>() == list.pieces, "every piece is either nested or said not to be");
    }

    // ---- writing it out ---------------------------------------------------

    #[test]
    fn the_spreadsheet_carries_the_warning_with_it() {
        // A spreadsheet that arrives without the warning is one somebody bids
        // off without knowing six members were left out.
        let mut short = a_row("W12x26", 20.0, 4, Some(26.0));
        short.scaled = false;
        let rows = vec![
            a_row("W12x26", 20.0, 0, Some(26.0)),
            a_row("SOMETHING ODD", 8.0, 0, None),
            short,
        ];
        let list = build(&rows, WeightColumns::default(), INCH);
        let csv = to_csv(&list, "'", &|p| format!("S-{}", p + 1));
        assert!(csv.contains("NOT IN THE WEIGHT"));
        assert!(csv.contains("SOMETHING ODD"));
        assert!(csv.contains("S-5"), "the unscaled sheet is named: {csv}");
        assert!(csv.contains("TOTAL"));
        assert!(csv.contains("520.00"));
    }

    #[test]
    fn a_shape_with_no_weight_has_an_empty_cell_and_never_a_zero() {
        let list = build(
            &[a_row("SOMETHING ODD", 8.0, 0, None)],
            WeightColumns::default(),
            INCH,
        );
        let csv = to_csv(&list, "'", &|p| format!("{}", p + 1));
        let all = csv
            .lines()
            .find(|l| l.starts_with("SOMETHING ODD — all"))
            .unwrap_or_default()
            .to_string();
        assert!(all.ends_with(",,"), "weight cell must be empty: {all:?}");
        assert!(!all.contains("0.00"), "never a zero: {all:?}");
    }

    #[test]
    fn a_subject_with_a_comma_does_not_break_the_spreadsheet() {
        let list = build(
            &[a_row("W12x26, GALV", 20.0, 0, Some(26.0))],
            WeightColumns::default(),
            INCH,
        );
        let csv = to_csv(&list, "'", &|p| format!("{}", p + 1));
        assert!(csv.contains("\"W12x26, GALV\""));
    }

    #[test]
    fn the_nesting_spreadsheet_names_what_will_not_fit() {
        let shape = shape_of("W12x26", &[70.0, 20.0]);
        let csv = nest_csv(&[nest(&shape, 60.0, 0.0)], "'");
        assert!(csv.contains("WILL NOT COME OUT OF"));
        assert!(csv.contains("70.0000"));
    }

    #[test]
    fn a_list_where_nothing_was_weighed_has_no_weight_rather_than_a_weight_of_zero() {
        // `0 lb` at the bottom of a cut list says the job weighs nothing, which
        // is the most expensive kind of wrong this program could be.
        let list = build(
            &[a_row("SOMETHING ODD", 8.0, 0, None)],
            WeightColumns::default(),
            INCH,
        );
        assert!(list.nothing_weighed());
        assert!(list.weight_says().contains("no weight"));
        assert!(!list.weight_says().contains("0 lb"));
        let csv = to_csv(&list, "'", &|p| format!("{}", p + 1));
        let total = csv.lines().find(|l| l.starts_with("TOTAL")).unwrap();
        assert!(!total.contains("0.00"), "{total:?}");
        let tons = csv.lines().find(|l| l.starts_with("TONS")).unwrap();
        assert!(!tons.contains("0.0000"), "{tons:?}");
    }

    #[test]
    fn a_partly_weighed_list_says_the_total_is_only_part_of_the_job() {
        let list = build(
            &[
                a_row("W12x26", 20.0, 0, Some(26.0)),
                a_row("SOMETHING ODD", 8.0, 0, None),
            ],
            WeightColumns::default(),
            INCH,
        );
        assert!(!list.nothing_weighed());
        let says = list.weight_says();
        assert!(says.contains("520 lb"), "{says}");
        assert!(says.contains("part only"), "{says}");
    }

    #[test]
    fn a_cut_line_says_where_on_the_grid_the_pieces_are() {
        // "Three at 24'-6"" is a purchase order. "Three at 24'-6", at B-4,
        // C-4 and D-4" is an instruction somebody can carry out.
        let rows = vec![
            at("W12x26", 24.5, "B-4"),
            at("W12x26", 24.5, "C-4"),
            at("W12x26", 24.5, "D-4"),
        ];
        let list = build(&rows, WeightColumns::default(), INCH);
        let cut = &list.shapes[0].cuts[0];
        assert_eq!(cut.count, 3);
        assert_eq!(cut.places, vec!["B-4", "C-4", "D-4"]);
        assert_eq!(list.shapes[0].places(), vec!["B-4", "C-4", "D-4"]);
        let csv = to_csv(&list, "'", &|p| format!("{}", p + 1));
        assert!(csv.contains("B-4 C-4 D-4"), "{csv}");
        assert!(csv.contains("Where"));
    }

    #[test]
    fn a_sheet_with_no_grid_leaves_the_where_column_empty_rather_than_guessing() {
        let rows = vec![a_row("W12x26", 24.5, 0, Some(26.0))];
        let list = build(&rows, WeightColumns::default(), INCH);
        assert!(list.shapes[0].cuts[0].places.is_empty());
        assert!(list.shapes[0].places().is_empty());
    }

    #[test]
    fn nobody_is_given_a_default_mill_length() {
        // There is no such thing as a standard stock length this program is
        // entitled to guess at.
        let buying = Buying::default();
        assert_eq!(buying.stock, 0.0);
        assert!(!buying.can_nest());
        assert!((buying.step - INCH).abs() < 1e-12);
    }
}
