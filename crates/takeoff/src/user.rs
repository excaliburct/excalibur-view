//! The markups list's own columns: what they are called, and what the formula
//! ones work out.
//!
//! A tool chest carries six slots of its own on every markup — "LBS Per FT",
//! "Col Height", "Piece Mark" — and the profile says what each is called and,
//! for some, how to work it out: `[Length] * [LBS Per FT]`. Revu shows the
//! worked-out number and throws the working away; a spreadsheet exported from
//! it has the number and not how it was got. Here the formula is worked out the
//! same way, from the same measured values, and nothing is invented: a formula
//! that refers to something the markup does not have gives no value, not zero.

use crate::Row;

/// One of the six, named.
#[derive(Clone, Debug, PartialEq)]
pub struct UserColumn {
    pub index: usize,
    pub name: String,
    /// `Some` for a formula column.
    pub formula: Option<String>,
    pub precision: usize,
}

impl UserColumn {
    pub fn plain(index: usize, name: &str) -> UserColumn {
        UserColumn {
            index,
            name: name.into(),
            formula: None,
            precision: 2,
        }
    }
}

/// Which slots carry the unit weights, read from what the columns are called.
///
/// Every office names them its own way — "LBS Per FT", "lb/ft", "PLF",
/// "Weight (lbs/ft)" — and puts them in whichever slot it likes. Found by
/// name, so another company's chest weighs its steel with its own weights;
/// the first and last slots, which is where they sit in the chests this was
/// built against, only when nothing is called anything recognisable.
pub fn weight_columns(columns: &[UserColumn]) -> crate::WeightColumns {
    const PER_LENGTH: &[&str] = &[
        "LBSPERFT", "LBSFT", "LBFT", "PLF", "WEIGHTPERFOOT", "WEIGHTPERFT", "WEIGHTLBSFT",
        "POUNDSPERFOOT", "LBSPERFOOT", "WTFT", "UNITWEIGHTLBFT", "LBPERFT",
    ];
    const PER_AREA: &[&str] = &[
        "LBSPERSQFT", "LBSSQFT", "LBSF", "PSF", "WEIGHTPERSQFT", "WEIGHTLBSSQFT",
        "POUNDSPERSQFT", "LBSPERSQUAREFOOT", "WTSF", "UNITWEIGHTPSF", "LBPERSQFT",
    ];
    let norm = |s: &str| -> String {
        s.to_uppercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
    };
    let find = |names: &[&str]| {
        columns
            .iter()
            .filter(|c| c.formula.as_deref().is_none_or(|f| f.trim().is_empty()))
            .find(|c| names.contains(&norm(&c.name).as_str()))
            .map(|c| c.index)
    };
    let default = crate::WeightColumns::default();
    crate::WeightColumns {
        per_length: find(PER_LENGTH).unwrap_or(default.per_length),
        per_area: find(PER_AREA).unwrap_or(default.per_area),
    }
}

/// What a slot is called: its own name, or "Custom 1".
pub fn name_of(columns: &[UserColumn], index: usize) -> String {
    columns
        .iter()
        .find(|c| c.index == index)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| format!("Custom {}", index + 1))
}

/// The slot as a number: typed in, or worked out.
pub fn value(row: &Row, columns: &[UserColumn], index: usize) -> Option<f64> {
    let budget = std::cell::Cell::new(MOST_LOOKUPS); // WHAT: how many more columns this one value may look at. WHY: shared by every level below it.
    value_at(row, columns, index, 0, &budget) // WHAT: worked out from the top level. WHY: with the whole budget.
}

/// The most columns one value may look at, all the levels together. A
/// formula naming a column twice looks at it twice, so nine levels of a
/// formula that names itself twelve times was twelve to the ninth lookups.
const MOST_LOOKUPS: usize = 10_000; // WHAT: ten thousand. WHY: far past any real sheet of formulas; one value can't take minutes.

fn value_at(row: &Row, columns: &[UserColumn], index: usize, depth: usize, budget: &std::cell::Cell<usize>) -> Option<f64> {
    // A formula that refers to itself, however indirectly, has no value.
    if depth > 8 {
        return None;
    }
    let left = budget.get().checked_sub(1)?; // WHAT: one lookup spent, or none left. WHY: past the budget the value is no value, as for a formula that refers to itself.
    budget.set(left); // WHAT: spent. WHY: the levels below see what is left.
    match columns.iter().find(|c| c.index == index).and_then(|c| c.formula.as_deref()) {
        Some(formula) if !formula.trim().is_empty() => {
            let mut look = |name: &str| reference(row, columns, name, depth + 1, budget);
            evaluate(formula, &mut look)
        }
        _ => number(row.columns.get(index)?),
    }
}

/// The slot as it reads in the list.
pub fn text(row: &Row, columns: &[UserColumn], index: usize) -> String {
    let formula = columns
        .iter()
        .find(|c| c.index == index)
        .filter(|c| c.formula.as_deref().is_some_and(|f| !f.trim().is_empty()));
    match formula {
        Some(column) => value(row, columns, index)
            .map(|v| format!("{v:.*}", column.precision.min(6)))
            .unwrap_or_default(),
        None => row.columns.get(index).cloned().unwrap_or_default(),
    }
}

/// What a name in a formula stands for, for this markup.
fn reference(row: &Row, columns: &[UserColumn], name: &str, depth: usize, budget: &std::cell::Cell<usize>) -> Option<f64> {
    let key: String = name.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    // Measured values only count on a sheet with a scale, as everywhere else.
    let measured = |v: Option<f64>| if row.scaled { v } else { None };
    match key.as_str() {
        "length" => measured(row.length),
        "area" => measured(row.area),
        "volume" => measured(row.volume),
        "perimeter" => measured(row.perimeter),
        "depth" => measured(row.depth),
        "wallarea" => measured(row.wall_area),
        "count" => Some(if row.count == 0.0 { 1.0 } else { row.count }),
        "radius" => measured(row.radius),
        "diameter" => measured(row.diameter),
        "angle" => row.angle,
        "slope" => row.slope,
        _ => {
            let column = columns.iter().find(|c| {
                c.name.chars().filter(|ch| ch.is_alphanumeric()).collect::<String>().to_lowercase()
                    == key
            })?;
            value_at(row, columns, column.index, depth, budget) // WHAT: the named column's value. WHY: from what is left of the budget.
        }
    }
}

fn number(text: &str) -> Option<f64> {
    let cleaned: String = text
        .trim()
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    cleaned.parse().ok()
}

/// Works out `[A] * [B] + 2` and the like: numbers, names in square brackets,
/// `+ - * /` and brackets. Anything it cannot read is no value.
pub fn evaluate(formula: &str, look: &mut dyn FnMut(&str) -> Option<f64>) -> Option<f64> {
    let tokens = tokens(formula)?;
    if too_deep(&tokens) {
        return None; // WHAT: a formula nested past any real one has no value. WHY: it is read recursively, and a tool chest must not choose how deep the stack goes.
    }
    let mut at = 0;
    let value = sum(&tokens, &mut at, look)?;
    (at == tokens.len() && value.is_finite()).then_some(value)
}

/// The deepest a formula's brackets, or a run of signs, may go.
const DEEPEST: usize = 64; // WHAT: sixty-four. WHY: no column formula comes near it; a hundred thousand ran a seat or the server out of stack.

/// Whether a formula nests deeper than [`DEEPEST`]: brackets inside brackets,
/// or signs one after another (`- - - 1`), each of which is read by a call
/// inside the last.
fn too_deep(tokens: &[Token]) -> bool {
    let (mut open, mut signs) = (0usize, 0usize); // WHAT: brackets open now, and signs in a row. WHY: either is a level of reading.
    for token in tokens {
        match token {
            Token::Open => open += 1, // WHAT: one bracket deeper. WHY: counted.
            Token::Close => open = open.saturating_sub(1), // WHAT: one out. WHY: counted.
            _ => {}
        }
        signs = if matches!(token, Token::Op(_)) { signs + 1 } else { 0 }; // WHAT: the run of signs so far. WHY: a sign before a sign is read one inside the other.
        if open > DEEPEST || signs > DEEPEST {
            return true; // WHAT: too deep. WHY: refused before reading.
        }
    }
    false
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Name(String),
    Op(char),
    Open,
    Close,
}

fn tokens(text: &str) -> Option<Vec<Token>> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => i += 1,
            '[' => {
                let end = chars[i..].iter().position(|c| *c == ']')? + i;
                out.push(Token::Name(chars[i + 1..end].iter().collect()));
                i = end + 1;
            }
            '+' | '-' | '*' | '/' => {
                out.push(Token::Op(c));
                i += 1;
            }
            '(' => {
                out.push(Token::Open);
                i += 1;
            }
            ')' => {
                out.push(Token::Close);
                i += 1;
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                out.push(Token::Number(chars[start..i].iter().collect::<String>().parse().ok()?));
            }
            _ => return None,
        }
    }
    Some(out)
}

fn sum(t: &[Token], at: &mut usize, look: &mut dyn FnMut(&str) -> Option<f64>) -> Option<f64> {
    let mut value = product(t, at, look)?;
    while let Some(Token::Op(op @ ('+' | '-'))) = t.get(*at) {
        *at += 1;
        let next = product(t, at, look)?;
        value = if *op == '+' { value + next } else { value - next };
    }
    Some(value)
}

fn product(t: &[Token], at: &mut usize, look: &mut dyn FnMut(&str) -> Option<f64>) -> Option<f64> {
    let mut value = atom(t, at, look)?;
    while let Some(Token::Op(op @ ('*' | '/'))) = t.get(*at) {
        *at += 1;
        let next = atom(t, at, look)?;
        value = if *op == '*' {
            value * next
        } else if next == 0.0 {
            return None;
        } else {
            value / next
        };
    }
    Some(value)
}

fn atom(t: &[Token], at: &mut usize, look: &mut dyn FnMut(&str) -> Option<f64>) -> Option<f64> {
    let token = t.get(*at)?.clone();
    *at += 1;
    match token {
        Token::Number(n) => Some(n),
        Token::Name(name) => look(&name),
        Token::Op('-') => atom(t, at, look).map(|v| -v),
        Token::Open => {
            let v = sum(t, at, look)?;
            (t.get(*at) == Some(&Token::Close)).then(|| *at += 1)?;
            Some(v)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn his_columns() -> Vec<UserColumn> {
        vec![
            UserColumn::plain(0, "LBS Per FT"),
            UserColumn {
                index: 1,
                name: "TTL Length LBS".into(),
                formula: Some("[Length] * [LBS Per FT]".into()),
                precision: 2,
            },
            UserColumn {
                index: 2,
                name: "TTL Height".into(),
                formula: Some("[Col Height] * [Count]".into()),
                precision: 2,
            },
            UserColumn::plain(3, "Col Height"),
            UserColumn {
                index: 4,
                name: "TTL Height LBS".into(),
                formula: Some("[TTL Height] * [LBS Per FT]".into()),
                precision: 2,
            },
            UserColumn::plain(5, "LBS Per SQFT"),
        ]
    }

    fn beam(feet: f64, plf: &str) -> Row {
        let mut row = Row::blank();
        row.kind = annot::Kind::Length;
        row.scaled = true;
        row.length = Some(feet);
        row.count = 1.0;
        row.columns[0] = plf.into();
        row
    }

    #[test]
    fn a_formula_column_is_worked_out_the_way_revu_works_it_out() {
        let row = beam(24.5, "26.00");
        assert_eq!(text(&row, &his_columns(), 1), "637.00");
        assert_eq!(name_of(&his_columns(), 1), "TTL Length LBS");
    }

    #[test]
    fn a_formula_can_use_another_formula() {
        let mut row = beam(0.0, "58.00");
        row.kind = annot::Kind::Count;
        row.length = None;
        row.count = 4.0;
        row.columns[3] = "14.5".into();
        assert_eq!(value(&row, &his_columns(), 2), Some(58.0));
        assert_eq!(value(&row, &his_columns(), 4), Some(58.0 * 58.0));
    }

    #[test]
    fn a_formula_over_something_missing_has_no_value_not_zero() {
        let row = beam(24.5, "");
        assert_eq!(value(&row, &his_columns(), 1), None);
        assert_eq!(text(&row, &his_columns(), 1), "");
        let mut unscaled = beam(24.5, "26");
        unscaled.scaled = false;
        assert_eq!(value(&unscaled, &his_columns(), 1), None);
    }

    #[test]
    fn the_weights_are_found_by_what_the_columns_are_called() {
        let w = weight_columns(&his_columns());
        assert_eq!((w.per_length, w.per_area), (0, 5));
        let theirs = vec![
            UserColumn::plain(0, "Piece Mark"),
            UserColumn::plain(2, "Weight (lbs/ft)"),
            UserColumn::plain(3, "PSF"),
        ];
        let w = weight_columns(&theirs);
        assert_eq!((w.per_length, w.per_area), (2, 3));
    }

    #[test]
    fn arithmetic_reads_the_way_it_is_written() {
        let mut none = |_: &str| None;
        assert_eq!(evaluate("2 + 3 * 4", &mut none), Some(14.0));
        assert_eq!(evaluate("(2 + 3) * 4", &mut none), Some(20.0));
        assert_eq!(evaluate("10 / 4 - 1", &mut none), Some(1.5));
        assert_eq!(evaluate("1 / 0", &mut none), None);
        assert_eq!(evaluate("2 +", &mut none), None);
        assert_eq!(evaluate("SUM(1)", &mut none), None);
    }

    #[test]
    fn a_formula_nested_past_any_real_one_has_no_value_and_does_not_run_out_of_stack() {
        // WHAT: brackets and signs a hundred thousand deep, worked out on a thread with a small stack. WHY: a tool chest's formula is read recursively, and one shared to the office reached every seat and the server.
        let brackets = format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000));
        let signs = format!("{}1", "-".repeat(100_000));
        let worked = std::thread::Builder::new()
            .stack_size(512 * 1024) // WHAT: a small stack. WHY: an unlimited reader runs out of it here first.
            .spawn(move || (evaluate(&brackets, &mut |_| None), evaluate(&signs, &mut |_| None)))
            .unwrap()
            .join(); // WHAT: the thread finished. WHY: a stack overflow would end the test process instead.
        assert_eq!(worked.ok(), Some((None, None)), "too deep is no value, and the reader lived");
        assert_eq!(evaluate("((2 + 3)) * -(-4)", &mut |_| None), Some(20.0), "ordinary nesting still works");
    }

    #[test]
    fn a_formula_that_refers_to_itself_has_no_value() {
        let columns = vec![UserColumn {
            index: 0,
            name: "Loop".into(),
            formula: Some("[Loop] + 1".into()),
            precision: 2,
        }];
        assert_eq!(value(&beam(1.0, "1"), &columns, 0), None);
    }

    #[test]
    fn a_chain_of_formulas_each_naming_the_last_many_times_comes_back_at_once() {
        // WHAT: eight columns, each adding the one before it to itself twelve times. WHY: each name is looked up again, level by level, so eight short formulas asked for twelve to the eighth lookups for every markup, on every seat and the server.
        let mut columns = vec![UserColumn::plain(0, "Base")]; // WHAT: a typed-in number. WHY: the bottom of the chain.
        for level in 1..=8 {
            let below = if level == 1 { "Base".to_string() } else { format!("L{}", level - 1) }; // WHAT: the column below. WHY: named twelve times.
            columns.push(UserColumn { index: level, name: format!("L{level}"), formula: Some(vec![format!("[{below}]"); 12].join(" + ")), precision: 2 }); // WHAT: twelve times the one below. WHY: the fan-out.
        }
        let started = std::time::Instant::now(); // WHAT: now. WHY: timed.
        assert_eq!(value(&beam(1.0, "1"), &columns, 8), None, "past the budget, no value");
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "took {:?}", started.elapsed());
        let ok = vec![UserColumn::plain(0, "Rate"), UserColumn { index: 1, name: "Twice".into(), formula: Some("[Rate] + [Rate]".into()), precision: 2 }]; // WHAT: an ordinary formula using another column twice. WHY: still worked out.
        assert_eq!(value(&beam(1.0, "3"), &ok, 1), Some(6.0));
    }
}
