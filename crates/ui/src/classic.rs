//! The Classic look: grey chrome, raised buttons, sunken white fields and a
//! navy band across the window in front — the look of the office machines a
//! lot of the trade learned computers on, and of Moo OS.
//!
//! egui draws a widget's frame as one rectangle in one colour, and a classic
//! bevel is four edges in four. So the Classic visuals give each kind of frame
//! its own marker colour, and at the end of every frame [`finish`] walks what
//! is about to be drawn and swaps each marked rectangle for the real thing: a
//! raised button, a sunken field, a window edge, a navy highlight. Anything
//! it does not recognise is left exactly as it was drawn.
//!
//! The markers are colours nothing else in the program uses, and each one is
//! close enough to what it stands for that a shape missed by [`finish`]
//! still looks nearly right: a near-black edge, a near-grey line.

use std::sync::Arc;

use egui::epaint::{ColorMode, RectShape, TextShape};
use egui::{Color32, CornerRadius, LayerId, Mesh, Order, Pos2, Rect, Shape, Stroke, Visuals};

/// The grey everything is made of.
pub const FACE: Color32 = Color32::from_rgb(212, 208, 200);
/// The lit edge of anything raised.
pub const HIGHLIGHT: Color32 = Color32::WHITE;
/// The shaded edge.
pub const SHADOW: Color32 = Color32::from_rgb(128, 128, 128);
/// The outer shaded edge, darker still.
pub const DARK: Color32 = Color32::from_rgb(64, 64, 64);
/// Behind a switched-on toggle: the face with light let into it.
pub const PUSHED_IN: Color32 = Color32::from_rgb(234, 232, 227);
/// A chosen row, the window in front, a menu under the pointer.
pub const NAVY: Color32 = Color32::from_rgb(10, 36, 106);
/// Where the window band's gradient ends, on the right.
pub const NAVY_END: Color32 = Color32::from_rgb(58, 110, 165);
/// Behind a tooltip.
pub const TIP: Color32 = Color32::from_rgb(255, 255, 225);

// ---- markers -----------------------------------------------------------------

/// The edge of a button, a checkbox or a text box at rest or under the pointer.
const RAISED: Color32 = Color32::from_rgb(1, 2, 3);
/// The edge of a button being pressed.
const PRESSED: Color32 = Color32::from_rgb(1, 2, 4);
/// The edge of a window, a menu or a tooltip.
const WINDOW: Color32 = Color32::from_rgb(1, 2, 5);
/// A separator line or a group's outline.
const ETCH: Color32 = Color32::from_rgb(127, 127, 128);
/// Behind a menu item or a list entry under the pointer.
const HOVER: Color32 = Color32::from_rgb(211, 207, 199);
/// Behind the title of the window in front, and a menu that is open.
const TITLE: Color32 = Color32::from_rgb(10, 36, 107);
/// Inside a checkbox; also a scroll bar's handle.
const BOX: Color32 = Color32::from_rgb(254, 254, 254);

/// The four edges of a bevel, from the outside in: top-left then
/// bottom-right, outer then inner.
#[derive(Clone, Copy)]
pub struct Edges {
    pub outer_lit: Color32,
    pub outer_shade: Color32,
    pub inner_lit: Color32,
    pub inner_shade: Color32,
}

/// A push button, and anything else that stands up off the face.
pub const RAISED_EDGES: Edges = Edges {
    outer_lit: HIGHLIGHT,
    outer_shade: DARK,
    inner_lit: FACE,
    inner_shade: SHADOW,
};

/// A window or a menu: lit on the inside, where a button is lit outside.
pub const WINDOW_EDGES: Edges = Edges {
    outer_lit: FACE,
    outer_shade: DARK,
    inner_lit: HIGHLIGHT,
    inner_shade: SHADOW,
};

/// A text box, a list or a checkbox: set into the face.
pub const SUNKEN_EDGES: Edges = Edges {
    outer_lit: SHADOW,
    outer_shade: HIGHLIGHT,
    inner_lit: DARK,
    inner_shade: FACE,
};

/// A button held down.
pub const PRESSED_EDGES: Edges = Edges {
    outer_lit: DARK,
    outer_shade: HIGHLIGHT,
    inner_lit: SHADOW,
    inner_shade: FACE,
};

/// egui's widgets, in the Classic look, with each frame marked so [`finish`]
/// can find it.
pub fn visuals() -> Visuals {
    let mut v = Visuals::light();
    v.dark_mode = false;
    v.override_text_color = None;
    v.panel_fill = FACE;
    v.window_fill = FACE;
    v.extreme_bg_color = Color32::WHITE;
    v.text_edit_bg_color = Some(Color32::WHITE);
    v.faint_bg_color = Color32::from_rgb(232, 229, 222);
    v.code_bg_color = Color32::WHITE;
    v.hyperlink_color = Color32::from_rgb(0, 0, 204);
    v.warn_fg_color = Color32::from_rgb(150, 75, 0);
    v.error_fg_color = Color32::from_rgb(170, 0, 0);
    v.window_stroke = Stroke::new(1.0_f32, WINDOW); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.window_shadow = egui::Shadow::NONE;
    v.popup_shadow = egui::Shadow::NONE;
    v.window_highlight_topmost = true;
    v.selection.bg_fill = NAVY;
    v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
    v.text_cursor.stroke = Stroke::new(1.5_f32, Color32::BLACK); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
    v.slider_trailing_fill = false;
    v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.55 };
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;

    let black = Stroke::new(1.0_f32, Color32::BLACK); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = FACE;
    w.noninteractive.weak_bg_fill = FACE;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, ETCH); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
    w.noninteractive.fg_stroke = black;
    w.inactive.bg_fill = BOX;
    w.inactive.weak_bg_fill = FACE;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, RAISED); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
    w.inactive.fg_stroke = black;
    w.hovered.bg_fill = BOX;
    w.hovered.weak_bg_fill = HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0_f32, RAISED); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
    w.hovered.fg_stroke = black;
    w.active.bg_fill = BOX;
    w.active.weak_bg_fill = FACE;
    w.active.bg_stroke = Stroke::new(1.0_f32, PRESSED); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
    w.active.fg_stroke = black;
    w.open.bg_fill = BOX;
    w.open.weak_bg_fill = TITLE;
    w.open.bg_stroke = Stroke::new(1.0_f32, RAISED); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
    w.open.fg_stroke = black;
    for widget in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        widget.corner_radius = CornerRadius::ZERO;
        widget.expansion = 0.0;
    }
    v
}

/// True while the Classic visuals are the ones in use.
pub fn active(ctx: &egui::Context) -> bool {
    ctx.style().visuals.window_stroke.color == WINDOW
}

/// Has [`finish`] run at the end of every frame. Safe to call more than once:
/// it is only ever added the first time.
pub fn install(ctx: &egui::Context) {
    let id = egui::Id::new("excalibur-classic-installed");
    if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(id, true));
    ctx.on_end_pass("classic look", Arc::new(finish));
}

/// Swaps every marked shape for its classic self. Does nothing unless the
/// Classic visuals are in use.
pub fn finish(ctx: &egui::Context) {
    if !active(ctx) {
        return;
    }
    let ppp = ctx.pixels_per_point();
    let mut layers: Vec<LayerId> = ctx.memory(|m| m.layer_ids().collect());
    if !layers.contains(&LayerId::background()) {
        layers.insert(0, LayerId::background());
    }
    ctx.graphics_mut(|graphics| {
        for layer in layers {
            let Some(list) = graphics.get_mut(layer) else { continue };
            let mut seen = Seen {
                tooltip: layer.order == Order::Tooltip,
                lit: Vec::new(),
                ppp,
            };
            let count = list.next_idx().0;
            for i in 0..count {
                list.mutate_shape(egui::layers::ShapeIdx(i), |clipped| {
                    restyle(&mut clipped.shape, &mut seen);
                });
            }
        }
    });
}

/// What has been met so far in one layer.
struct Seen {
    tooltip: bool,
    /// Navy areas already painted, whose words and marks go white.
    lit: Vec<Rect>,
    ppp: f32,
}

impl Seen {
    fn on_navy(&self, area: Rect) -> bool {
        let centre = area.center();
        self.lit.iter().any(|lit| lit.contains(centre))
    }
}

fn restyle(shape: &mut Shape, seen: &mut Seen) {
    match shape {
        Shape::Vec(shapes) => {
            for one in shapes {
                restyle(one, seen);
            }
        }
        Shape::Rect(rect) => {
            if let Some(classic) = frame(rect, seen) {
                *shape = classic;
            }
        }
        Shape::LineSegment { points, stroke } => {
            let colour = stroke.color;
            if colour == ETCH {
                *shape = etched_line(points[0], points[1], seen.ppp);
            } else if colour == WINDOW {
                // The line egui rules under a window's title: the band says
                // where the title ends already.
                *shape = Shape::Noop;
            } else if colour == RAISED || colour == PRESSED {
                stroke.color = SHADOW;
            } else if is_ink(colour) && seen.on_navy(Rect::from_two_pos(points[0], points[1])) {
                stroke.color = Color32::WHITE;
            }
        }
        Shape::Circle(circle) => {
            if circle.stroke.color == RAISED || circle.stroke.color == PRESSED {
                circle.stroke.color = SHADOW;
            }
            if circle.fill == BOX {
                circle.fill = Color32::WHITE;
            }
        }
        Shape::Path(path) => {
            if let ColorMode::Solid(colour) = path.stroke.color {
                if colour == RAISED || colour == PRESSED || colour == ETCH || colour == WINDOW {
                    path.stroke.color = ColorMode::Solid(SHADOW);
                } else if is_ink(colour) && seen.on_navy(path.visual_bounding_rect()) {
                    path.stroke.color = ColorMode::Solid(Color32::WHITE);
                }
            }
            if is_ink(path.fill) && seen.on_navy(path.visual_bounding_rect()) {
                path.fill = Color32::WHITE;
            }
        }
        Shape::Text(text) => {
            if seen.on_navy(text_area(text)) {
                text.override_text_color = Some(Color32::WHITE);
            }
        }
        _ => {}
    }
}

/// Black or near it: type and the marks drawn with it.
fn is_ink(colour: Color32) -> bool {
    colour.a() > 0 && colour.r() < 80 && colour.g() < 80 && colour.b() < 80
}

fn text_area(text: &TextShape) -> Rect {
    text.galley.rect.translate(text.pos.to_vec2())
}

/// The classic version of one of egui's frames, or `None` to leave it be.
fn frame(rect: &RectShape, seen: &mut Seen) -> Option<Shape> {
    let area = rect.rect;
    let stroked = rect.stroke.width > 0.0 && rect.stroke.color.a() > 0;
    let edge = if stroked { Some(rect.stroke.color) } else { None };
    let ppp = seen.ppp;
    match edge {
        Some(RAISED) => Some(if rect.fill == Color32::WHITE || rect.fill == BOX {
            bevel(area, SUNKEN_EDGES, Color32::WHITE, ppp)
        } else {
            bevel(area, RAISED_EDGES, FACE, ppp)
        }),
        Some(PRESSED) => Some(if rect.fill == Color32::WHITE || rect.fill == BOX {
            bevel(area, SUNKEN_EDGES, Color32::WHITE, ppp)
        } else {
            bevel(area, PRESSED_EDGES, FACE, ppp)
        }),
        Some(WINDOW) if seen.tooltip => Some(outlined(area, TIP, Color32::BLACK, ppp)),
        // egui's canvas: a white area edged like a window. Here it is a well.
        Some(WINDOW) if rect.fill == Color32::WHITE => {
            Some(bevel(area, SUNKEN_EDGES, Color32::WHITE, ppp))
        }
        Some(WINDOW) => Some(bevel(area, WINDOW_EDGES, FACE, ppp)),
        Some(ETCH) if rect.fill == Color32::WHITE => Some(bevel(area, SUNKEN_EDGES, Color32::WHITE, ppp)),
        Some(ETCH) => Some(etched_box(area, rect.fill, ppp)),
        // A text box with the cursor in it: egui outlines it in the
        // selection colour, which here is white.
        Some(Color32::WHITE) if rect.fill == Color32::WHITE => {
            Some(bevel(area, SUNKEN_EDGES, Color32::WHITE, ppp))
        }
        // A chosen entry: navy, without the white outline egui gives it.
        Some(Color32::WHITE) if rect.fill == NAVY => {
            seen.lit.push(area);
            Some(flat(area, NAVY, ppp))
        }
        None if rect.fill == HOVER => {
            seen.lit.push(area);
            Some(flat(area, NAVY, ppp))
        }
        None if rect.fill == TITLE => {
            seen.lit.push(area);
            Some(band(area, ppp))
        }
        None if rect.fill == NAVY => {
            seen.lit.push(area);
            None
        }
        // A scroll bar's handle stands up like a button.
        None if rect.fill == BOX => Some(bevel(area, RAISED_EDGES, FACE, ppp)),
        _ => None,
    }
}

// ---- drawing -----------------------------------------------------------------

/// One line's thickness: a whole number of the screen's pixels, so an edge is
/// crisp at any scaling rather than a smear either side of a pixel.
pub fn line_width(ppp: f32) -> f32 {
    ppp.round().max(1.0) / ppp
}

fn snapped(area: Rect, ppp: f32) -> Rect {
    let snap = |v: f32| (v * ppp).round() / ppp;
    Rect::from_min_max(
        Pos2::new(snap(area.min.x), snap(area.min.y)),
        Pos2::new(snap(area.max.x), snap(area.max.y)),
    )
}

/// A raised or sunken box, as one mesh of plain quads: no smoothing along
/// the edges, which is what keeps a one-pixel bevel looking like one.
pub fn bevel(area: Rect, edges: Edges, fill: Color32, ppp: f32) -> Shape {
    let r = snapped(area, ppp);
    let w = line_width(ppp);
    let mut mesh = Mesh::default();
    if fill.a() > 0 {
        mesh.add_colored_rect(r, fill);
    }
    if r.width() < 2.0 * w || r.height() < 2.0 * w {
        return Shape::mesh(mesh);
    }
    ring(&mut mesh, r, w, edges.outer_lit, edges.outer_shade);
    let inner = r.shrink(w);
    if inner.width() >= 2.0 * w && inner.height() >= 2.0 * w {
        ring(&mut mesh, inner, w, edges.inner_lit, edges.inner_shade);
    }
    Shape::mesh(mesh)
}

/// A thin, one-line bevel: the flat buttons of a toolbar under the pointer,
/// and the cells of a status bar.
pub fn thin(area: Rect, lit: Color32, shade: Color32, fill: Color32, ppp: f32) -> Shape {
    let r = snapped(area, ppp);
    let w = line_width(ppp);
    let mut mesh = Mesh::default();
    if fill.a() > 0 {
        mesh.add_colored_rect(r, fill);
    }
    if r.width() >= 2.0 * w && r.height() >= 2.0 * w {
        ring(&mut mesh, r, w, lit, shade);
    }
    Shape::mesh(mesh)
}

/// One ring of edges: lit along the top and left, shaded along the bottom and
/// right, the shaded edges running the full length the way Windows drew them.
fn ring(mesh: &mut Mesh, r: Rect, w: f32, lit: Color32, shade: Color32) {
    // Top and left.
    mesh.add_colored_rect(Rect::from_min_max(r.min, Pos2::new(r.max.x - w, r.min.y + w)), lit);
    mesh.add_colored_rect(Rect::from_min_max(r.min, Pos2::new(r.min.x + w, r.max.y - w)), lit);
    // Bottom and right.
    mesh.add_colored_rect(Rect::from_min_max(Pos2::new(r.min.x, r.max.y - w), r.max), shade);
    mesh.add_colored_rect(Rect::from_min_max(Pos2::new(r.max.x - w, r.min.y), r.max), shade);
}

/// A plain filled box with square corners.
fn flat(area: Rect, fill: Color32, ppp: f32) -> Shape {
    let mut mesh = Mesh::default();
    mesh.add_colored_rect(snapped(area, ppp), fill);
    Shape::mesh(mesh)
}

/// A box with a one-line outline, for tooltips.
fn outlined(area: Rect, fill: Color32, line: Color32, ppp: f32) -> Shape {
    thin(area, line, line, fill, ppp)
}

/// A group's outline, cut into the face: a shaded line with a lit one inside it.
fn etched_box(area: Rect, fill: Color32, ppp: f32) -> Shape {
    let r = snapped(area, ppp);
    let w = line_width(ppp);
    let mut mesh = Mesh::default();
    if fill.a() > 0 {
        mesh.add_colored_rect(r, fill);
    }
    if r.width() >= 3.0 * w && r.height() >= 3.0 * w {
        ring(&mut mesh, Rect::from_min_max(r.min, r.max - egui::vec2(w, w)), w, SHADOW, SHADOW);
        ring(&mut mesh, Rect::from_min_max(r.min + egui::vec2(w, w), r.max), w, HIGHLIGHT, HIGHLIGHT);
    }
    Shape::mesh(mesh)
}

/// A separator: a shaded line with a lit one beside it, so it reads as a
/// groove cut into the face.
fn etched_line(a: Pos2, b: Pos2, ppp: f32) -> Shape {
    let w = line_width(ppp);
    let snap = |v: f32| (v * ppp).round() / ppp;
    let mut mesh = Mesh::default();
    if (a.y - b.y).abs() <= (a.x - b.x).abs() {
        let y = snap(a.y - w * 0.5);
        let (x0, x1) = (snap(a.x.min(b.x)), snap(a.x.max(b.x)));
        mesh.add_colored_rect(Rect::from_min_max(Pos2::new(x0, y), Pos2::new(x1, y + w)), SHADOW);
        mesh.add_colored_rect(Rect::from_min_max(Pos2::new(x0, y + w), Pos2::new(x1, y + 2.0 * w)), HIGHLIGHT);
    } else {
        let x = snap(a.x - w * 0.5);
        let (y0, y1) = (snap(a.y.min(b.y)), snap(a.y.max(b.y)));
        mesh.add_colored_rect(Rect::from_min_max(Pos2::new(x, y0), Pos2::new(x + w, y1)), SHADOW);
        mesh.add_colored_rect(Rect::from_min_max(Pos2::new(x + w, y0), Pos2::new(x + 2.0 * w, y1)), HIGHLIGHT);
    }
    Shape::mesh(mesh)
}

/// The navy band across the top of the window in front, darker on the left.
pub fn band(area: Rect, ppp: f32) -> Shape {
    let r = snapped(area, ppp);
    let mut mesh = Mesh::default();
    let base = mesh.vertices.len() as u32;
    mesh.colored_vertex(r.left_top(), NAVY);
    mesh.colored_vertex(r.right_top(), NAVY_END);
    mesh.colored_vertex(r.right_bottom(), NAVY_END);
    mesh.colored_vertex(r.left_bottom(), NAVY);
    mesh.add_triangle(base, base + 1, base + 2);
    mesh.add_triangle(base, base + 2, base + 3);
    Shape::mesh(mesh)
}

// ---- type ----------------------------------------------------------------------

/// The Classic look's type: DejaVu Sans, cut down and renamed (see
/// `fonts/LICENSE-symbols.txt`), which is what Moo OS sets everything in.
static CLASSIC: &[u8] = include_bytes!("../fonts/classic.ttf");
static CLASSIC_BOLD: &[u8] = include_bytes!("../fonts/classic-bold.ttf");

/// The family for a bold title, in either look.
pub const BOLD: &str = "excalibur-bold";

/// Adds the Classic faces to a set of fonts: first in line for ordinary type
/// when `on`, and always as the bold family, so a bold title never asks for
/// a family that is not there.
pub fn add_fonts(fonts: &mut egui::FontDefinitions, on: bool) {
    fonts.font_data.insert(
        "excalibur-classic".into(),
        Arc::new(egui::FontData::from_static(CLASSIC)),
    );
    fonts.font_data.insert(
        "excalibur-classic-bold".into(),
        Arc::new(egui::FontData::from_static(CLASSIC_BOLD)),
    );
    let proportional = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut bold = vec!["excalibur-classic-bold".to_string()];
    bold.extend(proportional.iter().cloned());
    fonts.families.insert(egui::FontFamily::Name(BOLD.into()), bold);
    if on {
        if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            list.insert(0, "excalibur-classic".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_of(fill: Color32, stroke: Stroke) -> Shape {
        Shape::Rect(RectShape::new(
            Rect::from_min_size(Pos2::new(10.0, 10.0), egui::vec2(60.0, 20.0)),
            CornerRadius::ZERO,
            fill,
            stroke,
            egui::StrokeKind::Inside,
        ))
    }

    fn colours(shape: &Shape) -> Vec<Color32> {
        match shape {
            Shape::Mesh(mesh) => mesh.vertices.iter().map(|v| v.color).collect(),
            _ => Vec::new(),
        }
    }

    fn restyled(shape: Shape) -> Shape {
        let mut shape = shape;
        let mut seen = Seen { tooltip: false, lit: Vec::new(), ppp: 1.0 };
        restyle(&mut shape, &mut seen);
        shape
    }

    #[test]
    fn a_button_stands_up_and_a_text_box_sits_down() {
        let v = visuals();
        let button = restyled(frame_of(v.widgets.inactive.weak_bg_fill, v.widgets.inactive.bg_stroke));
        let c = colours(&button);
        assert!(c.contains(&HIGHLIGHT) && c.contains(&DARK) && c.contains(&FACE), "raised");
        let field = restyled(frame_of(v.extreme_bg_color, v.widgets.inactive.bg_stroke));
        let c = colours(&field);
        assert!(c.contains(&Color32::WHITE) && c.contains(&SHADOW), "sunken");
        // The markers never reach the screen.
        for shape in [&button, &field] {
            assert!(!colours(shape).contains(&RAISED));
        }
    }

    #[test]
    fn a_menu_item_under_the_pointer_goes_navy_with_white_words() {
        let v = visuals();
        let mut seen = Seen { tooltip: false, lit: Vec::new(), ppp: 1.0 };
        let mut hovered = frame_of(v.widgets.hovered.weak_bg_fill, Stroke::NONE);
        restyle(&mut hovered, &mut seen);
        assert!(colours(&hovered).contains(&NAVY));
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |_| {});
        let galley = ctx.fonts(|f| {
            f.layout_no_wrap("Open…".into(), egui::FontId::proportional(12.0), Color32::BLACK)
        });
        let mut words = Shape::galley(Pos2::new(14.0, 14.0), galley, Color32::BLACK);
        restyle(&mut words, &mut seen);
        match words {
            Shape::Text(text) => assert_eq!(text.override_text_color, Some(Color32::WHITE)),
            _ => panic!("still text"),
        }
    }

    #[test]
    fn something_it_does_not_know_is_left_alone() {
        let plain = frame_of(Color32::from_rgb(200, 30, 30), Stroke::new(2.0_f32, Color32::GREEN)); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
        assert!(matches!(restyled(plain), Shape::Rect(_)));
    }

    #[test]
    fn a_tooltip_is_pale_yellow() {
        let v = visuals();
        let mut shape = frame_of(v.window_fill, v.window_stroke);
        let mut seen = Seen { tooltip: true, lit: Vec::new(), ppp: 1.0 };
        restyle(&mut shape, &mut seen);
        assert!(colours(&shape).contains(&TIP));
    }

    #[test]
    fn an_edge_is_a_whole_number_of_pixels_at_any_scaling() {
        for ppp in [1.0f32, 1.25, 1.5, 2.0, 3.0] {
            let pixels = line_width(ppp) * ppp;
            assert!((pixels - pixels.round()).abs() < 1e-4 && pixels >= 1.0, "{ppp}");
        }
    }
}
