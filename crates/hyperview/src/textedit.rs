//! Changing the words already on a sheet.
//!
//! Click text on a drawing and the paragraph it is in opens for typing, right
//! where it is. Change the words, the size, the font, the colour or how it
//! lines up, and it is written back into the page: the old text taken out, the
//! new text set in the same place and wrapped to the same width.
//!
//! What a page calls text is a scatter of pieces — a word, a line, sometimes a
//! single letter each — with nothing that says which belong together. So the
//! pieces are put back together the way a reader would: pieces on one
//! baseline and close enough are one line, and lines of one size, stacked at
//! one spacing and lined up with each other, are one paragraph. All of it is
//! done along the text's own direction, so a note turned sideways on a
//! rotated sheet is read and written the same as one that isn't.
//!
//! A file's own font is kept when it can set every letter typed. A font that
//! was cut down to the letters the drawing used can only set those again; a
//! new letter means the paragraph is set in a standard font instead, and the
//! window says so before anything is written.
//!
//! Nothing is written unless the rest of the sheet is exactly as it was: the
//! sheet is drawn before and after with the paragraph left out of both, and
//! if they differ anywhere the change is thrown away and the drawing left
//! alone. Undo takes the text back to what it was.

/// One piece of text on a page, as the page has it.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    /// Which object on the page, in the page's own order.
    pub object: usize,
    pub text: String,
    /// Where its baseline starts, in PDF space.
    pub origin: [f64; 2],
    /// The way the text runs, as a unit vector in PDF space.
    pub along: [f64; 2],
    /// The height of its letters, in points.
    pub size: f64,
    /// How far it runs along its baseline, in points.
    pub advance: f64,
    pub colour: [u8; 3],
    /// Which font, as the page names it, and what it looks like.
    pub font: String,
    pub bold: bool,
    pub italic: bool,
}

/// How a paragraph's lines line up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Centre,
    Right,
}

impl Align {
    pub const ALL: &'static [Align] = &[Align::Left, Align::Centre, Align::Right];

    pub fn name(self) -> &'static str {
        match self {
            Align::Left => "Left",
            Align::Centre => "Centre",
            Align::Right => "Right",
        }
    }
}

/// The font to set new words in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Family {
    /// The one the paragraph already has, when it can set every letter.
    #[default]
    Keep,
    Sans,
    Serif,
    Mono,
}

impl Family {
    pub const ALL: &'static [Family] = &[Family::Keep, Family::Sans, Family::Serif, Family::Mono];

    pub fn name(self) -> &'static str {
        match self {
            Family::Keep => "Its own font",
            Family::Sans => "Helvetica",
            Family::Serif => "Times",
            Family::Mono => "Courier",
        }
    }

    /// The standard font of this family, as PDF names it.
    pub fn standard(self, bold: bool, italic: bool) -> &'static str {
        match (self, bold, italic) {
            (Family::Serif, false, false) => "Times-Roman",
            (Family::Serif, true, false) => "Times-Bold",
            (Family::Serif, false, true) => "Times-Italic",
            (Family::Serif, true, true) => "Times-BoldItalic",
            (Family::Mono, false, false) => "Courier",
            (Family::Mono, true, false) => "Courier-Bold",
            (Family::Mono, false, true) => "Courier-Oblique",
            (Family::Mono, true, true) => "Courier-BoldOblique",
            (_, false, false) => "Helvetica",
            (_, true, false) => "Helvetica-Bold",
            (_, false, true) => "Helvetica-Oblique",
            (_, true, true) => "Helvetica-BoldOblique",
        }
    }

    /// The standard family nearest a font the file names.
    pub fn nearest(font: &str) -> Family {
        let f = font.to_lowercase();
        if f.contains("courier") || f.contains("mono") || f.contains("consol") {
            Family::Mono
        } else if f.contains("times") || f.contains("serif") && !f.contains("sans") || f.contains("roman") || f.contains("georgia") || f.contains("garamond") {
            Family::Serif
        } else {
            Family::Sans
        }
    }
}

/// A paragraph found on a page: which pieces it is made of, what it says,
/// and where and how it is set. Everything in PDF space unless it says.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The pieces, in reading order: their words and where each starts. Used
    /// to find them again when the change is written, in case the page has
    /// been read again since.
    pub pieces: Vec<(String, [f64; 2])>,
    /// The objects they are, in the page's order.
    pub objects: Vec<usize>,
    /// Its lines as they are on the page.
    pub lines: Vec<String>,
    /// The way the text runs, and the way is up from it.
    pub along: [f64; 2],
    pub up: [f64; 2],
    /// In the text's own space: where the lines start, the baseline of the
    /// first, how wide the widest runs, the gap from one baseline to the next.
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub spacing: f64,
    pub size: f64,
    pub colour: [u8; 3],
    pub font: String,
    pub bold: bool,
    pub italic: bool,
    pub align: Align,
}

impl Found {
    /// What it says, with its lines run together the way a paragraph reads.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if out.ends_with('-') && line.chars().next().is_some_and(|c| c.is_lowercase()) {
                // A word broken over two lines comes back whole.
                out.pop();
            } else if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(line);
        }
        out
    }

    /// A point in the text's own space, in PDF space.
    pub fn to_page(&self, x: f64, y: f64) -> [f64; 2] {
        [x * self.along[0] + y * self.up[0], x * self.along[1] + y * self.up[1]]
    }

    /// The four corners of the paragraph, in PDF space: the tops of its first
    /// line's letters to the bottoms of its last's.
    pub fn corners(&self) -> [[f64; 2]; 4] {
        let top = self.top + self.size * 0.85;
        let bottom = self.top - self.spacing * (self.lines.len().max(1) - 1) as f64 - self.size * 0.3;
        [
            self.to_page(self.left, bottom),
            self.to_page(self.left + self.width, bottom),
            self.to_page(self.left + self.width, top),
            self.to_page(self.left, top),
        ]
    }

    /// Whether a point in PDF space is on it.
    pub fn holds(&self, p: [f64; 2]) -> bool {
        let x = p[0] * self.along[0] + p[1] * self.along[1];
        let y = p[0] * self.up[0] + p[1] * self.up[1];
        let top = self.top + self.size * 0.85;
        let bottom = self.top - self.spacing * (self.lines.len().max(1) - 1) as f64 - self.size * 0.3;
        let pad = self.size * 0.2;
        x >= self.left - pad && x <= self.left + self.width + pad && y >= bottom - pad && y <= top + pad
    }
}

/// What the paragraph is to become.
#[derive(Clone, Debug, PartialEq)]
pub struct Rewrite {
    pub text: String,
    pub size: f64,
    pub colour: [u8; 3],
    pub family: Family,
    pub bold: bool,
    pub italic: bool,
    pub align: Align,
    /// How wide it may run before it wraps, in points.
    pub width: f64,
}

impl Rewrite {
    pub fn of(found: &Found) -> Rewrite {
        Rewrite {
            text: found.text(),
            size: found.size,
            colour: found.colour,
            family: Family::Keep,
            bold: found.bold,
            italic: found.italic,
            align: found.align,
            width: found.width,
        }
    }
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// A piece measured in its own direction's space.
struct Placed {
    run: usize,
    x: f64,
    end: f64,
    y: f64,
}

struct Line {
    runs: Vec<usize>,
    x: f64,
    end: f64,
    y: f64,
    size: f64,
    text: String,
}

/// The paragraphs on a page, from its pieces of text.
pub fn paragraphs(runs: &[Run]) -> Vec<Found> {
    // Each direction on its own: a sheet can have notes running three ways.
    let mut directions: Vec<([f64; 2], Vec<usize>)> = Vec::new();
    for (i, run) in runs.iter().enumerate() {
        if run.text.trim().is_empty() || run.size <= 0.0 {
            continue;
        }
        match directions.iter_mut().find(|(d, _)| dot(*d, run.along) > 0.9998) {
            Some((_, members)) => members.push(i),
            None => directions.push((run.along, vec![i])),
        }
    }
    let mut out = Vec::new();
    for (along, members) in directions {
        let up = [-along[1], along[0]];
        let mut placed: Vec<Placed> = members
            .iter()
            .map(|&i| {
                let r = &runs[i];
                let x = dot(r.origin, along);
                Placed { run: i, x, end: x + r.advance.max(0.0), y: dot(r.origin, up) }
            })
            .collect();
        // Top to bottom, then left to right.
        placed.sort_by(|a, b| b.y.total_cmp(&a.y).then(a.x.total_cmp(&b.x)));

        // Lines: pieces on one baseline, close enough to read as one.
        let mut lines: Vec<Line> = Vec::new();
        for p in placed {
            let r = &runs[p.run];
            let joined = lines.iter_mut().rev().take(12).find(|l| {
                (l.y - p.y).abs() < 0.35 * l.size.min(r.size)
                    && (r.size / l.size) > 0.8
                    && (r.size / l.size) < 1.25
                    && p.x - l.end < 1.2 * l.size
                    && p.x > l.x - 0.5 * l.size
            });
            match joined {
                Some(line) => {
                    let gap = p.x - line.end;
                    if gap > 0.22 * line.size && !line.text.ends_with(' ') && !r.text.starts_with(' ') {
                        line.text.push(' ');
                    }
                    line.text.push_str(&r.text);
                    line.runs.push(p.run);
                    line.end = line.end.max(p.end);
                }
                None => lines.push(Line {
                    runs: vec![p.run],
                    x: p.x,
                    end: p.end,
                    y: p.y,
                    size: r.size,
                    text: r.text.clone(),
                }),
            }
        }
        lines.sort_by(|a, b| b.y.total_cmp(&a.y).then(a.x.total_cmp(&b.x)));

        // Paragraphs: lines of one size, stacked at one spacing, lined up.
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let home = groups.iter_mut().rev().take(24).find(|g| {
                let last = &lines[*g.last().unwrap()];
                let first = &lines[g[0]];
                let step = last.y - line.y;
                let size_ok = (line.size / last.size) > 0.85 && (line.size / last.size) < 1.18;
                let spacing_ok = step > 0.8 * last.size && step < 2.2 * last.size;
                let same_spacing = g.len() < 2 || {
                    let before = &lines[g[g.len() - 2]];
                    ((before.y - last.y) - step).abs() < 0.35 * last.size
                };
                let (left, right) = g.iter().fold((f64::MAX, f64::MIN), |(l, r), k| {
                    (l.min(lines[*k].x), r.max(lines[*k].end))
                });
                let overlaps = line.x < right + last.size && line.end > left - last.size;
                let lined_up = (line.x - first.x).abs() < 1.5 * last.size
                    || ((line.x + line.end) * 0.5 - (first.x + first.end) * 0.5).abs() < 1.0 * last.size
                    || (line.end - first.end).abs() < 1.0 * last.size;
                // The next note in a numbered list is its own paragraph, even
                // set at the same spacing as the one before it.
                let next_item = starts_an_item(&line.text) && ends_a_sentence(&last.text);
                size_ok && spacing_ok && same_spacing && overlaps && lined_up && !next_item
            });
            match home {
                Some(g) => g.push(i),
                None => groups.push(vec![i]),
            }
        }

        for g in groups {
            let first = &lines[g[0]];
            let left = g.iter().map(|k| lines[*k].x).fold(f64::MAX, f64::min);
            let right = g.iter().map(|k| lines[*k].end).fold(f64::MIN, f64::max);
            let size = first.size;
            let spacing = if g.len() > 1 {
                (first.y - lines[*g.last().unwrap()].y) / (g.len() - 1) as f64
            } else {
                size * 1.2
            };
            let align = if g.len() < 2 {
                Align::Left
            } else {
                let spread = |f: &dyn Fn(&Line) -> f64| {
                    let values: Vec<f64> = g.iter().map(|k| f(&lines[*k])).collect();
                    values.iter().cloned().fold(f64::MIN, f64::max) - values.iter().cloned().fold(f64::MAX, f64::min)
                };
                let lefts = spread(&|l: &Line| l.x);
                let centres = spread(&|l: &Line| (l.x + l.end) * 0.5);
                let rights = spread(&|l: &Line| l.end);
                if lefts <= 0.5 * size || (lefts <= centres && lefts <= rights) {
                    Align::Left
                } else if centres <= rights {
                    Align::Centre
                } else {
                    Align::Right
                }
            };
            let run_order: Vec<usize> = g.iter().flat_map(|k| lines[*k].runs.clone()).collect();
            let sample = &runs[run_order[0]];
            out.push(Found {
                pieces: run_order.iter().map(|i| (runs[*i].text.clone(), runs[*i].origin)).collect(),
                objects: run_order.iter().map(|i| runs[*i].object).collect(),
                lines: g.iter().map(|k| lines[*k].text.clone()).collect(),
                along,
                up,
                left,
                top: first.y,
                width: (right - left).max(size),
                spacing,
                size,
                colour: sample.colour,
                font: sample.font.clone(),
                bold: sample.bold,
                italic: sample.italic,
                align,
            });
        }
    }
    out
}

/// Words wrapped to a width, keeping the line breaks that were typed. A word
/// wider than the width gets a line of its own rather than being cut.
pub fn wrap(text: &str, width: f64, measure: &dyn Fn(&str) -> f64) -> Vec<String> {
    let mut out = Vec::new();
    for typed in text.split('\n') {
        let mut line = String::new();
        for word in typed.split_whitespace() {
            let tried = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if !line.is_empty() && measure(&tried) > width + 0.01 {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = tried;
            }
        }
        out.push(line);
    }
    // No empty line at the very end from a trailing Enter.
    while out.len() > 1 && out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out
}

/// Where each new line starts, in PDF space: the paragraph's left edge (or
/// its middle, or its right, as it is aligned), its first baseline, then one
/// spacing down for each line after.
pub fn origins(found: &Found, widths: &[f64], align: Align, width: f64, spacing: f64) -> Vec<[f64; 2]> {
    widths
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let x = match align {
                Align::Left => found.left,
                Align::Centre => found.left + (width - w) * 0.5,
                Align::Right => found.left + width - w,
            };
            found.to_page(x, found.top - spacing * i as f64)
        })
        .collect()
}

/// Whether a line begins a numbered or lettered note: "2. ", "12) ", "B. ",
/// "(3) ", "(c) ", or a bullet.
pub fn starts_an_item(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with(['\u{2022}', '\u{2013}', '-', '*']) && t.chars().nth(1).is_some_and(char::is_whitespace) {
        return true;
    }
    let (inner, rest) = match t.strip_prefix('(') {
        Some(after) => match after.find(')') {
            Some(close) => (&after[..close], &after[close + 1..]),
            None => return false,
        },
        None => match t.find(['.', ')']) {
            Some(end) => (&t[..end], &t[end + 1..]),
            None => return false,
        },
    };
    let marker = (!inner.is_empty() && inner.len() <= 3 && inner.chars().all(|c| c.is_ascii_digit()))
        || (inner.len() == 1 && inner.chars().all(|c| c.is_ascii_alphabetic()));
    marker && rest.starts_with(char::is_whitespace)
}

/// Whether a line finishes what it was saying.
fn ends_a_sentence(line: &str) -> bool {
    line.trim_end().ends_with(['.', ':', ';'])
}

/// Whether a font that was cut down to what the drawing used can set these
/// words: every letter in them has to be one it already had.
pub fn can_set(words: &str, had: &std::collections::HashSet<char>) -> bool {
    words.chars().filter(|c| !c.is_whitespace()).all(|c| had.contains(&c))
}

/// The fonts on a sheet that are in the file whole, by the name pdfium gives
/// them. One cut down to the letters the drawing used has a tag in front of
/// its name, on it or on the font inside it, and isn't among these.
pub fn whole_fonts(file: &pdf::Document, page: u32) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let Some(dict) = file.page(page as usize) else { return out };
    let resources = file.page_attr(&dict, "Resources");
    let Some(resources) = resources.as_dict() else { return out };
    let fonts = file.at(resources, "Font");
    let Some(fonts) = fonts.as_dict() else { return out };
    let name_of = |d: &pdf::Dict| d.get("BaseFont").and_then(|o| o.as_name()).map(|n| n.as_str().to_string());
    for (_, value) in fonts.iter() {
        let font = file.follow(value);
        let Some(font) = font.as_dict() else { continue };
        let Some(name) = name_of(font) else { continue };
        let inner = file.at(font, "DescendantFonts");
        let inner_cut = inner
            .as_array()
            .and_then(|a| a.first())
            .map(|d| file.follow(d))
            .and_then(|d| d.as_dict().and_then(name_of))
            .is_some_and(|n| is_subset(&n));
        if !is_subset(&name) && !inner_cut {
            out.insert(name);
        }
    }
    out
}

/// Whether a font's name says it was cut down to the letters used: six
/// capital letters and a plus sign in front, which is how every PDF writer
/// marks a subset.
pub fn is_subset(font: &str) -> bool {
    let b = font.as_bytes();
    b.len() > 7 && b[6] == b'+' && b[..6].iter().all(|c| c.is_ascii_uppercase())
}

/// A paragraph on the sheet being changed.
pub struct Rewording {
    pub page: u32,
    pub found: Found,
    pub rewrite: Rewrite,
    /// Sent to be written, and not back yet.
    pub running: bool,
    pub job: u64,
    /// The file as it was when the words were sent, so a save somewhere else
    /// in the meantime isn't written over.
    pub on_disk: Option<crate::sheet::Stamp>,
    pub error: Option<String>,
}

impl crate::app::App {
    /// Asks for the paragraphs on the sheet in view, once.
    pub fn want_paragraphs(&mut self) {
        let Some(doc) = self.doc_mut() else { return };
        let page = doc.page;
        if doc.paragraphs.contains_key(&page) || !doc.paragraphs_asked.insert(page) {
            return;
        }
        let id = doc.id;
        self.svc.send(crate::render::ToWorker::Paragraphs { doc: id, page });
    }

    /// The paragraph under a point on the screen, on the sheet in view.
    pub fn paragraph_at(&self, pointer: egui::Pos2) -> Option<usize> {
        let doc = self.doc()?;
        let found = doc.paragraphs.get(&doc.page)?;
        let s = doc.view.to_sheet(pointer);
        let at = doc.frame().to_pdf([s[0] as f64, s[1] as f64]);
        found.iter().position(|f| f.holds(at))
    }

    /// The Edit Text tool on the sheet: show what can be changed, and open
    /// what is clicked.
    pub fn edit_text_input(&mut self, ui: &egui::Ui, response: &egui::Response, pointer: egui::Pos2) {
        if self.rewording.is_some() {
            return;
        }
        self.want_paragraphs();
        self.text_under = self.paragraph_at(pointer);
        if self.text_under.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        if !response.clicked() {
            return;
        }
        let Some(doc) = self.doc() else { return };
        if doc.paragraphs.get(&doc.page).is_none() {
            self.status = "Still reading the words on this sheet.".into();
            return;
        }
        let Some(index) = self.text_under else {
            self.status = "There are no words there to change. Drawings from CAD often have \
                           their lettering as lines, which can't be retyped; a white-filled box \
                           over them and a text box on top is the way to change those."
                .into();
            return;
        };
        if doc.locked.is_some() {
            self.error = Some(
                "This drawing is locked with a password, and its words can't be changed \
                 in place. Save an unlocked copy first."
                    .into(),
            );
            return;
        }
        if doc.read_only {
            self.error = Some("This drawing is read-only. Save As somewhere you can write first.".into());
            return;
        }
        let page = doc.page;
        let Some(found) = doc.paragraphs.get(&page).and_then(|f| f.get(index)).cloned() else {
            return;
        };
        self.rewording = Some(Rewording {
            page,
            rewrite: Rewrite::of(&found),
            found,
            running: false,
            job: 0,
            on_disk: None,
            error: None,
        });
    }

    /// Outlines what can be changed while the tool is in hand: every
    /// paragraph faintly, the one under the pointer plainly.
    pub fn paint_paragraphs(&self, painter: &egui::Painter) {
        if self.tool != crate::app::Tool::EditText {
            return;
        }
        let Some(doc) = self.doc() else { return };
        let Some(found) = doc.paragraphs.get(&doc.page) else { return };
        let frame = doc.frame();
        let blue = egui::Color32::from_rgb(90, 170, 255);
        for (i, f) in found.iter().enumerate() {
            if self.rewording.as_ref().is_some_and(|r| r.found == *f) {
                continue;
            }
            let points: Vec<egui::Pos2> = f
                .corners()
                .iter()
                .map(|p| {
                    let s = frame.to_sheet(*p);
                    doc.view.to_screen([s[0] as f32, s[1] as f32])
                })
                .collect();
            let under = self.text_under == Some(i);
            if under {
                painter.add(egui::Shape::convex_polygon(
                    points.clone(),
                    egui::Color32::from_rgba_unmultiplied(90, 170, 255, 28),
                    egui::Stroke::NONE,
                ));
            }
            painter.add(egui::Shape::closed_line(
                points,
                egui::Stroke::new(
                    if under { 1.6_f32 } else { 0.8_f32 }, // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                    if under { blue } else { egui::Color32::from_rgba_unmultiplied(90, 170, 255, 110) },
                ),
            ));
        }
    }

    /// The paragraph open for changing: its words, typed on the sheet where
    /// they are, and a strip above for the size, the font and the rest.
    pub fn reword_window(&mut self, ctx: &egui::Context) {
        let Some(mut r) = self.rewording.take() else { return };
        let theme = self.chrome.theme;
        let Some(doc) = self.doc() else { return };
        if doc.page != r.page {
            // Gone to another sheet: let go without changing anything.
            return;
        }
        let frame = doc.frame();
        let zoom = doc.view.zoom;
        let corners: Vec<egui::Pos2> = r
            .found
            .corners()
            .iter()
            .map(|p| {
                let s = frame.to_sheet(*p);
                doc.view.to_screen([s[0] as f32, s[1] as f32])
            })
            .collect();
        let mut area = egui::Rect::from_points(&corners);
        // Room for more than was there: a paragraph can grow downwards.
        let width = (r.rewrite.width as f32 * zoom).max(80.0);
        area.set_width(area.width().max(width));
        let size = (r.rewrite.size as f32 * zoom).clamp(6.0, 96.0);
        let ink = egui::Color32::from_rgb(r.rewrite.colour[0], r.rewrite.colour[1], r.rewrite.colour[2]);
        let mut done = false;
        let mut cancel = false;
        // Ctrl+Enter finishes and Escape lets go. Taken before the words see
        // them, so neither becomes part of what is typed.
        if !r.running {
            ctx.input_mut(|i| {
                cancel |= i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
                done |= i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter);
            });
        }
        if cancel && !r.running {
            self.status = "Nothing was changed.".into();
            // Drawn once more without the box, so the next click lands on
            // the sheet and not on where the box was.
            ctx.request_repaint();
            return;
        }

        // The strip of settings, just above the words.
        egui::Area::new(egui::Id::new(("reword-strip", r.page)))
            .order(egui::Order::Foreground)
            .fixed_pos(area.left_top() - egui::vec2(0.0, 40.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.add_enabled_ui(!r.running, |ui| {
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("reword-font")
                                .selected_text(if r.rewrite.family == Family::Keep {
                                    Family::Keep.name().to_string()
                                } else {
                                    r.rewrite.family.name().to_string()
                                })
                                .width(110.0)
                                .show_ui(ui, |ui| {
                                    for family in Family::ALL {
                                        ui.selectable_value(&mut r.rewrite.family, *family, family.name());
                                    }
                                });
                            ui.add(
                                egui::DragValue::new(&mut r.rewrite.size)
                                    .range(2.0..=144.0)
                                    .speed(0.1)
                                    .suffix(" pt"),
                            );
                            ui.toggle_value(&mut r.rewrite.bold, egui::RichText::new("B").strong())
                                .on_hover_text("Bold. Sets it in a standard font.");
                            ui.toggle_value(&mut r.rewrite.italic, egui::RichText::new("I").italics())
                                .on_hover_text("Italic. Sets it in a standard font.");
                            let mut rgb = r.rewrite.colour;
                            if egui::widgets::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
                                r.rewrite.colour = rgb;
                            }
                            for align in Align::ALL {
                                ui.selectable_value(&mut r.rewrite.align, *align, align.name());
                            }
                            ui.separator();
                            if ui.button("Done").clicked() {
                                done = true;
                            }
                            if ui.button("Cancel").clicked() {
                                cancel = true;
                            }
                            if r.running {
                                ui.spinner();
                            }
                        });
                    });
                    // Bold or italic in a font that isn't a standard one means
                    // a standard one.
                    if (r.rewrite.bold != r.found.bold || r.rewrite.italic != r.found.italic)
                        && r.rewrite.family == Family::Keep
                    {
                        r.rewrite.family = Family::nearest(&r.found.font);
                    }
                    if let Some(why) = &r.error {
                        ui.colored_label(egui::Color32::from_rgb(235, 120, 120), why);
                    }
                });
            });

        // The words, where they are, over a sheet of paper that hides the old.
        egui::Area::new(egui::Id::new(("reword-words", r.page)))
            .order(egui::Order::Foreground)
            .fixed_pos(area.left_top())
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::WHITE)
                    .stroke(egui::Stroke::new(1.5_f32, theme.accent)) // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                    .inner_margin(egui::Margin::same(2))
                    .show(ui, |ui| {
                        let typed = ui.add_enabled(
                            !r.running,
                            egui::TextEdit::multiline(&mut r.rewrite.text)
                                .frame(false)
                                .font(egui::FontId::proportional(size))
                                .text_color(ink)
                                .desired_width(width)
                                .desired_rows(r.found.lines.len().max(1)),
                        );
                        if !r.running {
                            typed.request_focus();
                        }
                    });
            });

        if cancel && !r.running {
            self.status = "Nothing was changed.".into();
            ctx.request_repaint();
            return;
        }
        if done && !r.running {
            if r.rewrite.text.trim().is_empty() {
                r.error = Some("Type the words, or Cancel to leave them as they were.".into());
            } else if r.rewrite == Rewrite::of(&r.found) {
                self.status = "Nothing was changed.".into();
                ctx.request_repaint();
                return;
            } else {
                // Markups not yet written into the file go in first, so the
                // words are changed in the file as it really is.
                if self.doc().is_some_and(|d| d.dirty) {
                    self.save_now();
                }
                let Some((id, path)) = self.doc().map(|d| (d.id, d.path.clone())) else { return };
                self.reword_task += 1;
                r.job = self.reword_task;
                r.running = true;
                r.error = None;
                r.on_disk = crate::sheet::stamp(&path);
                self.svc.send(crate::render::ToWorker::Rewrite {
                    doc: id,
                    job: r.job,
                    page: r.page,
                    found: Box::new(r.found.clone()),
                    rewrite: Box::new(r.rewrite.clone()),
                });
                self.status = "Writing the words into the drawing…".into();
            }
        }
        self.rewording = Some(r);
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// The worker's answer: the file with the words changed in it, or why
    /// they weren't.
    pub fn rewritten(&mut self, id: u64, job: u64, page: u32, result: Result<(Vec<u8>, String), String>) {
        let Some(mut r) = self.rewording.take() else { return };
        if r.job != job {
            self.rewording = Some(r);
            return;
        }
        r.running = false;
        let (bytes, said) = match result {
            Ok(done) => done,
            Err(why) => {
                r.error = Some(why);
                self.rewording = Some(r);
                return;
            }
        };
        let Some(doc) = self.docs.iter_mut().find(|d| d.id == id) else { return };
        if crate::sheet::stamp(&doc.path) != r.on_disk {
            r.error = Some(
                "The drawing was saved from somewhere else while the words were being written, \
                 so they weren't. Click Done again."
                    .into(),
            );
            self.rewording = Some(r);
            return;
        }
        let fresh = doc.read_again(bytes.clone());
        let page_ref = doc.file.pages().get(page as usize).copied();
        let sides = page_ref.and_then(|p| crate::sheet::Drawing::either_side(&doc.file, &fresh, p));
        if let Err(why) = crate::sheet::write_into_place(&doc.path, &bytes) {
            r.error = Some(why);
            self.rewording = Some(r);
            return;
        }
        doc.file = fresh;
        doc.on_disk = crate::sheet::stamp(&doc.path);
        if let (Some(page_ref), Some((before, after))) = (page_ref, sides) {
            doc.undo.push(crate::sheet::Step {
                what: "Change the words".into(),
                marks: doc.marks.clone(),
                file: Some(crate::sheet::PageSwap { page: page_ref, before, after }),
                sheets: None,
            });
            if doc.undo.len() > crate::sheet::REMEMBERED_STEPS {
                doc.undo.remove(0);
            }
            doc.redo.clear();
        }
        doc.redraw.insert(page);
        doc.paragraphs.remove(&page);
        doc.paragraphs_asked.remove(&page);
        doc.geometry.remove(&page);
        doc.geometry_asked.remove(&page);
        self.text_under = None;
        self.status = format!("{said} Undo puts them back.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(object: usize, text: &str, x: f64, y: f64, size: f64) -> Run {
        Run {
            object,
            text: text.into(),
            origin: [x, y],
            along: [1.0, 0.0],
            size,
            advance: text.chars().count() as f64 * size * 0.5,
            colour: [0, 0, 0],
            font: "Helvetica".into(),
            bold: false,
            italic: false,
        }
    }

    #[test]
    fn a_numbered_note_in_three_lines_is_one_paragraph() {
        let runs = vec![
            run(0, "1. ALL STRUCTURAL STEEL SHALL", 100.0, 700.0, 10.0),
            run(1, "CONFORM TO ASTM A992 UNLESS", 100.0, 688.0, 10.0),
            run(2, "NOTED OTHERWISE.", 100.0, 676.0, 10.0),
            // The next note, after a bigger gap.
            run(3, "2. BOLTS SHALL BE A325-N.", 100.0, 650.0, 10.0),
            // A title, somewhere else and bigger.
            run(4, "GENERAL NOTES", 400.0, 740.0, 18.0),
        ];
        let found = paragraphs(&runs);
        assert_eq!(found.len(), 3, "{found:#?}");
        let note = found.iter().find(|f| f.objects == vec![0, 1, 2]).expect("the first note");
        assert_eq!(note.text(), "1. ALL STRUCTURAL STEEL SHALL CONFORM TO ASTM A992 UNLESS NOTED OTHERWISE.");
        assert!((note.spacing - 12.0).abs() < 1e-9);
        assert_eq!(note.align, Align::Left);
        assert!(found.iter().any(|f| f.objects == vec![3]));
        assert!(found.iter().any(|f| f.lines == vec!["GENERAL NOTES".to_string()]));
    }

    #[test]
    fn the_next_numbered_note_is_its_own_paragraph_even_close_up() {
        let runs = vec![
            run(0, "1. ALL STRUCTURAL STEEL SHALL CONFORM", 100.0, 700.0, 10.0),
            run(1, "TO ASTM A572 GRADE 50.", 100.0, 688.0, 10.0),
            run(2, "2. BOLTS SHALL BE A325-N.", 100.0, 676.0, 10.0),
            run(3, "3. SEE DETAIL 4 FOR", 100.0, 664.0, 10.0),
            run(4, "2. PLACES.", 100.0, 652.0, 10.0),
        ];
        let found = paragraphs(&runs);
        let groups: Vec<Vec<usize>> = found.iter().map(|f| f.objects.clone()).collect();
        assert!(groups.contains(&vec![0, 1]), "{groups:?}");
        assert!(groups.contains(&vec![2]), "{groups:?}");
        // A line that only looks like a number, after one that runs on.
        assert!(groups.contains(&vec![3, 4]), "{groups:?}");
        assert!(starts_an_item("(c) GROUT"));
        assert!(starts_an_item("B. ANCHOR RODS"));
        assert!(!starts_an_item("W12X26 BEAM"));
        assert!(!starts_an_item("3/4\" DIA."));
    }

    #[test]
    fn only_fonts_put_in_whole_are_trusted_with_new_letters() {
        let bodies = [
            "<</Type/Catalog/Pages 2 0 R>>",
            "<</Type/Pages/Kids[3 0 R]/Count 1/Resources<</Font<</F1 4 0 R/F2 5 0 R/F3 6 0 R>>>>>>",
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>",
            "<</Type/Font/Subtype/TrueType/BaseFont/Arial>>",
            "<</Type/Font/Subtype/TrueType/BaseFont/ABCDEF+Calibri>>",
            "<</Type/Font/Subtype/Type0/BaseFont/Romans-Identity-H/DescendantFonts[7 0 R]>>",
            "<</Type/Font/Subtype/CIDFontType2/BaseFont/GHIJKL+Romans>>",
        ];
        let mut file = b"%PDF-1.5\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in bodies.iter().enumerate() {
            offsets.push(file.len());
            file.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let table = file.len();
        file.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
        for offset in &offsets {
            file.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        file.extend_from_slice(
            format!("trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{table}\n%%EOF\n", bodies.len() + 1).as_bytes(),
        );
        let whole = whole_fonts(&pdf::Document::from_bytes(file), 0);
        assert_eq!(whole.into_iter().collect::<Vec<_>>(), vec!["Arial".to_string()]);
    }

    #[test]
    fn letters_set_one_at_a_time_come_back_as_words() {
        // CAD programs often write every letter as its own piece.
        let mut runs = Vec::new();
        let mut x = 100.0;
        for (i, c) in "W12X26".chars().enumerate() {
            runs.push(run(i, &c.to_string(), x, 500.0, 10.0));
            x += 5.0;
        }
        x += 5.0; // a word space
        for (i, c) in "BEAM".chars().enumerate() {
            runs.push(run(10 + i, &c.to_string(), x, 500.0, 10.0));
            x += 5.0;
        }
        let found = paragraphs(&runs);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text(), "W12X26 BEAM");
    }

    #[test]
    fn two_columns_on_one_baseline_stay_apart() {
        let runs = vec![run(0, "DRAWN BY", 100.0, 100.0, 8.0), run(1, "CHECKED BY", 300.0, 100.0, 8.0)];
        assert_eq!(paragraphs(&runs).len(), 2);
    }

    #[test]
    fn centred_lines_are_read_as_centred() {
        let runs = vec![
            run(0, "SECOND FLOOR", 200.0, 300.0, 12.0),
            run(1, "FRAMING PLAN AND DETAILS", 170.0, 285.0, 12.0),
        ];
        let found = paragraphs(&runs);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].align, Align::Centre);
    }

    #[test]
    fn text_running_up_the_sheet_is_read_along_its_own_direction() {
        // A note turned a quarter turn: it runs up the page, and its lines
        // stack to the right.
        let mut a = run(0, "VERIFY IN FIELD", 100.0, 100.0, 10.0);
        let mut b = run(1, "BEFORE FABRICATION", 112.0, 100.0, 10.0);
        a.along = [0.0, 1.0];
        b.along = [0.0, 1.0];
        let found = paragraphs(&[a, b]);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].text(), "VERIFY IN FIELD BEFORE FABRICATION");
        // The first line's start is where it was.
        let start = found[0].to_page(found[0].left, found[0].top);
        assert!((start[0] - 100.0).abs() < 1e-9 && (start[1] - 100.0).abs() < 1e-9, "{start:?}");
        assert!(found[0].holds([105.0, 120.0]));
        assert!(!found[0].holds([300.0, 300.0]));
    }

    #[test]
    fn a_word_broken_over_two_lines_comes_back_whole() {
        let runs = vec![run(0, "ANCHOR RODS SHALL BE GAL-", 100.0, 200.0, 10.0), run(1, "vanized", 100.0, 188.0, 10.0)];
        assert_eq!(paragraphs(&runs)[0].text(), "ANCHOR RODS SHALL BE GALvanized");
    }

    #[test]
    fn words_wrap_to_the_width_and_keep_typed_breaks() {
        let measure = |s: &str| s.chars().count() as f64;
        assert_eq!(wrap("one two three four", 9.0, &measure), vec!["one two", "three", "four"]);
        assert_eq!(wrap("a b\nc", 20.0, &measure), vec!["a b", "c"]);
        assert_eq!(wrap("enormousword x", 4.0, &measure), vec!["enormousword", "x"]);
        assert_eq!(wrap("done\n", 20.0, &measure), vec!["done"]);
    }

    #[test]
    fn new_lines_start_where_the_old_ones_did() {
        let found = paragraphs(&[run(0, "ONE", 100.0, 500.0, 10.0), run(1, "TWO", 100.0, 488.0, 10.0)]).remove(0);
        let at = origins(&found, &[20.0, 30.0, 25.0], Align::Left, found.width, found.spacing);
        assert_eq!(at, vec![[100.0, 500.0], [100.0, 488.0], [100.0, 476.0]]);
        let right = origins(&found, &[10.0], Align::Right, 40.0, 12.0);
        assert_eq!(right, vec![[130.0, 500.0]]);
    }

    #[test]
    fn a_cut_down_font_can_only_set_what_it_had() {
        assert!(is_subset("ABCDEF+ArialMT"));
        assert!(!is_subset("ArialMT"));
        let had: std::collections::HashSet<char> = "BEAM 12".chars().collect();
        assert!(can_set("BEAM 21", &had));
        assert!(!can_set("BEAMS", &had));
    }

    #[test]
    fn a_file_font_is_matched_to_the_standard_family_nearest_it() {
        assert_eq!(Family::nearest("ABCDEF+TimesNewRomanPSMT"), Family::Serif);
        assert_eq!(Family::nearest("CourierNewPS-BoldMT"), Family::Mono);
        assert_eq!(Family::nearest("ArialMT"), Family::Sans);
        assert_eq!(Family::Serif.standard(true, false), "Times-Bold");
    }
}
