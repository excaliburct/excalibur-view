//! What the next markup will look like, and changing what is already drawn.
//!
//! Revu's properties toolbar: the colour, the fill, the weight, the line
//! style, the arrowheads, the hatch, the opacity, and how the words are set.
//! Every one of them does the same two things — it sets what the next markup
//! drawn will look like, and if something is selected, it changes that too.
//! That is the behaviour people have in their hands, and getting it the other
//! way round is the difference between a toolbar that works and one that has
//! to be thought about.

use crate::app::App;

/// The look the next markup will be drawn with.
#[derive(Clone, PartialEq, Debug)]
pub struct Pen {
    pub line: [u8; 4],
    /// `None` is no fill at all, which is not the same as white.
    pub fill: Option<[u8; 4]>,
    pub width: f64,
    pub style: Dashes,
    pub start: End,
    pub end: End,
    pub hatch: Hatch,
    /// Nought to one.
    pub opacity: f32,
    pub fill_opacity: f32,
    /// Blended by multiplying, so the drawing shows through.
    pub blend: bool,
    pub text: annot::text::Setting,
}

impl Default for Pen {
    fn default() -> Pen {
        Pen {
            line: [220, 40, 40, 255],
            fill: None,
            width: 2.0,
            style: Dashes::Solid,
            start: End::None,
            end: End::None,
            hatch: Hatch::Solid,
            opacity: 1.0,
            fill_opacity: 1.0,
            blend: false,
            text: annot::text::Setting::default(),
        }
    }
}

/// How a line is broken up.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Dashes {
    #[default]
    Solid,
    Dashed,
    Dotted,
    DashDot,
    LongDash,
}

impl Dashes {
    pub const ALL: &'static [Dashes] = &[
        Dashes::Solid,
        Dashes::Dashed,
        Dashes::Dotted,
        Dashes::DashDot,
        Dashes::LongDash,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Dashes::Solid => "Solid",
            Dashes::Dashed => "Dashed",
            Dashes::Dotted => "Dotted",
            Dashes::DashDot => "Dash-dot",
            Dashes::LongDash => "Long dash",
        }
    }

    /// The pattern, in points. Scaled by the line's weight so a dashed line
    /// two points thick does not read as a solid one.
    pub fn pattern(self, width: f64) -> Vec<f64> {
        let w = width.max(0.5);
        match self {
            Dashes::Solid => Vec::new(),
            Dashes::Dashed => vec![w * 3.0, w * 2.0],
            Dashes::Dotted => vec![w * 0.1, w * 2.0],
            Dashes::DashDot => vec![w * 4.0, w * 2.0, w * 0.1, w * 2.0],
            Dashes::LongDash => vec![w * 8.0, w * 3.0],
        }
    }

    /// Which of ours a pattern from another program is.
    ///
    /// Read from the shape of the pattern rather than by matching numbers:
    /// the same dashed line written by two programs is two different pairs of
    /// numbers, and a file whose line is plainly dashed must not come back as
    /// "something else" and leave the toolbar showing nothing.
    pub fn from_pattern(pattern: &[f64], _width: f64) -> Dashes {
        let on = pattern.first().copied().unwrap_or(0.0);
        let off = pattern.get(1).copied().unwrap_or(on);
        if pattern.is_empty() || (on <= 0.0 && off <= 0.0) {
            return Dashes::Solid;
        }
        // Four numbers is a dash and a dot: long, gap, short, gap.
        if pattern.len() >= 4 {
            return Dashes::DashDot;
        }
        // A dot is a mark with no length to speak of.
        if on <= 1.5 || (off > 0.0 && on / off <= 0.35) {
            return Dashes::Dotted;
        }
        if off > 0.0 && on / off >= 2.0 {
            return Dashes::LongDash;
        }
        Dashes::Dashed
    }
}

/// What sits on the end of a line.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum End {
    #[default]
    None,
    Arrow,
    OpenArrow,
    Circle,
    Square,
    Diamond,
    Butt,
    Slash,
}

impl End {
    pub const ALL: &'static [End] = &[
        End::None,
        End::Arrow,
        End::OpenArrow,
        End::Circle,
        End::Square,
        End::Diamond,
        End::Butt,
        End::Slash,
    ];

    pub fn name(self) -> &'static str {
        match self {
            End::None => "None",
            End::Arrow => "Closed arrow",
            End::OpenArrow => "Open arrow",
            End::Circle => "Circle",
            End::Square => "Square",
            End::Diamond => "Diamond",
            End::Butt => "Butt",
            End::Slash => "Slash",
        }
    }

    /// What a PDF calls it.
    pub fn as_pdf(self) -> &'static str {
        match self {
            End::None => "None",
            End::Arrow => "ClosedArrow",
            End::OpenArrow => "OpenArrow",
            End::Circle => "Circle",
            End::Square => "Square",
            End::Diamond => "Diamond",
            End::Butt => "Butt",
            End::Slash => "Slash",
        }
    }

    pub fn from_pdf(name: &str) -> End {
        match name {
            "ClosedArrow" => End::Arrow,
            "OpenArrow" => End::OpenArrow,
            "Circle" => End::Circle,
            "Square" => End::Square,
            "Diamond" => End::Diamond,
            "Butt" => End::Butt,
            "Slash" => End::Slash,
            _ => End::None,
        }
    }
}

/// How a shape is filled.
///
/// A hatch is what tells one area takeoff from another on a plan with six of
/// them over each other, which is why it matters more here than it looks.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Hatch {
    #[default]
    Solid,
    None,
    Diagonal,
    BackDiagonal,
    Cross,
    Horizontal,
    Vertical,
    Dots,
}

impl Hatch {
    pub const ALL: &'static [Hatch] = &[
        Hatch::Solid,
        Hatch::None,
        Hatch::Diagonal,
        Hatch::BackDiagonal,
        Hatch::Cross,
        Hatch::Horizontal,
        Hatch::Vertical,
        Hatch::Dots,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Hatch::Solid => "Solid",
            Hatch::None => "None",
            Hatch::Diagonal => "Diagonal",
            Hatch::BackDiagonal => "Back diagonal",
            Hatch::Cross => "Cross",
            Hatch::Horizontal => "Horizontal",
            Hatch::Vertical => "Vertical",
            Hatch::Dots => "Dots",
        }
    }

    /// The name Revu writes it under, so a tool chest tool keeps its hatch.
    pub fn as_pdf(self) -> &'static str {
        match self {
            Hatch::Solid => "Solid",
            Hatch::None => "None",
            Hatch::Diagonal => "Diagonal",
            Hatch::BackDiagonal => "BackDiagonal",
            Hatch::Cross => "Cross",
            Hatch::Horizontal => "Horizontal",
            Hatch::Vertical => "Vertical",
            Hatch::Dots => "Dots",
        }
    }

    pub fn from_pdf(name: &str) -> Hatch {
        Hatch::ALL
            .iter()
            .copied()
            .find(|h| h.as_pdf() == name)
            .unwrap_or(Hatch::Solid)
    }
}

/// The weights on the toolbar. Revu's list, because a line weight somebody
/// has in their hand should be where their hand goes.
pub const WEIGHTS: &[f64] = &[0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0];

/// The sizes on the font list.
pub const SIZES: &[f64] = &[6.0, 8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 18.0, 24.0, 36.0, 48.0, 72.0];

impl Pen {
    /// Reads the look off a markup, so picking one up puts its look on the
    /// toolbar.
    pub fn of(markup: &annot::Markup) -> Pen {
        let colour = markup.colour();
        let to_byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let width = markup.width();
        let pattern = markup
            .dict
            .get("BS")
            .and_then(|o| o.as_dict())
            .and_then(|d| d.get("D"))
            .map(|o| o.numbers())
            .unwrap_or_default();
        let dashed = markup
            .dict
            .get("BS")
            .and_then(|o| o.as_dict())
            .and_then(|d| d.get("S"))
            .and_then(|o| o.as_name())
            .map(|n| n.as_str() == "D")
            .unwrap_or(false);
        let ends: Vec<String> = markup
            .dict
            .get("LE")
            .and_then(|o| o.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|o| o.as_name())
                    .map(|n| n.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default();

        Pen {
            line: [to_byte(colour[0]), to_byte(colour[1]), to_byte(colour[2]), 255],
            fill: markup
                .interior()
                .map(|c| [to_byte(c[0]), to_byte(c[1]), to_byte(c[2]), 255]),
            width,
            style: if dashed {
                Dashes::from_pattern(&pattern, width)
            } else {
                Dashes::Solid
            },
            start: ends.first().map(|n| End::from_pdf(n)).unwrap_or_default(),
            end: ends.get(1).map(|n| End::from_pdf(n)).unwrap_or_default(),
            hatch: markup
                .dict
                .get("BSIFillType")
                .and_then(|o| o.as_name())
                .map(|n| Hatch::from_pdf(n.as_str()))
                .unwrap_or(Hatch::Solid),
            opacity: markup.opacity(),
            fill_opacity: markup.fill_opacity(),
            blend: markup.multiplies(),
            text: annot::text::Setting::of(markup),
        }
    }

    /// Puts this look onto a markup, leaving its shape and its measurement
    /// alone.
    pub fn onto(&self, markup: &mut annot::Markup) {
        let to_f = |v: u8| v as f32 / 255.0;
        markup.set_colour([to_f(self.line[0]), to_f(self.line[1]), to_f(self.line[2])]);
        match self.fill {
            Some(fill) => {
                markup.dict.set(
                    "IC",
                    pdf::Object::Array(
                        [fill[0], fill[1], fill[2]]
                            .iter()
                            .map(|v| pdf::Object::real(to_f(*v) as f64))
                            .collect(),
                    ),
                );
            }
            None => {
                markup.dict.remove("IC");
            }
        }
        markup.set_width(self.width);

        let mut border = pdf::Dict::new();
        border.set("W", pdf::Object::real(self.width));
        match self.style {
            Dashes::Solid => {
                border.set("S", pdf::Object::name("S"));
            }
            other => {
                border.set("S", pdf::Object::name("D"));
                border.set(
                    "D",
                    pdf::Object::Array(
                        other
                            .pattern(self.width)
                            .iter()
                            .map(|v| pdf::Object::real(*v))
                            .collect(),
                    ),
                );
            }
        }
        markup.dict.set("BS", pdf::Object::Dict(border));

        if self.start == End::None && self.end == End::None {
            markup.dict.remove("LE");
        } else {
            markup.dict.set(
                "LE",
                pdf::Object::Array(vec![
                    pdf::Object::name(self.start.as_pdf()),
                    pdf::Object::name(self.end.as_pdf()),
                ]),
            );
        }

        markup
            .dict
            .set("BSIFillType", pdf::Object::name(self.hatch.as_pdf()));
        markup
            .dict
            .set("CA", pdf::Object::real(self.opacity as f64));
        markup
            .dict
            .set("FillOpacity", pdf::Object::real(self.fill_opacity as f64));
        markup.set_multiply(self.blend);
        self.text.onto(markup);
        // The appearance was drawn for the old look — unless it is a picture
        // that can only be kept, which a colour doesn't change.
        if !markup.keeps_its_appearance() {
            markup.dict.remove("AP");
        }
    }
}

/// Which chooser is open on the properties toolbar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choosing {
    LineColour,
    FillColour,
    TextColour,
    Opacity,
    Hatch,
    Width,
    Style,
    Start,
    End,
    Font,
    FontSize,
}

impl Choosing {
    pub fn from_command(id: &str) -> Option<Choosing> {
        Some(match id {
            "DropDown.ColorLine" => Choosing::LineColour,
            "DropDown.ColorFill" => Choosing::FillColour,
            "DropDown.ColorText" => Choosing::TextColour,
            "DropDown.Opacity" => Choosing::Opacity,
            "DropDown.HatchPattern" => Choosing::Hatch,
            "DropDown.LineWidth" => Choosing::Width,
            "DropDown.LineStyle" => Choosing::Style,
            "DropDown.LineStart" => Choosing::Start,
            "DropDown.LineEnd" => Choosing::End,
            "Combo.Font" => Choosing::Font,
            "Combo.FontSize" => Choosing::FontSize,
            _ => return None,
        })
    }

    pub fn title(self) -> &'static str {
        match self {
            Choosing::LineColour => "Line colour",
            Choosing::FillColour => "Fill",
            Choosing::TextColour => "Text colour",
            Choosing::Opacity => "How see-through",
            Choosing::Hatch => "Hatch",
            Choosing::Width => "Line weight",
            Choosing::Style => "Line style",
            Choosing::Start => "Start of the line",
            Choosing::End => "End of the line",
            Choosing::Font => "Font",
            Choosing::FontSize => "Size",
        }
    }
}

impl App {
    /// Changes the pen, and everything selected with it.
    ///
    /// Both, always, and in that order: somebody who picks a colour with three
    /// beams selected means those three beams *and* the next one they draw.
    pub fn change_pen(&mut self, change: impl Fn(&mut Pen)) {
        change(&mut self.pen);
        // The colour on the old toolbar and the pen are the same thing.
        self.color = self.pen.line;
        let Some(doc) = self.doc_mut() else { return };
        let picked = doc.selection();
        if picked.is_empty() {
            return;
        }
        doc.checkpoint_named(&format!(
            "Change {} markup{}",
            picked.len(),
            if picked.len() == 1 { "" } else { "s" }
        ));
        // Only what was chosen changes on each of them. Six boxes in three
        // colours given a fill keep their three colours.
        for index in &picked {
            if let Some(mark) = doc.marks.get_mut(*index) {
                let mut look = Pen::of(&mark.markup);
                change(&mut look);
                look.onto(&mut mark.markup);
                mark.changed = true;
            }
        }
        doc.dirty = true;
        self.status = format!(
            "{} markup{} changed.",
            picked.len(),
            if picked.len() == 1 { "" } else { "s" }
        );
    }

    /// Puts the selected markup's look on the toolbar, so the next one drawn
    /// matches what was just picked up.
    pub fn take_pen_from_selection(&mut self) {
        let Some(pen) = self
            .doc()
            .and_then(|d| d.selected.and_then(|i| d.marks.get(i)))
            .map(|m| Pen::of(&m.markup))
        else {
            return;
        };
        self.pen = pen;
        self.color = self.pen.line;
    }
}

/// Colours offered on a markup's right-click menu: the toolbar's own.
fn menu_colours() -> impl Iterator<Item = (&'static str, [u8; 4])> {
    crate::app::PALETTE.iter().chain(MORE_COLOURS.iter()).map(|(n, c)| (*n, *c))
}

/// A row of colour swatches in a menu. Comes back with the one clicked.
fn swatches(ui: &mut egui::Ui, line: egui::Color32) -> Option<[u8; 4]> {
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.set_max_width(180.0);
        for (name, rgb) in menu_colours() {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
            ui.painter().rect_filled(rect.shrink(2.0), 3.0, egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
            ui.painter().rect_stroke(rect.shrink(2.0), 3.0, egui::Stroke::new(1.0_f32, line), egui::StrokeKind::Inside); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
            if response.on_hover_text(name).clicked() {
                picked = Some(rgb);
            }
        }
    });
    picked
}

impl App {
    /// The right-click menu on a markup on the sheet: its fill, its outline,
    /// blending, how see-through it is, its line weight, its words.
    pub fn markup_menu(&mut self, response: &egui::Response) {
        let theme = self.chrome.theme;
        let Some(doc) = self.doc() else { return };
        let picked = doc.selection();
        // One markup, or one group of them such as a Cloud+, counts as one.
        let single = crate::groups::one_thing(&doc.marks, &picked);
        let one = single.then(|| picked.first().and_then(|i| doc.marks.get(*i))).flatten();
        let look = one.map(|m| Pen::of(&m.markup)).unwrap_or_else(|| self.pen.clone());
        let words_at = if single {
            picked
                .iter()
                .copied()
                .find(|i| doc.marks.get(*i).is_some_and(|m| m.markup.subtype() == annot::Subtype::FreeText))
        } else {
            None
        };
        let words = words_at.is_some();
        let count = if single { 1 } else { picked.len() };
        let mut chosen: Option<Box<dyn Fn(&mut Pen)>> = None;
        let mut edit_words = false;
        let mut delete = false;
        response.context_menu(|ui| {
            if count == 0 {
                ui.close();
                return;
            }
            ui.set_min_width(220.0);
            ui.label(
                egui::RichText::new(if count == 1 {
                    "This markup".to_string()
                } else {
                    format!("These {count} markups")
                })
                .color(theme.faint)
                .size(11.0),
            );
            ui.menu_button("Fill", |ui| {
                if ui.button("No fill").clicked() {
                    chosen = Some(Box::new(|p: &mut Pen| p.fill = None));
                    ui.close();
                }
                if let Some(rgb) = swatches(ui, theme.line) {
                    chosen = Some(Box::new(move |p: &mut Pen| p.fill = Some(rgb)));
                    ui.close();
                }
                ui.horizontal(|ui| {
                    let now = look.fill.unwrap_or([255, 255, 255, 255]);
                    let mut rgb = [now[0], now[1], now[2]];
                    if egui::widgets::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
                        chosen = Some(Box::new(move |p: &mut Pen| p.fill = Some([rgb[0], rgb[1], rgb[2], 255])));
                    }
                    ui.label(egui::RichText::new("Any other").color(theme.faint).size(11.0));
                });
            });
            ui.menu_button("Outline", |ui| {
                if ui.button("No outline").clicked() {
                    chosen = Some(Box::new(|p: &mut Pen| p.width = 0.0));
                    ui.close();
                }
                if let Some(rgb) = swatches(ui, theme.line) {
                    chosen = Some(Box::new(move |p: &mut Pen| {
                        p.line = rgb;
                        if p.width <= 0.0 {
                            p.width = 1.0;
                        }
                    }));
                    ui.close();
                }
                ui.horizontal(|ui| {
                    let mut rgb = [look.line[0], look.line[1], look.line[2]];
                    if egui::widgets::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
                        chosen = Some(Box::new(move |p: &mut Pen| p.line = [rgb[0], rgb[1], rgb[2], 255]));
                    }
                    ui.label(egui::RichText::new("Any other").color(theme.faint).size(11.0));
                });
            });
            ui.menu_button("Line weight", |ui| {
                for weight in WEIGHTS {
                    if ui.selectable_label((look.width - weight).abs() < 0.01, format!("{weight} pt")).clicked() {
                        let at = *weight;
                        chosen = Some(Box::new(move |p: &mut Pen| p.width = at));
                        ui.close();
                    }
                }
            });
            ui.menu_button("See-through", |ui| {
                for (label, share) in [("Solid", 1.0f32), ("Three quarters", 0.75), ("Half", 0.5), ("A third", 0.35), ("A quarter", 0.25)] {
                    if ui.selectable_label((look.fill_opacity - share).abs() < 0.01, label).clicked() {
                        chosen = Some(Box::new(move |p: &mut Pen| p.fill_opacity = share));
                        ui.close();
                    }
                }
            });
            let mut blend = look.blend;
            if ui
                .checkbox(&mut blend, "Multiply")
                .on_hover_text("The drawing shows through the fill, the way it does under a highlighter.")
                .changed()
            {
                chosen = Some(Box::new(move |p: &mut Pen| p.blend = blend));
                ui.close();
            }
            if words {
                ui.separator();
                if ui.button("Edit the words").clicked() {
                    edit_words = true;
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Delete").clicked() {
                delete = true;
                ui.close();
            }
        });
        if let Some(change) = chosen {
            self.change_pen(change);
            self.save_soon();
        }
        if edit_words {
            if let Some(i) = words_at {
                if let Some(doc) = self.doc_mut() {
                    doc.checkpoint_named("Edit text");
                }
                self.editing_text = Some(i);
            }
        }
        if delete {
            self.delete_selected();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_written_onto_a_markup_reads_back_the_same() {
        let pen = Pen {
            line: [10, 20, 30, 255],
            fill: Some([200, 210, 220, 255]),
            width: 3.0,
            style: Dashes::DashDot,
            start: End::Circle,
            end: End::Arrow,
            hatch: Hatch::Cross,
            opacity: 0.5,
            fill_opacity: 0.25,
            blend: true,
            text: annot::text::Setting::default(),
        };
        let mut markup = annot::Markup::new(annot::Subtype::Line);
        markup.set_line([0.0, 0.0], [100.0, 0.0]);
        pen.onto(&mut markup);

        let back = Pen::of(&markup);
        assert_eq!(back.width, 3.0);
        assert_eq!(back.style, Dashes::DashDot);
        assert_eq!(back.start, End::Circle);
        assert_eq!(back.end, End::Arrow);
        assert_eq!(back.hatch, Hatch::Cross);
        assert!((back.opacity - 0.5).abs() < 0.01);
        assert!(back.fill.is_some());
    }

    #[test]
    fn changing_one_thing_leaves_the_rest_of_a_markups_look_alone() {
        // What the right-click menu and the toolbar do to each selected
        // markup: read its own look, change the one thing, put it back.
        let mut red = annot::Markup::new(annot::Subtype::Square);
        red.set_box([0.0, 0.0, 10.0, 10.0]);
        Pen { line: [220, 40, 40, 255], width: 3.0, ..Pen::default() }.onto(&mut red);
        let mut look = Pen::of(&red);
        look.fill = Some([255, 255, 0, 255]);
        look.blend = true;
        look.onto(&mut red);
        let after = Pen::of(&red);
        assert_eq!(after.line, [220, 40, 40, 255], "the outline is still red");
        assert_eq!(after.width, 3.0);
        assert_eq!(after.fill, Some([255, 255, 0, 255]));
        assert!(after.blend && red.multiplies());
    }

    #[test]
    fn no_fill_is_not_the_same_as_a_white_one() {
        // A shape with a white fill hides what is under it; one with no fill
        // does not. Confusing the two is how a takeoff blanks out a plan.
        let mut markup = annot::Markup::new(annot::Subtype::Polygon);
        markup.set_vertices(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
        let mut pen = Pen::default();
        pen.fill = Some([255, 255, 255, 255]);
        pen.onto(&mut markup);
        assert!(Pen::of(&markup).fill.is_some());

        pen.fill = None;
        pen.onto(&mut markup);
        assert!(Pen::of(&markup).fill.is_none());
        assert!(!markup.dict.has("IC"));
    }

    #[test]
    fn a_dashed_line_from_another_program_comes_back_as_one_of_ours() {
        // Rather than as "something else", which would leave the toolbar
        // showing nothing selected for a line that is plainly dashed.
        let mut markup = annot::Markup::new(annot::Subtype::Line);
        markup.set_line([0.0, 0.0], [100.0, 0.0]);
        markup.set_width(2.0);
        let mut border = pdf::Dict::new();
        border.set("S", pdf::Object::name("D"));
        border.set(
            "D",
            pdf::Object::Array(vec![pdf::Object::real(6.0), pdf::Object::real(4.0)]),
        );
        markup.dict.set("BS", pdf::Object::Dict(border));
        assert_eq!(Pen::of(&markup).style, Dashes::Dashed);
    }

    #[test]
    fn a_solid_line_says_solid() {
        let mut markup = annot::Markup::new(annot::Subtype::Line);
        markup.set_line([0.0, 0.0], [100.0, 0.0]);
        assert_eq!(Pen::of(&markup).style, Dashes::Solid);
    }

    #[test]
    fn a_dash_pattern_grows_with_the_line_it_is_on() {
        // A dashed line two points thick with a three point gap reads as a
        // solid line from across a plot room.
        let thin = Dashes::Dashed.pattern(0.5);
        let thick = Dashes::Dashed.pattern(6.0);
        assert!(thick[0] > thin[0] * 4.0);
        assert!(Dashes::Solid.pattern(2.0).is_empty());
    }

    #[test]
    fn every_line_ending_has_a_name_a_pdf_knows() {
        for end in End::ALL {
            assert_eq!(End::from_pdf(end.as_pdf()), *end);
        }
    }

    #[test]
    fn every_hatch_survives_the_trip_through_a_file() {
        for hatch in Hatch::ALL {
            assert_eq!(Hatch::from_pdf(hatch.as_pdf()), *hatch);
        }
    }
}

// ---- the choosers ----------------------------------------------------------

impl App {
    /// The little window a properties-toolbar button opens.
    ///
    /// One window rather than eleven, because they differ only in what is in
    /// them, and a chooser that closes when you click away is the behaviour
    /// people expect from all of them alike.
    pub fn pen_chooser(&mut self, ctx: &egui::Context) {
        let Some(which) = self.choosing else { return };
        let theme = self.chrome.theme;
        let mut keep = true;
        let mut chosen: Option<Box<dyn Fn(&mut Pen)>> = None;
        let selected = self
            .doc()
            .map(|d| d.selection().len())
            .unwrap_or(0);
        let pen = self.pen.clone();

        egui::Window::new(which.title())
            .open(&mut keep)
            .collapsible(false)
            .resizable(false)
            .default_width(260.0)
            .max_width(360.0)
            .show(ctx, |ui| {
                if selected > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "Changes {selected} selected markup{} and what you draw next.",
                            if selected == 1 { "" } else { "s" }
                        ))
                        .color(theme.faint)
                        .size(10.0),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("Changes what you draw next.")
                            .color(theme.faint)
                            .size(10.0),
                    );
                }
                ui.add_space(8.0);

                match which {
                    Choosing::LineColour | Choosing::FillColour | Choosing::TextColour => {
                        if which == Choosing::FillColour {
                            if ui.button("No fill").clicked() {
                                chosen = Some(Box::new(|pen| pen.fill = None));
                            }
                            ui.add_space(6.0);
                        }
                        let swatch = |ui: &mut egui::Ui, name: &str, rgb: [u8; 4]| {
                            let (rect, response) = ui.allocate_exact_size(
                                egui::vec2(28.0, 28.0),
                                egui::Sense::click(),
                            );
                            ui.painter().rect_filled(
                                rect.shrink(2.0),
                                3.0,
                                egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                            );
                            ui.painter().rect_stroke(
                                rect.shrink(2.0),
                                3.0,
                                egui::Stroke::new(1.0_f32, theme.line), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                                egui::StrokeKind::Inside,
                            );
                            response.on_hover_text(name).clicked()
                        };
                        let mut picked: Option<[u8; 4]> = None;
                        ui.horizontal_wrapped(|ui| {
                            for (name, rgb) in crate::app::PALETTE {
                                if swatch(ui, name, *rgb) {
                                    picked = Some(*rgb);
                                }
                            }
                            for (name, rgb) in MORE_COLOURS {
                                if swatch(ui, name, *rgb) {
                                    picked = Some(*rgb);
                                }
                            }
                        });
                        // Any colour at all, for a company's own or one to
                        // match a drawing.
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let now = match which {
                                Choosing::LineColour => pen.line,
                                Choosing::FillColour => pen.fill.unwrap_or([255, 255, 255, 255]),
                                _ => {
                                    let c = pen.text.colour;
                                    [(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, 255]
                                }
                            };
                            let mut rgb = [now[0], now[1], now[2]];
                            if egui::widgets::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
                                picked = Some([rgb[0], rgb[1], rgb[2], 255]);
                            }
                            ui.label(egui::RichText::new("Any other colour").color(theme.faint).size(11.0));
                        });
                        if which == Choosing::FillColour {
                            ui.add_space(6.0);
                            let mut blend = pen.blend;
                            if ui
                                .checkbox(&mut blend, "Multiply, so the drawing shows through")
                                .on_hover_text(
                                    "Blends the fill onto the sheet the way a highlighter does: \
                                     the lines under it stay dark and readable at full colour.",
                                )
                                .changed()
                            {
                                chosen = Some(Box::new(move |pen: &mut Pen| pen.blend = blend));
                            }
                        }
                        if let Some(rgb) = picked {
                            chosen = Some(match which {
                                Choosing::LineColour => Box::new(move |pen: &mut Pen| {
                                    pen.line = rgb;
                                }),
                                Choosing::FillColour => Box::new(move |pen: &mut Pen| {
                                    pen.fill = Some(rgb);
                                }),
                                _ => Box::new(move |pen: &mut Pen| {
                                    pen.text.colour = [
                                        rgb[0] as f32 / 255.0,
                                        rgb[1] as f32 / 255.0,
                                        rgb[2] as f32 / 255.0,
                                    ];
                                }),
                            });
                        }
                    }
                    Choosing::Opacity => {
                        let mut line = pen.opacity;
                        let mut fill = pen.fill_opacity;
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Everything").size(11.0));
                            if ui
                                .add(egui::Slider::new(&mut line, 0.1..=1.0).show_value(false))
                                .changed()
                            {
                                chosen = Some(Box::new(move |pen: &mut Pen| {
                                    pen.opacity = line;
                                }));
                            }
                            ui.label(
                                egui::RichText::new(format!("{:.0}%", line * 100.0))
                                    .color(theme.faint)
                                    .size(11.0),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("The fill").size(11.0));
                            if ui
                                .add(egui::Slider::new(&mut fill, 0.0..=1.0).show_value(false))
                                .changed()
                            {
                                chosen = Some(Box::new(move |pen: &mut Pen| {
                                    pen.fill_opacity = fill;
                                }));
                            }
                            ui.label(
                                egui::RichText::new(format!("{:.0}%", fill * 100.0))
                                    .color(theme.faint)
                                    .size(11.0),
                            );
                        });
                        let mut blend = pen.blend;
                        if ui
                            .checkbox(&mut blend, "Multiply, so the drawing shows through")
                            .changed()
                        {
                            chosen = Some(Box::new(move |pen: &mut Pen| pen.blend = blend));
                        }
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(
                                "A filled area takeoff at about a third lets the drawing \
                                 under it still be read.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                    }
                    Choosing::Width => {
                        for weight in WEIGHTS {
                            let row = ui.selectable_label(
                                (pen.width - weight).abs() < 0.01,
                                format!("{weight} pt"),
                            );
                            let at = *weight;
                            if row.clicked() {
                                chosen = Some(Box::new(move |pen: &mut Pen| pen.width = at));
                            }
                        }
                    }
                    Choosing::Style => {
                        for style in Dashes::ALL {
                            let row = ui.selectable_label(pen.style == *style, style.name());
                            let at = *style;
                            if row.clicked() {
                                chosen = Some(Box::new(move |pen: &mut Pen| pen.style = at));
                            }
                        }
                    }
                    Choosing::Start | Choosing::End => {
                        let now = if which == Choosing::Start { pen.start } else { pen.end };
                        for end in End::ALL {
                            let row = ui.selectable_label(now == *end, end.name());
                            let at = *end;
                            let start = which == Choosing::Start;
                            if row.clicked() {
                                chosen = Some(Box::new(move |pen: &mut Pen| {
                                    if start {
                                        pen.start = at;
                                    } else {
                                        pen.end = at;
                                    }
                                }));
                            }
                        }
                    }
                    Choosing::Hatch => {
                        for hatch in Hatch::ALL {
                            let row = ui.selectable_label(pen.hatch == *hatch, hatch.name());
                            let at = *hatch;
                            if row.clicked() {
                                chosen = Some(Box::new(move |pen: &mut Pen| pen.hatch = at));
                            }
                        }
                    }
                    Choosing::Font => {
                        for family in annot::text::Family::ALL {
                            let row =
                                ui.selectable_label(pen.text.family == *family, family.name());
                            let at = *family;
                            if row.clicked() {
                                chosen =
                                    Some(Box::new(move |pen: &mut Pen| pen.text.family = at));
                            }
                        }
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(
                                "Only fonts every reader already has. A markup naming a \
                                 font off this machine arrives set in whatever the other \
                                 machine had instead, and stops fitting its box.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                    }
                    Choosing::FontSize => {
                        ui.horizontal_wrapped(|ui| {
                            for size in SIZES {
                                let row = ui.selectable_label(
                                    (pen.text.size - size).abs() < 0.01,
                                    format!("{size}"),
                                );
                                let at = *size;
                                if row.clicked() {
                                    chosen =
                                        Some(Box::new(move |pen: &mut Pen| pen.text.size = at));
                                }
                            }
                        });
                    }
                }
            });

        if let Some(change) = chosen {
            self.change_pen(change);
        }
        if !keep {
            self.choosing = None;
        }
    }
}

/// The rest of a drawing office's colours, beyond the seven on the toolbar.
pub const MORE_COLOURS: &[(&str, [u8; 4])] = &[
    ("White", [255, 255, 255, 255]),
    ("Grey", [130, 130, 130, 255]),
    ("Brown", [140, 90, 40, 255]),
    ("Cyan", [0, 175, 200, 255]),
    ("Magenta", [210, 40, 160, 255]),
    ("Lime", [150, 205, 40, 255]),
    ("Navy", [25, 45, 120, 255]),
    ("Maroon", [125, 25, 45, 255]),
];

// ---- the one measurement that needs a number typed -------------------------

impl App {
    /// Asks how deep a volume is, right after the shape is drawn.
    ///
    /// A volume is an area with a depth. Without one there is no volume, and
    /// this program does not put a number on a sheet that nobody gave it — so
    /// a shape left without a depth is reported in the Markups list as having
    /// no volume rather than being counted as nothing.
    pub fn depth_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut asking) = self.asking_depth.take() else {
            return;
        };
        let theme = self.chrome.theme;
        let mut done = false;
        let mut leave_it = false;
        let units = self.units;
        let scale = self.doc().and_then(|d| d.scale());

        egui::Window::new("How deep?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .max_width(620.0)
            .show(ctx, |ui| {
                ui.set_min_width(380.0);
                match &scale {
                    Some(_) => {
                        ui.label(
                            egui::RichText::new(
                                "The depth of the pour, the thickness of the slab, the \
                                 height of the wall.",
                            )
                            .color(theme.faint)
                            .size(11.0),
                        );
                    }
                    None => {
                        ui.label(
                            egui::RichText::new(
                                "This sheet has no scale, so nothing on it can be \
                                 measured yet. Set the scale and the volume will read.",
                            )
                            .color(theme.warn)
                            .size(11.0),
                        );
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let box_ = ui.add(
                        egui::TextEdit::singleline(&mut asking.typed)
                            .hint_text(units.hint())
                            .desired_width(180.0),
                    );
                    box_.request_focus();
                    ui.label(
                        egui::RichText::new(units.name())
                            .color(theme.faint)
                            .size(11.0),
                    );
                });
                if let Some(why) = &asking.error {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(why).color(theme.warn).size(11.0));
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let ready = !asking.typed.trim().is_empty();
                    if ui.add_enabled(ready, egui::Button::new("That is the depth")).clicked()
                        || (ready && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        done = true;
                    }
                    if ui.button("Leave it without one").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        leave_it = true;
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Left without one, the shape stays on the sheet and the \
                         Markups list reports it as having no volume. It is never \
                         counted as nothing.",
                    )
                    .color(theme.faint)
                    .size(10.0),
                );
            });

        if leave_it {
            self.status = "That shape has no depth, so it has no volume. The Markups \
                           list says so."
                .into();
            return;
        }
        if done {
            match units.parse(&asking.typed) {
                Some(depth) if depth > 0.0 => {
                    let at = asking.markup;
                    let scale_points = scale.as_ref().map(|s| s.points_for(depth));
                    if let Some(doc) = self.doc_mut() {
                        if let Some(mark) = doc.marks.get_mut(at) {
                            // Kept in the sheet's own points, which is the space
                            // every other measurement on the markup is in.
                            let in_points = scale_points.unwrap_or(depth);
                            mark.markup
                                .set("Depth", pdf::Object::real(in_points));
                            mark.changed = true;
                        }
                        doc.remeasure_selection();
                        doc.dirty = true;
                    }
                    self.status = format!("Depth set. {}", asking.typed.trim());
                    return;
                }
                _ => {
                    asking.error = Some(format!(
                        "That is not a depth this program can read. {}",
                        units.hint()
                    ));
                }
            }
        }
        self.asking_depth = Some(asking);
    }
}
