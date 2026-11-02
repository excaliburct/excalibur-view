//! Totals.
//!
//! The rule that matters here: a measurement from a sheet with no scale is
//! **left out** of every total and counted separately, so a takeoff can never
//! quietly come up short because somebody forgot to calibrate a sheet.

use std::collections::BTreeMap;

use annot::Kind;

use crate::columns::Column;
use crate::row::{Row, WeightColumns};

#[derive(Clone, Debug)]
pub struct Group {
    pub subject: String,
    pub kind: Kind,
    /// How many markups went into this line.
    pub picks: usize,
    pub count: f64,
    pub length: f64,
    pub area: f64,
    pub perimeter: f64,
    pub volume: f64,
    pub pounds: f64,
    /// Set when at least one markup in the group carried a unit weight.
    pub weighed: bool,
    pub unit: String,
    pub pages: Vec<usize>,
    /// Markups in this group that had no scale and were left out.
    pub unscaled: usize,
}

impl Group {
    pub fn tons(&self) -> f64 {
        self.pounds / 2000.0
    }
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub groups: Vec<Group>,
    pub picks: usize,
    pub pounds: f64,
    /// Markups left out for want of a scale.
    pub unscaled: usize,
    /// Sheets that carry markups but have no scale.
    pub unscaled_pages: Vec<usize>,
}

impl Summary {
    pub fn tons(&self) -> f64 {
        self.pounds / 2000.0
    }

    pub fn is_short(&self) -> bool {
        self.unscaled > 0
    }
}

/// Totals the rows, keeping each subject and each kind apart.
pub fn summarise(rows: &[Row], weights: WeightColumns) -> Summary {
    let mut groups: BTreeMap<(String, &'static str), Group> = BTreeMap::new();
    let mut summary = Summary::default();

    for row in rows {
        if !row.kind.measures() {
            continue;
        }
        summary.picks += 1;
        if !row.scaled && row.kind != Kind::Count {
            summary.unscaled += 1;
            if !summary.unscaled_pages.contains(&row.page) {
                summary.unscaled_pages.push(row.page);
            }
        }
        let subject = if row.subject.trim().is_empty() {
            "(no subject)".to_string()
        } else {
            row.subject.clone()
        };
        let group = groups
            .entry((subject.clone(), row.kind.name()))
            .or_insert_with(|| Group {
                subject,
                kind: row.kind,
                picks: 0,
                count: 0.0,
                length: 0.0,
                area: 0.0,
                perimeter: 0.0,
                volume: 0.0,
                pounds: 0.0,
                weighed: false,
                unit: row.unit.clone(),
                pages: Vec::new(),
                unscaled: 0,
            });
        group.picks += 1;
        group.count += row.count * row.quantity;
        if !group.pages.contains(&row.page) {
            group.pages.push(row.page);
        }
        if !row.scaled {
            if row.kind != Kind::Count {
                group.unscaled += 1;
            }
            continue;
        }
        if group.unit.is_empty() {
            group.unit = row.unit.clone();
        }
        let q = row.quantity;
        group.length += row.length.unwrap_or(0.0) * q;
        group.area += row.area.unwrap_or(0.0) * q;
        group.perimeter += row.perimeter.unwrap_or(0.0) * q;
        group.volume += row.volume.unwrap_or(0.0) * q;
        if let Some(pounds) = row.pounds(weights) {
            group.pounds += pounds;
            group.weighed = true;
            summary.pounds += pounds;
        }
    }

    summary.groups = groups.into_values().collect();
    summary
        .groups
        .sort_by(|a, b| a.subject.cmp(&b.subject).then(a.kind.name().cmp(b.kind.name())));
    summary
}

/// Writes the rows out as a spreadsheet, one line per markup.
pub fn to_csv(rows: &[Row], columns: &[Column], weights: WeightColumns) -> String {
    to_csv_with(rows, columns, weights, &|p| format!("{}", p + 1))
}

/// The same, with the sheets named the way the drawing set names them, so a
/// line reads `S-200` and not `page 14`.
pub fn to_csv_with(
    rows: &[Row],
    columns: &[Column],
    weights: WeightColumns,
    sheet: &dyn Fn(usize) -> String,
) -> String {
    let mut out = String::new();
    let headings: Vec<String> = columns.iter().map(|c| c.heading()).collect();
    out.push_str(&headings.iter().map(|h| quote(h)).collect::<Vec<_>>().join(","));
    out.push('\n');
    for row in rows {
        let cells: Vec<String> = columns
            .iter()
            .map(|c| {
                // A spreadsheet wants the number, not the way it reads on the
                // sheet — except where there is no number to give.
                if *c == Column::Page {
                    return sheet(row.page);
                }
                match (c.numeric(), c.number(row, weights)) {
                    (true, Some(v)) => format!("{v}"),
                    _ => c.text(row, weights),
                }
            })
            .collect();
        out.push_str(&cells.iter().map(|c| quote(c)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    out
}

/// Writes the totals out, one line per subject.
pub fn summary_csv(summary: &Summary) -> String {
    summary_csv_with(summary, &|p| format!("{}", p + 1))
}

/// The same, with the sheets named the way the drawing set names them.
pub fn summary_csv_with(summary: &Summary, sheet: &dyn Fn(usize) -> String) -> String {
    let mut out = String::from("Subject,Type,Picks,Count,Length,Area,Volume,Pounds,Tons,Sheets,Left out\n");
    for g in &summary.groups {
        let sheets: Vec<String> = g.pages.iter().map(|p| sheet(*p)).collect();
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{}\n",
            quote(&g.subject),
            g.kind.name(),
            g.picks,
            g.count,
            g.length,
            g.area,
            g.volume,
            if g.weighed { format!("{:.2}", g.pounds) } else { String::new() },
            if g.weighed { format!("{:.4}", g.tons()) } else { String::new() },
            quote(&sheets.join(" ")),
            g.unscaled,
        ));
    }
    out.push_str(&format!(
        "{},,{},,,,,{:.2},{:.4},,{}\n",
        quote("TOTAL"),
        summary.picks,
        summary.pounds,
        summary.tons(),
        summary.unscaled
    ));
    if summary.is_short() {
        out.push_str(&format!(
            "\n# {} measurement(s) on sheet(s) {} were left out because those sheets have no scale set.\n",
            summary.unscaled,
            summary
                .unscaled_pages
                .iter()
                .map(|p| sheet(*p))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out
}

fn quote(field: &str) -> String {
    crate::export::csv_cell(field) // WHAT: quoted, and a formula written as text. WHY: a subject, a shape or a place is whatever somebody typed, and a spreadsheet must not run it.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::row::measure;
    use annot::measure::imperial;
    use annot::{Markup, Subtype};
    use pdf::Ref;

    fn beam(subject: &str, page: usize, points: f64, scaled: bool) -> Row {
        let mut m = Markup::new(Subtype::Line);
        m.set("IT", pdf::Object::name("LineDimension"));
        m.set_subject(subject);
        m.set(
            "BSIColumnData",
            pdf::Object::Array(vec![pdf::Object::text("26.00")]),
        );
        m.set_line([0.0, 0.0], [points, 0.0]);
        let scale = imperial(48.0, "1/4\" = 1'-0\"", 16);
        measure(page, Ref::new(1, 0), &m, scaled.then_some(&scale))
    }

    fn conn(page: usize) -> Row {
        let mut m = Markup::new(Subtype::Polygon);
        m.set("IT", pdf::Object::name("PolygonCount"));
        m.set_subject("Shear Conn");
        m.set_vertices(&[[0.0, 0.0], [9.0, 0.0], [9.0, 9.0]]);
        let scale = imperial(48.0, "1/4\" = 1'-0\"", 16);
        measure(page, Ref::new(1, 0), &m, Some(&scale))
    }

    #[test]
    fn subjects_are_totalled_separately_and_weighed() {
        let rows = vec![
            beam("W12x26", 0, 720.0, true),
            beam("W12x26", 1, 360.0, true),
            beam("W8x31", 0, 720.0, true),
        ];
        let s = summarise(&rows, Default::default());
        assert_eq!(s.groups.len(), 2);
        let w12 = s.groups.iter().find(|g| g.subject == "W12x26").unwrap();
        assert_eq!(w12.picks, 2);
        assert!((w12.length - 60.0).abs() < 1e-9);
        assert!((w12.pounds - 60.0 * 26.0).abs() < 1e-9);
        assert_eq!(w12.pages, vec![0, 1]);
        assert!((s.tons() - (60.0 + 40.0) * 26.0 / 2000.0).abs() < 1e-9);
    }

    #[test]
    fn an_unscaled_measurement_is_left_out_and_counted_as_left_out() {
        let rows = vec![
            beam("W12x26", 0, 720.0, true),
            beam("W12x26", 4, 720.0, false),
        ];
        let s = summarise(&rows, Default::default());
        assert_eq!(s.unscaled, 1);
        assert_eq!(s.unscaled_pages, vec![4]);
        assert!(s.is_short());
        let group = &s.groups[0];
        assert_eq!(group.picks, 2, "both picks are reported");
        assert_eq!(group.unscaled, 1);
        assert!(
            (group.length - 40.0).abs() < 1e-9,
            "but only the scaled one is totalled"
        );
    }

    #[test]
    fn counts_and_lengths_of_the_same_subject_stay_apart() {
        let rows = vec![beam("Shear Conn", 0, 720.0, true), conn(0), conn(0)];
        let s = summarise(&rows, Default::default());
        assert_eq!(s.groups.len(), 2);
        let counted = s.groups.iter().find(|g| g.kind == Kind::Count).unwrap();
        assert_eq!(counted.count, 2.0);
        assert_eq!(counted.length, 0.0);
    }

    #[test]
    fn a_group_with_no_unit_weight_is_not_reported_as_weighing_nothing() {
        let mut row = beam("Handrail", 0, 720.0, true);
        row.columns = Default::default();
        let s = summarise(&[row], Default::default());
        assert!(!s.groups[0].weighed);
        assert_eq!(s.groups[0].pounds, 0.0);
        let csv = summary_csv(&s);
        let line = csv.lines().nth(1).unwrap();
        assert!(line.ends_with(",1,0") || line.contains(",,,"), "{line}");
    }

    #[test]
    fn the_summary_says_in_writing_what_it_left_out() {
        let rows = vec![beam("W12x26", 2, 720.0, false)];
        let csv = summary_csv(&summarise(&rows, Default::default()));
        assert!(csv.contains("sheet(s) 3"), "{csv}");
        assert!(csv.contains("no scale set"), "{csv}");
    }

    #[test]
    fn a_subject_with_a_comma_does_not_break_the_spreadsheet() {
        let rows = vec![beam("ABBREVIATIONS, SYMBOLS", 0, 720.0, true)];
        let csv = to_csv(&rows, &[Column::Subject, Column::Length], Default::default());
        assert!(csv.contains("\"ABBREVIATIONS, SYMBOLS\""), "{csv}");
    }

    #[test]
    fn the_spreadsheet_carries_numbers_not_the_way_they_read() {
        let rows = vec![beam("W12x26", 0, 720.0, true)];
        let csv = to_csv(&rows, &[Column::Length, Column::Tons], Default::default());
        let line = csv.lines().nth(1).unwrap();
        assert!(line.starts_with("40,"), "{line}");
        assert!(!line.contains('\''), "a spreadsheet wants 40, not 40'-0\"");
    }
}
