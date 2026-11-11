//! Building PDF content streams: the drawing operators that make a markup
//! visible in every reader, not just in Hyperview.
//!
//! An annotation without an appearance stream is data with nothing to look at.
//! Pdfium draws nothing for one; Acrobat draws its own idea of one. So Hyperview
//! writes the appearance itself, and what the user sees is what everybody sees.

use pdf::write::number;

#[derive(Default)]
pub struct Content {
    bytes: Vec<u8>,
}

impl Content {
    pub fn new() -> Content {
        Content::default()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    fn word(&mut self, text: &str) -> &mut Content {
        self.bytes.extend_from_slice(text.as_bytes());
        self.bytes.push(b'\n');
        self
    }

    fn numbers(&mut self, values: &[f64], operator: &str) -> &mut Content {
        let mut line = String::new();
        for v in values {
            line.push_str(&number(*v));
            line.push(' ');
        }
        line.push_str(operator);
        self.word(&line)
    }

    pub fn save(&mut self) -> &mut Content {
        self.word("q")
    }

    pub fn restore(&mut self) -> &mut Content {
        self.word("Q")
    }

    pub fn stroke_colour(&mut self, c: [f32; 3]) -> &mut Content {
        self.numbers(&[c[0] as f64, c[1] as f64, c[2] as f64], "RG")
    }

    pub fn fill_colour(&mut self, c: [f32; 3]) -> &mut Content {
        self.numbers(&[c[0] as f64, c[1] as f64, c[2] as f64], "rg")
    }

    pub fn line_width(&mut self, width: f64) -> &mut Content {
        self.numbers(&[width.max(0.0)], "w")
    }

    /// 0 butt, 1 round, 2 square.
    pub fn line_cap(&mut self, cap: i32) -> &mut Content {
        self.numbers(&[cap as f64], "J")
    }

    pub fn line_join(&mut self, join: i32) -> &mut Content {
        self.numbers(&[join as f64], "j")
    }

    pub fn dash(&mut self, pattern: &[f64], phase: f64) -> &mut Content {
        let mut line = String::from("[");
        for (i, v) in pattern.iter().enumerate() {
            if i > 0 {
                line.push(' ');
            }
            line.push_str(&number(*v));
        }
        line.push_str("] ");
        line.push_str(&number(phase));
        line.push_str(" d");
        self.word(&line)
    }

    /// Names a graphics state from the resources, which is how opacity is set.
    pub fn graphics_state(&mut self, name: &str) -> &mut Content {
        self.word(&format!("/{name} gs"))
    }

    pub fn move_to(&mut self, p: [f64; 2]) -> &mut Content {
        self.numbers(&[p[0], p[1]], "m")
    }

    pub fn line_to(&mut self, p: [f64; 2]) -> &mut Content {
        self.numbers(&[p[0], p[1]], "l")
    }

    pub fn curve_to(&mut self, a: [f64; 2], b: [f64; 2], to: [f64; 2]) -> &mut Content {
        self.numbers(&[a[0], a[1], b[0], b[1], to[0], to[1]], "c")
    }

    /// Concatenates a matrix onto the current one.
    pub fn matrix(&mut self, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> &mut Content {
        self.numbers(&[a, b, c, d, e, f], "cm")
    }

    /// Draws a named XObject — a picture, or a form.
    pub fn draw_object(&mut self, name: &str) -> &mut Content {
        self.word(&format!("/{name} Do"))
    }

    pub fn rectangle(&mut self, x: f64, y: f64, w: f64, h: f64) -> &mut Content {
        self.numbers(&[x, y, w, h], "re")
    }

    pub fn close(&mut self) -> &mut Content {
        self.word("h")
    }

    pub fn stroke(&mut self) -> &mut Content {
        self.word("S")
    }

    pub fn fill(&mut self) -> &mut Content {
        self.word("f")
    }

    pub fn fill_and_stroke(&mut self) -> &mut Content {
        self.word("B")
    }

    pub fn clip(&mut self) -> &mut Content {
        self.word("W n")
    }

    /// A polyline, optionally closed. Returns without drawing if there is
    /// nothing to draw, so an empty markup cannot produce a broken stream.
    pub fn path(&mut self, points: &[[f64; 2]], closed: bool) -> &mut Content {
        if points.len() < 2 {
            return self;
        }
        self.move_to(points[0]);
        for p in &points[1..] {
            self.line_to(*p);
        }
        if closed {
            self.close();
        }
        self
    }

    /// An ellipse inscribed in a rectangle, as four bezier arcs.
    pub fn ellipse(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) -> &mut Content {
        // The classic circle-from-beziers constant.
        const K: f64 = 0.5523;
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let (rx, ry) = ((x1 - x0).abs() / 2.0, (y1 - y0).abs() / 2.0);
        let (ox, oy) = (rx * K, ry * K);
        self.move_to([cx - rx, cy]);
        self.curve_to([cx - rx, cy + oy], [cx - ox, cy + ry], [cx, cy + ry]);
        self.curve_to([cx + ox, cy + ry], [cx + rx, cy + oy], [cx + rx, cy]);
        self.curve_to([cx + rx, cy - oy], [cx + ox, cy - ry], [cx, cy - ry]);
        self.curve_to([cx - ox, cy - ry], [cx - rx, cy - oy], [cx - rx, cy]);
        self.close()
    }

    /// One line of text at a point, in the named font.
    pub fn text(&mut self, font: &str, size: f64, at: [f64; 2], colour: [f32; 3], text: &str) -> &mut Content {
        self.word("BT");
        self.word(&format!("/{font} {} Tf", number(size)));
        self.numbers(&[colour[0] as f64, colour[1] as f64, colour[2] as f64], "rg");
        self.numbers(&[1.0, 0.0, 0.0, 1.0, at[0], at[1]], "Tm");
        self.show(text);
        self.word("Tj");
        self.word("ET")
    }

    /// Text turned to run along a direction given as cosine and sine.
    pub fn text_turned(
        &mut self,
        font: &str,
        size: f64,
        at: [f64; 2],
        turn: [f64; 2],
        colour: [f32; 3],
        text: &str,
    ) -> &mut Content {
        self.word("BT");
        self.word(&format!("/{font} {} Tf", number(size)));
        self.numbers(&[colour[0] as f64, colour[1] as f64, colour[2] as f64], "rg");
        self.numbers(
            &[turn[0], turn[1], -turn[1], turn[0], at[0], at[1]],
            "Tm",
        );
        self.show(text);
        self.word("Tj");
        self.word("ET")
    }

    fn show(&mut self, text: &str) {
        self.bytes.push(b'(');
        for ch in composed(text).chars() {
            let b = to_win_ansi(ch);
            match b {
                b'(' | b')' | b'\\' => {
                    self.bytes.push(b'\\');
                    self.bytes.push(b);
                }
                0x20..=0x7e => self.bytes.push(b),
                _ => self
                    .bytes
                    .extend_from_slice(format!("\\{b:03o}").as_bytes()),
            }
        }
        self.bytes.push(b')');
        self.bytes.push(b'\n');
    }
}

/// A letter followed by a combining accent, as the one letter WinAnsi has
/// for the pair: `e` + U+0301 is `é`. Text pasted from a Mac or a web page
/// often comes this way, and each accent on its own became a question mark
/// after its letter.
pub fn composed(text: &str) -> String {
    // WHAT: for each combining accent, the letters it goes on and what they become. WHY: only the pairs WinAnsi has a letter for.
    const PAIRS: &[(char, &str, &str)] = &[
        ('\u{300}', "AEIOUaeiou", "ÀÈÌÒÙàèìòù"),
        ('\u{301}', "AEIOUYaeiouy", "ÁÉÍÓÚÝáéíóúý"),
        ('\u{302}', "AEIOUaeiou", "ÂÊÎÔÛâêîôû"),
        ('\u{303}', "ANOano", "ÃÑÕãñõ"),
        ('\u{308}', "AEIOUaeiouy", "ÄËÏÖÜäëïöüÿ"),
        ('\u{30a}', "Aa", "Åå"),
        ('\u{327}', "Cc", "Çç"),
    ];
    let mut out = String::with_capacity(text.len()); // WHAT: the text with pairs made one. WHY: returned.
    for ch in text.chars() {
        let pair = PAIRS.iter().find(|(mark, _, _)| *mark == ch); // WHAT: whether this is an accent with letters. WHY: anything else goes through as it is.
        if let (Some((_, bases, results)), Some(last)) = (pair, out.chars().last()) {
            if let Some(at) = bases.chars().position(|b| b == last) {
                out.pop(); // WHAT: the letter goes. WHY: it is replaced by the letter with its accent.
                out.push(results.chars().nth(at).unwrap_or(last)); // WHAT: the accented letter. WHY: one character WinAnsi can set.
                continue;
            }
        }
        out.push(ch);
    }
    out
}

/// Maps a character into WinAnsi, which is the encoding the base fonts use.
/// Anything with no place there becomes a question mark rather than silently
/// shifting every following character.
pub fn to_win_ansi(ch: char) -> u8 {
    match ch {
        '\u{0}'..='\u{7f}' => ch as u8,
        '\u{a0}'..='\u{ff}' => ch as u8,
        '\u{2018}' => 0x91,
        '\u{2019}' => 0x92,
        '\u{201c}' => 0x93,
        '\u{201d}' => 0x94,
        '\u{2013}' => 0x96,
        '\u{2014}' => 0x97,
        '\u{2022}' => 0x95,
        '\u{2026}' => 0x85,
        '\u{20ac}' => 0x80,
        '\u{2032}' => b'\'',
        '\u{2033}' => b'"',
        _ => b'?',
    }
}

/// Helvetica character widths, in thousandths of the point size. Captions have
/// to be centred on a measurement, and centring them needs real metrics.
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // 32..47
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, // 48..63
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, // 64..79
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, // 80..95
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, // 96..111
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, // 112..126
];

const HELVETICA_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, //
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, //
    975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, //
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556, //
    333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, //
    611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584, //
];

/// How wide a string will be in a family, in points. Courier is the same
/// width for every letter; Times is measured as Helvetica, the same as the
/// words typed on the sheet are shown, so the two wrap alike.
pub fn width_of_in(text: &str, size: f64, family: crate::text::Family, bold: bool) -> f64 {
    match family {
        crate::text::Family::Courier => text.chars().count() as f64 * 600.0 * size / 1000.0, // WHAT: 600 thousandths of the size for every letter. WHY: Courier is fixed-width, and measuring it as Helvetica wrapped a Courier note far too late.
        _ => width_of(text, size, bold), // WHAT: Helvetica's widths. WHY: as before.
    }
}

/// How wide a string will be, in points.
pub fn width_of(text: &str, size: f64, bold: bool) -> f64 {
    let table = if bold { &HELVETICA_BOLD } else { &HELVETICA };
    let mut total = 0u32;
    for ch in composed(text).chars() { // WHAT: measured as it is drawn, accents joined to their letters. WHY: an accent measured on its own made the line wider than what was drawn.
        let b = to_win_ansi(ch);
        let w = if (32..127).contains(&b) {
            table[(b - 32) as usize]
        } else {
            556
        };
        total += w as u32;
    }
    total as f64 * size / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(c: Content) -> String {
        String::from_utf8(c.into_bytes()).unwrap()
    }

    #[test]
    fn a_line_comes_out_as_the_operators_a_reader_expects() {
        let mut c = Content::new();
        c.save()
            .stroke_colour([1.0, 0.0, 0.0])
            .line_width(2.0)
            .path(&[[10.0, 20.0], [110.0, 20.0]], false)
            .stroke()
            .restore();
        assert_eq!(
            text_of(c),
            "q\n1 0 0 RG\n2 w\n10 20 m\n110 20 l\nS\nQ\n"
        );
    }

    #[test]
    fn a_path_with_nothing_in_it_draws_nothing_rather_than_half_an_operator() {
        let mut c = Content::new();
        c.path(&[], false).path(&[[1.0, 1.0]], true);
        assert!(c.is_empty());
    }

    #[test]
    fn text_is_escaped_so_a_bracket_in_a_note_cannot_break_the_stream() {
        let mut c = Content::new();
        c.text("F1", 12.0, [0.0, 0.0], [0.0, 0.0, 0.0], "L3x2x1/4 (typ)");
        let out = text_of(c);
        assert!(out.contains(r"(L3x2x1/4 \(typ\))"), "{out}");
    }

    #[test]
    fn a_curly_quote_becomes_its_plain_equivalent_not_a_shifted_alphabet() {
        assert_eq!(to_win_ansi('\u{2032}'), b'\'');
        assert_eq!(to_win_ansi('\u{201d}'), 0x94);
        assert_eq!(to_win_ansi('\u{4e00}'), b'?');
    }

    #[test]
    fn measured_text_width_matches_helvetica() {
        // Known values: a 12pt space is 3.336pt, "W" is 11.328pt.
        assert!((width_of(" ", 12.0, false) - 3.336).abs() < 1e-6);
        assert!((width_of("W", 12.0, false) - 11.328).abs() < 1e-6);
        // Digits are all one width, so a dimension string is predictable.
        assert!((width_of("12", 12.0, false) - width_of("99", 12.0, false)).abs() < 1e-9);
        assert!(width_of("20'-0\"", 12.0, false) > 0.0);
    }

    #[test]
    fn an_ellipse_closes_its_path() {
        let mut c = Content::new();
        c.ellipse(0.0, 0.0, 100.0, 50.0);
        let out = text_of(c);
        assert_eq!(out.matches(" c\n").count(), 4);
        assert!(out.ends_with("h\n"));
    }

    #[test]
    fn an_accent_typed_after_its_letter_is_drawn_as_one_letter() {
        // WHAT: e + combining acute is drawn as é. WHY: text pasted from a Mac or a web page carries accents this way, and each became a question mark.
        let mut c = Content::new();
        c.text("F1", 10.0, [0.0, 0.0], [0.0, 0.0, 0.0], "Cafe\u{301} NIN\u{303}O");
        let drawn = text_of(c);
        assert!(drawn.contains("(Caf\\351 NI\\321O)"), "{drawn}");
        assert_eq!(width_of("e\u{301}", 10.0, false), width_of("é", 10.0, false));
    }
}
