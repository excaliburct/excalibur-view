//! How words on a markup are set: the font, its size, its weight, and where
//! they sit in the box.
//!
//! A text box whose words are not drawn into the appearance stream shows as an
//! empty rectangle in Acrobat, in a browser and on a general contractor's
//! screen — the same failure as a measurement whose number is not drawn in.
//! So the words go in, with the font somebody chose, wrapped to the box and
//! lined up the way they asked.

use pdf::{Dict, Name, Object};

/// One of the fonts every PDF reader has without being sent one.
///
/// Only the base fourteen. A markup that named a font off somebody's machine
/// would arrive at the general contractor set in whatever their machine had
/// instead, which is how a note that fitted its box stops fitting it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Family {
    #[default]
    Helvetica,
    Times,
    Courier,
}

impl Family {
    pub const ALL: &'static [Family] = &[Family::Helvetica, Family::Times, Family::Courier];

    pub fn name(self) -> &'static str {
        match self {
            Family::Helvetica => "Helvetica",
            Family::Times => "Times",
            Family::Courier => "Courier",
        }
    }

    /// What the PDF calls it, with the weight and slant applied.
    pub fn base_font(self, bold: bool, italic: bool) -> &'static str {
        match (self, bold, italic) {
            (Family::Helvetica, false, false) => "Helvetica",
            (Family::Helvetica, true, false) => "Helvetica-Bold",
            (Family::Helvetica, false, true) => "Helvetica-Oblique",
            (Family::Helvetica, true, true) => "Helvetica-BoldOblique",
            (Family::Times, false, false) => "Times-Roman",
            (Family::Times, true, false) => "Times-Bold",
            (Family::Times, false, true) => "Times-Italic",
            (Family::Times, true, true) => "Times-BoldItalic",
            (Family::Courier, false, false) => "Courier",
            (Family::Courier, true, false) => "Courier-Bold",
            (Family::Courier, false, true) => "Courier-Oblique",
            (Family::Courier, true, true) => "Courier-BoldOblique",
        }
    }

    /// The short name it goes under in the appearance stream's resources.
    pub fn resource_name(self, bold: bool, italic: bool) -> String {
        let letters = match (bold, italic) {
            (false, false) => "",
            (true, false) => "B",
            (false, true) => "I",
            (true, true) => "BI",
        };
        format!(
            "{}{letters}",
            match self {
                Family::Helvetica => "Helv",
                Family::Times => "TiRo",
                Family::Courier => "Cour",
            }
        )
    }

    pub fn from_base_font(name: &str) -> Family {
        if name.starts_with("Times") {
            Family::Times
        } else if name.starts_with("Courier") {
            Family::Courier
        } else {
            Family::Helvetica
        }
    }
}

/// Where the words sit across the box.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Across {
    #[default]
    Left,
    Middle,
    Right,
}

impl Across {
    pub fn from_q(q: i64) -> Across {
        match q {
            1 => Across::Middle,
            2 => Across::Right,
            _ => Across::Left,
        }
    }
    pub fn q(self) -> i64 {
        match self {
            Across::Left => 0,
            Across::Middle => 1,
            Across::Right => 2,
        }
    }
}

/// Where the words sit down the box.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Down {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// Everything about how the words are set.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Setting {
    pub family: Family,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub colour: [f32; 3],
    pub across: Across,
    pub down: Down,
    /// Raised or lowered, for the odd note that needs a square foot.
    pub raised: i8,
}

impl Default for Setting {
    fn default() -> Setting {
        Setting {
            family: Family::Helvetica,
            size: 12.0,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            colour: [0.0, 0.0, 0.0],
            across: Across::Left,
            down: Down::Top,
            raised: 0,
        }
    }
}

impl Setting {
    /// Reads how a markup's words are set, from the entries a PDF keeps it in.
    ///
    /// `/DA` is the one every reader agrees on: a scrap of content stream
    /// naming a font, a size and a colour. `/DS` is Revu's richer version and
    /// is read where it says something `/DA` does not.
    pub fn of(markup: &crate::Markup) -> Setting {
        let mut out = Setting {
            colour: markup.colour(),
            ..Setting::default()
        };
        let da = markup.text_of("DA");
        let words: Vec<&str> = da.split_whitespace().collect();
        for (at, word) in words.iter().enumerate() {
            if *word == "Tf" && at >= 2 {
                if let Some(name) = words[at - 2].strip_prefix('/') {
                    out.family = family_of_resource(name);
                    out.bold = name.ends_with('B') || name.ends_with("BI");
                    out.italic = name.ends_with('I');
                }
                if let Ok(size) = words[at - 1].parse::<f64>() {
                    if size > 0.0 {
                        out.size = size.clamp(2.0, 400.0);
                    }
                }
            }
            if *word == "g" && at >= 1 {
                if let Ok(grey) = words[at - 1].parse::<f32>() {
                    out.colour = [grey; 3];
                }
            }
            if *word == "rg" && at >= 3 {
                let read = |s: &str| s.parse::<f32>().ok();
                if let (Some(r), Some(g), Some(b)) =
                    (read(words[at - 3]), read(words[at - 2]), read(words[at - 1]))
                {
                    out.colour = [r, g, b];
                }
            }
        }

        // Revu's style string, where it has one.
        let ds = markup.text_of("DS");
        for part in ds.split(';') {
            let part = part.trim();
            if let Some(rest) = part.strip_prefix("font-size:") {
                if let Ok(size) = rest.trim().trim_end_matches("pt").trim().parse::<f64>() {
                    out.size = size.clamp(2.0, 400.0);
                }
            }
            if let Some(rest) = part.strip_prefix("font-weight:") {
                out.bold = rest.trim() == "bold";
            }
            if let Some(rest) = part.strip_prefix("font-style:") {
                out.italic = rest.trim() == "italic";
            }
            if let Some(rest) = part.strip_prefix("font-family:") {
                out.family = Family::from_base_font(rest.trim().trim_matches('"'));
            }
            if let Some(rest) = part.strip_prefix("text-decoration:") {
                out.underline = rest.contains("underline");
                out.strike = rest.contains("line-through");
            }
        }

        if let Some(q) = markup.dict.get("Q").and_then(|o| o.as_i64()) {
            out.across = Across::from_q(q);
        }
        out
    }

    /// Writes the setting back into the entries a reader will look at.
    pub fn onto(&self, markup: &mut crate::Markup) {
        let resource = self.family.resource_name(self.bold, self.italic);
        let colour = format!(
            "{} {} {} rg",
            pdf::write::number(self.colour[0] as f64),
            pdf::write::number(self.colour[1] as f64),
            pdf::write::number(self.colour[2] as f64)
        );
        markup.dict.set(
            Name::new("DA"),
            Object::text(&format!(
                "/{resource} {} Tf {colour}",
                pdf::write::number(self.size)
            )),
        );
        markup.dict.set(Name::new("Q"), Object::Int(self.across.q()));
        let mut decoration = Vec::new();
        if self.underline {
            decoration.push("underline");
        }
        if self.strike {
            decoration.push("line-through");
        }
        markup.dict.set(
            Name::new("DS"),
            Object::text(&format!(
                "font-family:\"{}\";font-size:{}pt;font-weight:{};font-style:{};text-decoration:{}",
                self.family.base_font(false, false),
                pdf::write::number(self.size),
                if self.bold { "bold" } else { "normal" },
                if self.italic { "italic" } else { "normal" },
                if decoration.is_empty() {
                    "none".to_string()
                } else {
                    decoration.join(" ")
                }
            )),
        );
        // The appearance was drawn with the old setting, so it has to go —
        // unless it is a picture that can only be kept.
        if !markup.keeps_its_appearance() {
            markup.dict.remove("AP");
        }
    }

    /// One font dictionary for this setting, to hang in the resources.
    pub fn font_dict(&self) -> (String, Dict) {
        let name = self.family.resource_name(self.bold, self.italic);
        let mut dict = Dict::new();
        dict.set("Type", Object::name("Font"));
        dict.set("Subtype", Object::name("Type1"));
        dict.set(
            "BaseFont",
            Object::name(self.family.base_font(self.bold, self.italic)),
        );
        dict.set("Encoding", Object::name("WinAnsiEncoding"));
        (name, dict)
    }
}

fn family_of_resource(name: &str) -> Family {
    let stem = name
        .trim_end_matches("BI")
        .trim_end_matches('B')
        .trim_end_matches('I');
    match stem {
        "TiRo" | "Times" => Family::Times,
        "Cour" | "Courier" => Family::Courier,
        _ => Family::Helvetica,
    }
}

/// The paragraphs of a text, split at every kind of line break a file may
/// carry: `\n`, the `\r` Revu writes, and the `\r\n` a pasted Windows text
/// brings. Each is a range of characters, by character, not by byte.
#[must_use]
pub fn paragraphs(text: &str) -> Vec<std::ops::Range<usize>> {
    let chars: Vec<char> = text.chars().collect(); // WHAT: the text as characters. WHY: every position here is a character's, so a degree sign or an emoji is one place, never half of one.
    let mut out = Vec::new(); // WHAT: the paragraphs found. WHY: returned.
    let mut start = 0; // WHAT: where the paragraph being read began. WHY: its range starts there.
    let mut at = 0; // WHAT: the character being looked at. WHY: walked once.
    while at < chars.len() {
        match chars[at] {
            '\n' => {
                out.push(start..at); // WHAT: the paragraph up to the break. WHY: a typed Enter.
                start = at + 1; // WHAT: the next starts after it. WHY: the break is not part of either.
            }
            '\r' => {
                out.push(start..at); // WHAT: the paragraph up to a carriage return. WHY: Revu ends its lines with one, and a box it wrote lost every line break when it was set again here.
                if chars.get(at + 1) == Some(&'\n') {
                    at += 1; // WHAT: a \r\n counts once. WHY: it is one line break, not a blank line.
                }
                start = at + 1; // WHAT: the next starts after the break. WHY: as above.
            }
            _ => {}
        }
        at += 1;
    }
    out.push(start..chars.len()); // WHAT: the last paragraph, empty when the text ends in a break. WHY: a new line with nothing on it yet is still a line.
    out
}

/// One line of wrapped words, as ranges of characters in the text it came
/// from: what is shown, and where the next line starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The first character on the line.
    pub start: usize,
    /// One past the last character shown. Spaces a wrap happened at are not
    /// shown: they are between `end` and `next`.
    pub end: usize,
    /// Where the next line's characters start.
    pub next: usize,
    /// Whether the line ends with a break somebody typed rather than one the
    /// width made.
    pub typed: bool,
}

/// Where a text's lines break at a width, in characters of the text.
///
/// The words are wrapped the way they are typed: spaces between words stay as
/// they were typed, two of them as two, and a line indented with spaces stays
/// indented. Only the spaces a line was wrapped at go, as they do in every
/// word processor. This is the one wrapping both the box being typed in and
/// the saved drawing use, so a line typed on the sheet ends where the saved
/// drawing ends it.
///
/// Words that are themselves too long for the box are broken rather than
/// allowed to run out past the edge, because a note that runs off the sheet is
/// a note nobody reads.
#[must_use]
pub fn breaks(text: &str, width: f64, setting: &Setting) -> Vec<Line> {
    let chars: Vec<char> = text.chars().collect(); // WHAT: the text as characters. WHY: lines are ranges of characters.
    let measure = |from: usize, to: usize| -> f64 {
        let piece: String = chars[from..to].iter().collect(); // WHAT: the characters between two places. WHY: measured as one run.
        crate::content::width_of_in(&piece, setting.size, setting.family, setting.bold) // WHAT: its width in the font it is set in. WHY: a Courier note is wider than a Helvetica one.
    };
    let mut out = Vec::new(); // WHAT: the lines. WHY: returned.
    let paragraphs = paragraphs(text); // WHAT: the typed lines. WHY: each wraps on its own.
    let count = paragraphs.len(); // WHAT: how many. WHY: the last one's end is the text's end, not a break.
    for (p, range) in paragraphs.into_iter().enumerate() {
        let after = if p + 1 < count {
            // WHAT: where the next paragraph starts. WHY: past this one's line break, one or two characters.
            let mut n = range.end + 1;
            if chars.get(range.end) == Some(&'\r') && chars.get(range.end + 1) == Some(&'\n') {
                n += 1;
            }
            n
        } else {
            range.end
        };
        if width <= 0.0 || !width.is_finite() {
            // WHAT: no width to wrap at, one line per paragraph. WHY: a box that grows with its words breaks only where Enter was pressed.
            out.push(Line { start: range.start, end: range.end, next: after, typed: p + 1 < count });
            continue;
        }
        let mut start = range.start; // WHAT: the first character of the line being filled. WHY: lines are measured from it.
        let mut shown = range.start; // WHAT: one past the last word on that line. WHY: where it ends if the next word does not fit.
        let mut has_word = false; // WHAT: whether the line has a word on it yet. WHY: a line always takes its first word, however long.
        let mut at = range.start;
        while at < range.end {
            // WHAT: the spaces before the next word, then the word. WHY: a word is what wraps, with the spaces in front of it.
            while at < range.end && chars[at].is_whitespace() {
                at += 1;
            }
            let word = at;
            while at < range.end && !chars[at].is_whitespace() {
                at += 1;
            }
            if word == at {
                // WHAT: spaces with no word after them, at the end of the paragraph. WHY: kept on the line, as typed.
                shown = at;
                break;
            }
            if has_word && measure(start, at) > width {
                // WHAT: the word does not fit: the line ends before the spaces in front of it. WHY: a wrap happens at a space and the space is not shown.
                out.push(Line { start, end: shown, next: word, typed: false });
                start = word;
            }
            shown = at;
            has_word = true;
            // WHAT: a word wider than the box on its own, broken. WHY: a note that runs off the sheet is a note nobody reads.
            while measure(start, shown) > width && shown - start > 1 {
                let mut cut = shown - 1;
                while cut > start + 1 && measure(start, cut) > width {
                    cut -= 1;
                }
                out.push(Line { start, end: cut, next: cut, typed: false });
                start = cut;
            }
        }
        if !has_word {
            shown = range.end; // WHAT: a paragraph of spaces or nothing. WHY: kept, as typed.
        }
        out.push(Line { start, end: shown.max(start), next: after, typed: p + 1 < count });
    }
    out
}

/// Breaks a run of words into lines that fit a width: the words of each line
/// from [`breaks`].
pub fn wrap(text: &str, width: f64, setting: &Setting) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect(); // WHAT: the text as characters. WHY: the lines are ranges of them.
    breaks(text, width, setting)
        .into_iter()
        .map(|line| chars[line.start..line.end].iter().collect()) // WHAT: each line's characters. WHY: what is drawn.
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Markup, Subtype};

    #[test]
    fn a_setting_written_and_read_back_is_the_same_setting() {
        let mut markup = Markup::new(Subtype::FreeText);
        let setting = Setting {
            family: Family::Times,
            size: 18.0,
            bold: true,
            italic: false,
            underline: true,
            strike: false,
            colour: [1.0, 0.0, 0.0],
            across: Across::Middle,
            down: Down::Top,
            raised: 0,
        };
        setting.onto(&mut markup);
        let back = Setting::of(&markup);
        assert_eq!(back.family, Family::Times);
        assert_eq!(back.size, 18.0);
        assert!(back.bold);
        assert!(!back.italic);
        assert!(back.underline);
        assert_eq!(back.across, Across::Middle);
        assert_eq!(back.colour, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn a_markup_from_revu_is_read_the_way_revu_wrote_it() {
        // /DA is what every reader agrees on, and it is what a Revu tool
        // carries.
        let mut markup = Markup::new(Subtype::FreeText);
        markup.set("DA", pdf::Object::text("/HelvB 14 Tf 1 0 0 rg"));
        let setting = Setting::of(&markup);
        assert_eq!(setting.size, 14.0);
        assert!(setting.bold);
        assert_eq!(setting.colour, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn every_font_is_one_a_reader_already_has() {
        // A markup naming a font off somebody's machine arrives set in
        // whatever the other machine had instead.
        for family in Family::ALL {
            for bold in [false, true] {
                for italic in [false, true] {
                    let name = family.base_font(bold, italic);
                    assert!(
                        [
                            "Helvetica", "Helvetica-Bold", "Helvetica-Oblique",
                            "Helvetica-BoldOblique", "Times-Roman", "Times-Bold",
                            "Times-Italic", "Times-BoldItalic", "Courier",
                            "Courier-Bold", "Courier-Oblique", "Courier-BoldOblique",
                        ]
                        .contains(&name),
                        "{name} is not one of the base fourteen"
                    );
                }
            }
        }
    }

    #[test]
    fn words_wrap_to_the_box_they_are_in() {
        let setting = Setting::default();
        let lines = wrap(
            "FIELD VERIFY ALL DIMENSIONS BEFORE FABRICATION SEE DETAIL 3 ON S-501",
            120.0,
            &setting,
        );
        assert!(lines.len() > 3, "{lines:?}");
        for line in &lines {
            assert!(
                crate::content::width_of(line, setting.size, false) <= 121.0,
                "{line:?} is too wide"
            );
        }
    }

    #[test]
    fn a_word_too_long_for_the_box_is_broken_rather_than_left_hanging_out() {
        let setting = Setting::default();
        let lines = wrap("WWWWWWWWWWWWWWWWWWWWWWWWWWWWWW", 40.0, &setting);
        assert!(lines.len() > 1, "{lines:?}");
        for line in &lines {
            assert!(crate::content::width_of(line, setting.size, false) <= 41.0);
        }
    }

    #[test]
    fn line_breaks_somebody_typed_are_kept() {
        let lines = wrap("FIRST\nSECOND", 400.0, &Setting::default());
        assert_eq!(lines, vec!["FIRST".to_string(), "SECOND".to_string()]);
    }

    #[test]
    fn line_breaks_revu_writes_are_kept_when_the_words_are_set_again() {
        // WHAT: Revu ends a text box's lines with \r; Windows text pasted in has \r\n. WHY: both came back as one long line when the box was set again here.
        let setting = Setting::default();
        assert_eq!(wrap("FIRST\rSECOND", 400.0, &setting), vec!["FIRST", "SECOND"]);
        assert_eq!(wrap("FIRST\r\nSECOND", 400.0, &setting), vec!["FIRST", "SECOND"], "one break, not a blank line");
        assert_eq!(wrap("A\n", 400.0, &setting), vec!["A", ""], "a new line with nothing on it yet");
    }

    #[test]
    fn spaces_typed_are_drawn_as_they_were_typed() {
        // WHAT: two spaces stay two, an indent stays an indent. WHY: the box being typed in shows them, and the saved drawing used to close them up.
        let setting = Setting::default();
        assert_eq!(wrap("A  B", 400.0, &setting), vec!["A  B"]);
        assert_eq!(wrap("   INDENTED", 400.0, &setting), vec!["   INDENTED"]);
        assert_eq!(wrap("TAB\tSTOP", 400.0, &setting), vec!["TAB\tSTOP"]);
    }

    #[test]
    fn a_wrap_never_loses_or_moves_a_character() {
        // WHAT: every character of the text is on a line, or is a space a line wrapped at, or a line break, in order. WHY: the box being typed in shows the same text broken at the same places, so nothing may go missing in between.
        let setting = Setting::default();
        let texts = [
            "FIELD VERIFY ALL DIMENSIONS BEFORE FABRICATION SEE DETAIL 3 ON S-501",
            "W12x26 @ 16\" O.C.  ±½\" TYP\r\nSEE  NOTE 4°\n\n   LAST 😀 LINE   ",
            "WWWWWWWWWWWWWWWWWWWWWWWWWWWWWW and then some more words",
            "",
            "\n\r\r\n",
        ];
        for text in texts {
            let chars: Vec<char> = text.chars().collect();
            for width in [5.0, 40.0, 120.0, 400.0, f64::INFINITY] {
                let lines = breaks(text, width, &setting);
                let mut at = 0;
                for line in &lines {
                    assert_eq!(line.start, at, "{text:?} at {width}: {lines:?}");
                    assert!(line.start <= line.end && line.end <= line.next, "{lines:?}");
                    for c in &chars[line.end..line.next] {
                        assert!(c.is_whitespace(), "{c:?} was dropped from {text:?} at {width}");
                    }
                    at = line.next;
                }
                assert_eq!(at, chars.len(), "{text:?} at {width}: the end was lost");
            }
        }
    }

    #[test]
    fn courier_is_measured_as_courier() {
        // WHAT: every Courier letter is 0.6 of the size wide. WHY: measured as Helvetica, a Courier note wrapped too late and ran out of its box.
        assert_eq!(crate::content::width_of_in("iiii", 10.0, Family::Courier, false), 24.0);
        assert_eq!(crate::content::width_of_in("iiii", 10.0, Family::Helvetica, false), crate::content::width_of("iiii", 10.0, false));
    }
}
