//! Excalibur View's own tool chest format: `.evtools`.
//!
//! A tool chest is the office's own knowledge — what a W12x26 weighs, what a
//! moment connection is called, what colour a punch item is — and it should
//! live in a file the office owns and can read, not in another company's
//! format. So Excalibur View keeps its tools in this one: plain JSON, one file
//! per chest, with nothing in it that needs anybody else's program to open.
//!
//! A Revu profile (`.bpx`) or tool set (`.btx`) is read once, on the way in,
//! and saved as one of these; from then on it is ours. What comes across is
//! the tools, the office's column names and formulas, and the markups list's
//! columns. What does not is Revu's toolbar arrangement — Excalibur View lays
//! out its own.
//!
//! Each tool keeps the markup it stamps down as a PDF annotation dictionary
//! (`template`, base64 of the dictionary as a PDF writes it). That is the PDF
//! standard's own format, not Revu's, and it is what makes a markup drawn with
//! the tool the same markup every PDF reader shows. The fields beside it —
//! subject, label, colour, columns — are the ones people edit, and they win:
//! they are written into the template when the file is read.

use base64::Engine;
use pdf::{Name, Object, Reader};
use serde::{Deserialize, Serialize};

use crate::profile::{Column, CustomColumn, Profile};
use crate::toolset::{Tool, ToolSet};

/// The file's extension, without the dot.
pub const EXTENSION: &str = "evtools";

/// What the file says it is, so it can be told from any other JSON.
pub const FORMAT: &str = "excalibur-view-tools";

/// The version of this format. A newer file is read as far as this build
/// understands it; nothing in it is ever required that an older file lacks.
pub const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Chest {
    format: String,
    version: u32,
    name: String,
    /// Where it came from, when it was read in from something else: a line
    /// for whoever opens the file, never used for anything.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    imported_from: String,
    /// The office's own columns: "LBS Per FT", "Piece Mark", formulas.
    #[serde(default)]
    columns: Vec<OwnColumn>,
    /// Which columns the markups list shows, in order, and how wide.
    #[serde(default)]
    list: Vec<ListColumn>,
    sets: Vec<Set>,
}

#[derive(Serialize, Deserialize)]
struct OwnColumn {
    /// Which of the six a tool's `columns` it is, from nought.
    slot: usize,
    name: String,
    /// `Number`, `Text`, `Formula`, `Choice`, `Date`.
    kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    formula: String,
    #[serde(default)]
    decimals: usize,
}

#[derive(Serialize, Deserialize)]
struct ListColumn {
    key: String,
    width: f32,
    visible: bool,
}

#[derive(Serialize, Deserialize)]
struct Set {
    title: String,
    tools: Vec<OwnTool>,
}

#[derive(Serialize, Deserialize)]
struct OwnTool {
    subject: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    label: String,
    /// Length, Area, Count… For people reading the file: the kind comes from
    /// the template, because that is what gets drawn.
    kind: String,
    /// `#rrggbb`.
    colour: String,
    /// The six column values, in the order of the chest's `columns`.
    #[serde(default)]
    columns: [String; 6],
    /// Carries properties onto the next markup drawn rather than being
    /// stamped down as it stands.
    #[serde(default, skip_serializing_if = "is_false")]
    properties_only: bool,
    /// The PDF annotation dictionary this tool stamps, base64.
    template: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Whether some bytes are one of these files.
pub fn is_native(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with('{') && text.contains(FORMAT)
}

/// Writes a chest in Excalibur View's own format.
pub fn write(profile: &Profile, imported_from: &str) -> Vec<u8> {
    let chest = Chest {
        format: FORMAT.into(),
        version: VERSION,
        name: profile.name.clone(),
        imported_from: imported_from.into(),
        columns: profile
            .custom_columns
            .iter()
            .map(|c| OwnColumn {
                slot: c.index,
                name: c.name.clone(),
                kind: c.kind.clone(),
                formula: c.expression.clone(),
                decimals: c.precision,
            })
            .collect(),
        list: profile
            .markup_columns
            .iter()
            .map(|c| ListColumn { key: c.key.clone(), width: c.width, visible: c.visible })
            .collect(),
        sets: profile
            .sets
            .iter()
            .map(|set| Set {
                title: set.title.clone(),
                tools: set.tools.iter().map(own_tool).collect(),
            })
            .collect(),
    };
    let mut out = serde_json::to_vec_pretty(&chest).unwrap_or_default();
    out.push(b'\n');
    out
}

fn own_tool(tool: &Tool) -> OwnTool {
    let mut bytes = Vec::new();
    pdf::write::write_dict(&tool.annotation, &mut bytes);
    OwnTool {
        subject: tool.subject.clone(),
        label: tool.label.clone(),
        kind: tool.kind.name().into(),
        colour: hex_colour(tool.colour),
        columns: tool.columns.clone(),
        properties_only: tool.properties_only,
        template: base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

/// Reads one of these files. `None` when it is not one, or is too broken to
/// be worth half-reading.
pub fn read(bytes: &[u8]) -> Option<Profile> {
    let text = std::str::from_utf8(bytes).ok()?.trim_start_matches('\u{feff}');
    let chest: Chest = serde_json::from_str(text).ok()?;
    if chest.format != FORMAT {
        return None;
    }
    let sets: Vec<ToolSet> = chest
        .sets
        .into_iter()
        .map(|set| ToolSet {
            title: set.title,
            tools: set.tools.into_iter().filter_map(tool_from).collect(),
        })
        .filter(|set| !set.tools.is_empty())
        .collect();
    Some(Profile {
        name: if chest.name.trim().is_empty() { "Tool chest".into() } else { chest.name },
        toolbars: Vec::new(),
        nav_left: Vec::new(),
        nav_middle: Vec::new(),
        nav_right: Vec::new(),
        hover_bar: Vec::new(),
        panels: Default::default(),
        markup_columns: chest
            .list
            .into_iter()
            .map(|c| Column { key: c.key, width: c.width, visible: c.visible })
            .collect(),
        custom_columns: chest
            .columns
            .into_iter()
            .filter(|c| c.slot < 6)
            .map(|c| CustomColumn {
                index: c.slot,
                name: c.name,
                kind: c.kind,
                expression: c.formula,
                precision: c.decimals,
            })
            .collect(),
        thumbnail_size: 132.0,
        sets,
    })
}

fn tool_from(own: OwnTool) -> Option<Tool> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(own.template.trim()).ok()?;
    let mut annotation = Reader::new(&bytes).object().ok()?.as_dict()?.clone();
    // What people edit wins over what is inside the template.
    let colour = parse_colour(&own.colour).unwrap_or([0.0, 0.0, 0.0]);
    annotation.set(Name::new("Subj"), Object::text(&own.subject));
    if own.label.is_empty() {
        annotation.remove("Label");
    } else {
        annotation.set(Name::new("Label"), Object::text(&own.label));
    }
    annotation.set(
        Name::new("C"),
        Object::Array(colour.iter().map(|c| Object::real(*c as f64)).collect()),
    );
    if own.columns.iter().any(|c| !c.is_empty()) || annotation.get("BSIColumnData").is_some() {
        annotation.set(
            Name::new("BSIColumnData"),
            Object::Array(own.columns.iter().map(|c| Object::text(c)).collect()),
        );
    }
    let kind = annot::Kind::read(&annotation);
    Some(Tool {
        subject: own.subject,
        label: own.label,
        kind,
        annotation,
        columns: own.columns,
        colour,
        properties_only: own.properties_only,
    })
}

fn hex_colour(c: [f32; 3]) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(c[0]), byte(c[1]), byte(c[2]))
}

fn parse_colour(text: &str) -> Option<[f32; 3]> {
    let hex = text.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let part = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(|v| v as f32 / 255.0);
    Some([part(0)?, part(2)?, part(4)?])
}

/// What [`check`] found: the chest exactly as Excalibur View's own import
/// reads it, and everything in the file that import would drop or change
/// without saying so.
#[derive(Debug)]
pub struct Checked {
    /// The chest as [`read`] gives it, or `None` when it refuses the file.
    pub profile: Option<Profile>,
    /// Each thing wrong, in words. Empty when the file comes through whole.
    pub problems: Vec<String>,
}

/// Checks a file meant to be one of these, the way a program that writes
/// tool chests needs to: through the same import Excalibur View uses, and
/// then tool by tool, so a tool the import would quietly leave out, a colour
/// it would quietly turn black, or a kind the file claims but the template
/// doesn't draw, is named rather than lost.
#[must_use] // WHAT: a caller that drops the report gets a warning. WHY: checking has no other effect.
pub fn check(bytes: &[u8]) -> Checked {
    let mut problems = Vec::new(); // WHAT: what is wrong, in order found. WHY: the report.
    if !is_native(bytes) { // WHAT: the import's own first test. WHY: a file that fails it is read as a Revu file instead, and refused.
        problems.push(format!("it doesn't start with '{{' and name the format \"{FORMAT}\" in its first 512 bytes, so it isn't read as an Excalibur View tool chest")); // WHAT: says so. WHY: the fix is to put "format" first.
        return Checked { profile: None, problems }; // WHAT: nothing more to check. WHY: the import goes no further.
    }
    let Ok(text) = std::str::from_utf8(bytes) else { // WHAT: the bytes as UTF-8. WHY: the import refuses anything else.
        problems.push("it isn't UTF-8 text".into()); // WHAT: says so. WHY: the file must be UTF-8.
        return Checked { profile: None, problems };
    };
    let chest: Chest = match serde_json::from_str(text.trim_start_matches('\u{feff}')) { // WHAT: the file read with the import's own shape. WHY: the same fields, the same required ones.
        Ok(c) => c,
        Err(e) => {
            problems.push(format!("its JSON doesn't have the tool chest's shape: {e}")); // WHAT: serde's reason, with line and column. WHY: points at the field.
            return Checked { profile: None, problems };
        }
    };
    if chest.format != FORMAT { // WHAT: a different format value. WHY: the import refuses it.
        problems.push(format!("its format is \"{}\", not \"{FORMAT}\"", chest.format)); // WHAT: says what it said. WHY: the fix is obvious from it.
    }
    if chest.version > VERSION { // WHAT: written for a newer version than this build knows. WHY: it is read as far as this build understands, which may not be all of it.
        problems.push(format!("it is version {}, and this build knows version {VERSION}", chest.version)); // WHAT: says so. WHY: a writer should write the version it means.
    }
    if chest.name.trim().is_empty() { // WHAT: no name. WHY: the import calls it "Tool chest", which nobody can tell apart.
        problems.push("it has no name".into()); // WHAT: says so. WHY: the person sees the name in the program.
    }
    let mut slots = std::collections::BTreeSet::new(); // WHAT: column slots seen. WHY: two columns in one slot fight over it.
    for column in &chest.columns { // WHAT: each column the chest names. WHY: each is checked.
        if column.slot >= 6 { // WHAT: past the six slots. WHY: the import drops it.
            problems.push(format!("column \"{}\" is in slot {}, and there are only slots 0 to 5", column.name, column.slot)); // WHAT: says which. WHY: it would vanish.
        } else if !slots.insert(column.slot) { // WHAT: a second column in a slot. WHY: only one name can show.
            problems.push(format!("two columns are in slot {}", column.slot)); // WHAT: says which slot. WHY: the same.
        }
        if !matches!(column.kind.as_str(), "Number" | "Text" | "Formula" | "Choice" | "Date") { // WHAT: a kind the program doesn't know. WHY: the column would be treated as plain text.
            problems.push(format!("column \"{}\" has kind \"{}\"; it should be Number, Text, Formula, Choice or Date", column.name, column.kind)); // WHAT: says so. WHY: names the choices.
        }
    }
    for set in &chest.sets { // WHAT: each tool set. WHY: tools are checked one by one.
        if set.tools.is_empty() { // WHAT: a set with no tools. WHY: the import leaves it out.
            problems.push(format!("tool set \"{}\" has no tools, so it is left out", set.title)); // WHAT: says so. WHY: it would vanish.
        }
        for tool in &set.tools { // WHAT: each tool. WHY: each can be lost on its own.
            let at = format!("tool \"{}\" in \"{}\"", tool.subject, set.title); // WHAT: how the tool is named in the report. WHY: the writer finds it.
            if tool.subject.trim().is_empty() { // WHAT: no subject. WHY: the subject is what the takeoff groups by.
                problems.push(format!("{at} has no subject")); // WHAT: says so. WHY: the same.
            }
            if parse_colour(&tool.colour).is_none() { // WHAT: a colour that doesn't read. WHY: the import quietly makes it black.
                problems.push(format!("{at} has colour \"{}\"; it should be #rrggbb", tool.colour)); // WHAT: says so. WHY: the form it needs.
            }
            let read = OwnTool { // WHAT: a copy of the tool to read. WHY: tool_from takes it by value.
                subject: tool.subject.clone(),
                label: tool.label.clone(),
                kind: tool.kind.clone(),
                colour: tool.colour.clone(),
                columns: tool.columns.clone(),
                properties_only: tool.properties_only,
                template: tool.template.clone(),
            };
            match tool_from(read) { // WHAT: the tool through the import's own reading. WHY: exactly what the program would get.
                None => problems.push(format!("{at} is left out: its template isn't base64 of one PDF dictionary")), // WHAT: says so. WHY: the import drops it without a word.
                Some(made) if !made.kind.name().eq_ignore_ascii_case(tool.kind.trim()) => problems.push(format!(
                    "{at} says it is a {} tool, but its template draws a {} tool",
                    tool.kind,
                    made.kind.name()
                )), // WHAT: the stated kind and the drawn kind differ. WHY: the template wins, so the file misleads whoever reads it.
                Some(_) => {}
            }
        }
    }
    let profile = read(bytes); // WHAT: the real import, start to finish. WHY: what the program gets is the final word.
    if profile.is_none() && problems.is_empty() { // WHAT: refused for a reason not found above. WHY: never report a clean file the import refuses.
        problems.push("Excalibur View's import refuses it".into());
    }
    Checked { profile, problems }
}

/// Whatever sort of chest `bytes` is, as a chest.
pub fn read_any(bytes: &[u8]) -> Option<Profile> {
    if is_native(bytes) {
        read(bytes)
    } else {
        Profile::read_revu(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use annot::Kind;

    fn a_chest() -> Profile {
        let mut beam = crate::toolset::plain("W12x26", Kind::Length);
        beam.annotation = Reader::new(
            b"<< /Type /Annot /Subtype /PolyLine /Subj (W12x26) /IT /PolyLineDimension \
               /C [1 0 0] /BSIColumnData [(26) () () () () ()] /Measure << /Type /Measure >> >>",
        )
        .object()
        .unwrap()
        .as_dict()
        .unwrap()
        .clone();
        beam.kind = Kind::read(&beam.annotation);
        beam.columns[0] = "26".into();
        beam.colour = [1.0, 0.0, 0.0];
        Profile {
            name: "Steel Takeoff".into(),
            toolbars: Vec::new(),
            nav_left: Vec::new(),
            nav_middle: Vec::new(),
            nav_right: Vec::new(),
            hover_bar: Vec::new(),
            panels: Default::default(),
            markup_columns: vec![Column { key: "Subject".into(), width: 210.0, visible: true }],
            custom_columns: vec![CustomColumn {
                index: 0,
                name: "LBS Per FT".into(),
                kind: "Number".into(),
                expression: String::new(),
                precision: 2,
            }],
            thumbnail_size: 132.0,
            sets: vec![ToolSet { title: "Beams".into(), tools: vec![beam] }],
        }
    }

    #[test]
    fn a_chest_comes_back_as_it_went_out() {
        let chest = a_chest();
        let bytes = write(&chest, "Revu profile: Steel Takeoff.bpx");
        assert!(is_native(&bytes));
        let back = read(&bytes).expect("our own file reads");
        assert_eq!(back.name, "Steel Takeoff");
        assert_eq!(back.custom_columns, chest.custom_columns);
        let tool = &back.sets[0].tools[0];
        assert_eq!(tool.subject, "W12x26");
        assert_eq!(tool.kind, chest.sets[0].tools[0].kind);
        assert_eq!(tool.pounds_per_foot(), Some(26.0), "the weight comes across");
        assert!(tool.annotation.get("Measure").is_some(), "and so does the rest of the template");
        assert!(back.toolbars.is_empty(), "the toolbars are Excalibur View's own");
    }

    #[test]
    fn what_people_edit_in_the_file_wins() {
        let text = String::from_utf8(write(&a_chest(), "")).unwrap();
        let edited = text
            .replace("\"subject\": \"W12x26\"", "\"subject\": \"W12x30\"")
            .replace("\"26\"", "\"30\"")
            .replace("#ff0000", "#0000ff");
        let tool = read(edited.as_bytes()).unwrap().sets.remove(0).tools.remove(0);
        assert_eq!(tool.subject, "W12x30");
        assert_eq!(tool.pounds_per_foot(), Some(30.0));
        assert_eq!(tool.colour, [0.0, 0.0, 1.0]);
        assert_eq!(tool.annotation.get("Subj").and_then(|o| o.as_text()).as_deref(), Some("W12x30"));
    }

    #[test]
    fn a_chest_this_program_wrote_checks_clean() {
        let checked = check(&write(&a_chest(), "")); // WHAT: our own file through the checker. WHY: what the program writes must pass what it checks.
        assert!(checked.problems.is_empty(), "{:?}", checked.problems); // WHAT: nothing wrong. WHY: the reference case.
        assert_eq!(checked.profile.unwrap().tool_count(), 1); // WHAT: the one tool, read. WHY: the checker hands back the real import.
    }

    /// A chest file as text, with one tool whose fields are given.
    fn one_tool(kind: &str, colour: &str, template: &str, columns: &str) -> String {
        format!(
            r#"{{"format":"excalibur-view-tools","version":1,"name":"Test","columns":{columns},"sets":[{{"title":"S","tools":[{{"subject":"T","kind":"{kind}","colour":"{colour}","template":"{template}"}}]}}]}}"#
        ) // WHAT: the smallest whole chest. WHY: each test changes one field.
    }

    fn b64(dict: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(dict) // WHAT: a template as the file keeps it. WHY: base64 of the dictionary's text.
    }

    #[test]
    fn a_tool_the_import_would_drop_is_named() {
        let text = one_tool("Length", "#ff0000", "not base64!", "[]"); // WHAT: a template that isn't base64. WHY: the import drops the tool quietly.
        let checked = check(text.as_bytes()); // WHAT: checked. WHY: it must say so out loud.
        assert!(checked.problems.iter().any(|p| p.contains("left out")), "{:?}", checked.problems); // WHAT: named. WHY: a lost tool must never pass.
        assert!(checked.problems.iter().any(|p| p.contains("no tools")) || checked.profile.is_none() || checked.profile.as_ref().is_some_and(|p| p.tool_count() == 0)); // WHAT: and nothing came through. WHY: what the import really did.
    }

    #[test]
    fn a_kind_the_template_doesnt_draw_is_named() {
        let area = b64("<</Type /Annot/Subtype /Polygon/IT /PolygonDimension/MeasurementTypes 1>>"); // WHAT: an area template. WHY: claimed below to be a length.
        let checked = check(one_tool("Length", "#ff0000", &area, "[]").as_bytes()); // WHAT: a file that says Length. WHY: the mismatch.
        assert!(checked.problems.iter().any(|p| p.contains("draws a Area")), "{:?}", checked.problems); // WHAT: named. WHY: the template wins, so the file misleads.
        assert!(check(one_tool("Area", "#ff0000", &area, "[]").as_bytes()).problems.is_empty()); // WHAT: the same tool, honest. WHY: clean.
    }

    #[test]
    fn a_colour_or_column_the_import_would_change_is_named() {
        let count = b64("<</Type /Annot/Subtype /Polygon/IT /PolygonCount/MeasurementTypes 128>>"); // WHAT: a count template. WHY: a good tool, so only the other fields are wrong.
        assert!(check(one_tool("Count", "red", &count, "[]").as_bytes()).problems.iter().any(|p| p.contains("#rrggbb"))); // WHAT: a named colour. WHY: the import makes it black.
        let late = r#"[{"slot":6,"name":"Seventh","kind":"Text"}]"#; // WHAT: a column past slot 5. WHY: the import drops it.
        assert!(check(one_tool("Count", "#00ff00", &count, late).as_bytes()).problems.iter().any(|p| p.contains("slot 6"))); // WHAT: named. WHY: it would vanish.
        let twice = r#"[{"slot":1,"name":"A","kind":"Text"},{"slot":1,"name":"B","kind":"Text"}]"#; // WHAT: two columns in slot 1. WHY: only one can show.
        assert!(check(one_tool("Count", "#00ff00", &count, twice).as_bytes()).problems.iter().any(|p| p.contains("two columns"))); // WHAT: named. WHY: the same.
        let odd = r#"[{"slot":1,"name":"A","kind":"Wordy"}]"#; // WHAT: a kind nobody knows. WHY: treated as text.
        assert!(check(one_tool("Count", "#00ff00", &count, odd).as_bytes()).problems.iter().any(|p| p.contains("Wordy"))); // WHAT: named. WHY: the same.
    }

    #[test]
    fn a_file_the_import_refuses_says_why() {
        assert!(check(b"{\"hello\": 1}").problems[0].contains("first 512 bytes")); // WHAT: JSON that never names the format. WHY: read as Revu, refused.
        let padded = format!("{{{}\"format\":\"excalibur-view-tools\"}}", " ".repeat(600)); // WHAT: the format named too late. WHY: the import only looks at the start.
        assert!(check(padded.as_bytes()).problems[0].contains("first 512 bytes")); // WHAT: refused for that. WHY: the generator's commonest slip.
        let shapeless = br#"{"format":"excalibur-view-tools","version":1,"name":"x"}"#; // WHAT: no sets. WHY: a required field missing.
        let checked = check(shapeless); // WHAT: checked. WHY: the JSON shape.
        assert!(checked.profile.is_none() && checked.problems[0].contains("sets"), "{:?}", checked.problems); // WHAT: refused, naming the field. WHY: points at the fix.
        let newer = br#"{"format":"excalibur-view-tools","version":9,"name":"x","sets":[]}"#; // WHAT: a version from the future. WHY: may not be read whole.
        assert!(check(newer).problems.iter().any(|p| p.contains("version 9"))); // WHAT: named. WHY: the writer should write version 1.
    }

    #[test]
    fn other_json_is_not_a_tool_chest() {
        assert!(!is_native(b"{\"hello\": 1}"));
        assert!(read(b"{\"format\": \"something-else\", \"version\": 1, \"name\": \"x\", \"sets\": []}").is_none());
        assert!(read(b"not json").is_none());
    }
}
