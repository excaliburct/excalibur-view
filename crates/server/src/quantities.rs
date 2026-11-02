//! Working out a takeoff from what the server holds.
//!
//! The same engine the viewer uses, on the same annotation dictionaries, so a
//! number fetched over the API and a number read off somebody's screen cannot
//! disagree. That is the entire point of doing it this way rather than
//! re-implementing the arithmetic in a web handler.
//!
//! And the same rule: **nothing is estimated.** A markup on a sheet with no
//! scale is reported, counted as left out, and kept out of every total. It is
//! never zero.

use anyhow::Result;
use annot::measure::Measure;
use annot::Markup;
use hub::{Takeoff, TakeoffGroup, TakeoffRow};
use takeoff::{Column, Row, WeightColumns};

/// Which of the six user columns carry unit weights. Column zero is pounds per
/// foot and column five is pounds per square foot, which is how the tool chests
/// in use here are set up. The numbers come out of those columns and nowhere
/// else: never a section table this program looked up.
pub const WEIGHTS: WeightColumns = WeightColumns {
    per_length: 0,
    per_area: 5,
};

/// One stored markup, enough to measure it.
pub struct Stored {
    pub id: String,
    pub page: usize,
    pub dictionary: Vec<u8>,
    pub author: String,
}

/// A sheet, and whether it has a scale.
pub struct SheetScale {
    pub number: String,
    pub scale: Option<Measure>,
}

/// Measures every stored markup, keeping its id and its sheet's name beside it.
///
/// Everything that works out a quantity on this server goes through here, so
/// the takeoff, the cut list and the double-count check are all looking at the
/// same measurements rather than three separate attempts at them.
pub fn measured(
    markups: &[Stored],
    sheets: &[SheetScale],
) -> Vec<(String, Row, String)> {
    let mut rows: Vec<(String, Row, String)> = Vec::new();
    for stored in markups {
        let Some(markup) = read_markup(&stored.dictionary) else {
            // A dictionary we cannot parse is left out and said so, rather
            // than counted as nothing.
            continue;
        };
        let scale = sheets.get(stored.page).and_then(|s| s.scale.clone());
        let row = takeoff::measure(
            stored.page,
            pdf::Ref::new(0, 0),
            &markup,
            scale.as_ref(),
        );
        let sheet = sheets
            .get(stored.page)
            .map(|s| s.number.clone())
            .unwrap_or_else(|| format!("Sheet {}", stored.page + 1));
        rows.push((stored.id.clone(), row, sheet));
    }
    rows
}

/// Just the measurements, for the things that only want those.
pub fn rows_of(markups: &[Stored], sheets: &[SheetScale]) -> Vec<Row> {
    measured(markups, sheets)
        .into_iter()
        .map(|(_, row, _)| row)
        .collect()
}

/// Measures everything and totals it.
pub fn takeoff(set: &str, revision: u64, markups: &[Stored], sheets: &[SheetScale]) -> Result<Takeoff> {
    let rows = measured(markups, sheets);

    let measured: Vec<Row> = rows.iter().map(|(_, r, _)| r.clone()).collect();
    let summary = takeoff::summarise(&measured, WEIGHTS);

    let out_rows = rows
        .iter()
        .map(|(id, row, sheet)| TakeoffRow {
            markup: id.clone(),
            page: row.page,
            sheet: sheet.clone(),
            subject: row.subject.clone(),
            kind: row.kind.name().to_string(),
            caption: row.caption.clone(),
            author: row.author.clone(),
            scaled: row.scaled,
            count: row.count,
            // Every measured field is absent, not zero, when there is no
            // scale. A zero in a JSON document is a measurement; a null is an
            // admission.
            length_feet: row.scaled.then(|| row.length).flatten(),
            area_square_feet: row.scaled.then(|| row.area).flatten(),
            volume_cubic_feet: row.scaled.then(|| row.volume).flatten(),
            pounds: row.pounds(WEIGHTS),
        })
        .collect();

    let name_of = |page: usize| {
        sheets
            .get(page)
            .map(|s| s.number.clone())
            .unwrap_or_else(|| format!("Sheet {}", page + 1))
    };

    let groups = summary
        .groups
        .iter()
        .map(|g| TakeoffGroup {
            subject: g.subject.clone(),
            kind: g.kind.name().to_string(),
            picks: g.picks,
            count: g.count,
            length_feet: g.length,
            area_square_feet: g.area,
            volume_cubic_feet: g.volume,
            pounds: g.weighed.then_some(g.pounds),
            sheets: g.pages.iter().map(|p| name_of(*p)).collect(),
            left_out: g.unscaled,
        })
        .collect();

    Ok(Takeoff {
        set: set.to_string(),
        revision,
        rows: out_rows,
        groups,
        picks: summary.picks,
        pounds: (summary.pounds > 0.0).then_some(summary.pounds),
        tons: (summary.pounds > 0.0).then_some(summary.tons()),
        left_out: summary.unscaled,
        sheets_without_a_scale: summary.unscaled_pages.iter().map(|p| name_of(*p)).collect(),
    })
}

/// Parses a stored annotation dictionary back into a markup.
pub fn read_markup(bytes: &[u8]) -> Option<Markup> {
    let mut reader = pdf::Reader::new(bytes); // WHAT: a reader over the markup's bytes. WHY: asked afterwards whether it went too deep.
    let object = reader.object().ok()?;
    if reader.went_too_deep() {
        return None; // WHAT: a markup nested past any real one is not a markup. WHY: its bytes are kept and handed to every seat as they came, and a seat older than this one runs out of stack reading them.
    }
    let dict = object.as_dict()?.clone();
    Some(Markup { dict, picture: None })
}

/// Serialises a markup for storage: the dictionary exactly as it would be
/// written into the drawing.
pub fn write_markup(markup: &Markup) -> Vec<u8> {
    let mut out = Vec::new();
    pdf::write::write_object(&pdf::Object::Dict(markup.dict.clone()), &mut out);
    out
}

/// The CSV a spreadsheet wants, from the same rows.
pub fn to_csv(markups: &[Stored], sheets: &[SheetScale], columns: &[Column]) -> String {
    let mut rows = Vec::new();
    for stored in markups {
        let Some(markup) = read_markup(&stored.dictionary) else {
            continue;
        };
        let scale = sheets.get(stored.page).and_then(|s| s.scale.clone());
        rows.push(takeoff::measure(
            stored.page,
            pdf::Ref::new(0, 0),
            &markup,
            scale.as_ref(),
        ));
    }
    let name_of = |page: usize| {
        sheets
            .get(page)
            .map(|s| s.number.clone())
            .unwrap_or_else(|| format!("Sheet {}", page + 1))
    };
    let mut out = takeoff::summary::to_csv_with(&rows, columns, WEIGHTS, &name_of);
    let summary = takeoff::summarise(&rows, WEIGHTS);
    out.push('\n');
    out.push_str(&takeoff::summary::summary_csv_with(&summary, &name_of));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use annot::measure::imperial;
    use annot::Subtype;

    fn quarter() -> Measure {
        imperial(48.0, "1/4\" = 1'-0\"", 16)
    }

    /// A forty-foot W12x26, weighing what his chest says it weighs.
    fn a_beam(page: usize, id: &str) -> Stored {
        let mut m = Markup::new(Subtype::Line);
        m.set("IT", pdf::Object::name("LineDimension"));
        m.set_subject("W12x26");
        m.set_author("Sam");
        m.set(
            "BSIColumnData",
            pdf::Object::Array(vec![
                pdf::Object::text("26"),
                pdf::Object::text(""),
                pdf::Object::text(""),
                pdf::Object::text(""),
                pdf::Object::text(""),
                pdf::Object::text(""),
            ]),
        );
        m.set_line([0.0, 0.0], [720.0, 0.0]);
        Stored {
            id: id.to_string(),
            page,
            dictionary: write_markup(&m),
            author: "Sam".into(),
        }
    }

    fn scaled_sheets() -> Vec<SheetScale> {
        vec![
            SheetScale {
                number: "S-200".into(),
                scale: Some(quarter()),
            },
            SheetScale {
                number: "S-201".into(),
                scale: None,
            },
        ]
    }

    #[test]
    fn a_markup_survives_being_written_and_read_back() {
        let beam = a_beam(0, "m1");
        let back = read_markup(&beam.dictionary).expect("it parses");
        assert_eq!(back.subject(), "W12x26");
        assert_eq!(back.points().len(), 2);
    }

    #[test]
    fn the_api_totals_what_the_viewer_totals() {
        let markups = vec![a_beam(0, "m1"), a_beam(0, "m2")];
        let result = takeoff("set1", 3, &markups, &scaled_sheets()).unwrap();
        // Eighty feet of W12x26 is 2,080 lb.
        assert_eq!(result.pounds, Some(2080.0));
        assert_eq!(result.tons, Some(1.04));
        assert_eq!(result.left_out, 0);
        assert!(!result.is_short());
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0].caption, "40'-0\"");
        assert_eq!(result.rows[0].sheet, "S-200");
    }

    #[test]
    fn a_markup_on_an_unscaled_sheet_is_reported_and_left_out() {
        // One on a scaled sheet, one on a sheet with no scale.
        let markups = vec![a_beam(0, "m1"), a_beam(1, "m2")];
        let result = takeoff("set1", 4, &markups, &scaled_sheets()).unwrap();
        assert!(result.is_short(), "the total is short and must say so");
        assert_eq!(result.left_out, 1);
        assert_eq!(result.sheets_without_a_scale, vec!["S-201".to_string()]);
        // Only the scaled one counted.
        assert_eq!(result.pounds, Some(1040.0));

        let unscaled = result.rows.iter().find(|r| r.markup == "m2").unwrap();
        assert!(!unscaled.scaled);
        assert_eq!(unscaled.length_feet, None, "absent, not zero");
        assert_eq!(unscaled.pounds, None, "absent, not zero");
        assert_eq!(unscaled.caption, "");
    }

    #[test]
    fn nothing_measured_reports_no_weight_rather_than_no_pounds() {
        let result = takeoff("set1", 0, &[], &scaled_sheets()).unwrap();
        assert_eq!(result.pounds, None, "no weight at all, rather than zero");
        assert_eq!(result.tons, None);
        assert_eq!(result.picks, 0);
        assert!(!result.is_short());
    }

    #[test]
    fn a_dictionary_that_cannot_be_parsed_is_dropped_rather_than_counted() {
        let mut markups = vec![a_beam(0, "m1")];
        markups.push(Stored {
            id: "broken".into(),
            page: 0,
            dictionary: b"this is not a pdf dictionary".to_vec(),
            author: "Sam".into(),
        });
        let result = takeoff("set1", 1, &markups, &scaled_sheets()).unwrap();
        assert_eq!(result.rows.len(), 1, "the good one is still measured");
        assert_eq!(result.pounds, Some(1040.0));
    }

    #[test]
    fn the_csv_names_the_sheets_the_way_the_drawing_does() {
        let markups = vec![a_beam(0, "m1"), a_beam(1, "m2")];
        let csv = to_csv(
            &markups,
            &scaled_sheets(),
            &[Column::Page, Column::Subject, Column::Measurement, Column::Pounds],
        );
        assert!(csv.contains("S-200"), "{csv}");
        assert!(csv.contains("S-201"), "{csv}");
        assert!(csv.contains("40'-0\"") || csv.contains("40'-0"), "{csv}");
        assert!(csv.contains("were left out"), "{csv}");
    }
}
