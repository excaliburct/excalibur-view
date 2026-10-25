//! The drawing canvas: transform, tile cache, painting and tool input.

use std::collections::HashMap;

use egui::{pos2, vec2, Color32, FontId, Pos2, Rect, Sense, Stroke, TextureHandle, Vec2};

use crate::render;
use crate::app::{App, Calibrating, Tool};

use crate::render::{PageSize, TileKey, TILE};
use crate::sheet::Split;

/// Where the sheet sits on screen. `screen = sheet * zoom + offset`.
#[derive(Clone, Debug)]
pub struct View {
    pub zoom: f32,
    pub offset: Vec2,
    pub fit_requested: bool,
}

impl View {
    pub fn new() -> View {
        View {
            zoom: 1.0,
            offset: Vec2::ZERO,
            fit_requested: true,
        }
    }

    /// Puts a point on the sheet in the middle of the window.
    pub fn centre_on(&mut self, at: [f64; 2], area: Rect) {
        let want = area.center();
        self.offset = egui::vec2(
            want.x - at[0] as f32 * self.zoom,
            want.y - at[1] as f32 * self.zoom,
        );
        self.fit_requested = false;
    }

    pub fn to_screen(&self, p: [f32; 2]) -> Pos2 {
        pos2(
            p[0] * self.zoom + self.offset.x,
            p[1] * self.zoom + self.offset.y,
        )
    }

    /// The same view, with the origin moved to another sheet's corner.
    ///
    /// This is the whole of what a page arrangement costs: a neighbouring
    /// sheet is drawn with exactly the code that draws the one being worked
    /// on, through a view whose origin has been moved to where that sheet
    /// sits.
    pub fn shifted(&self, by: [f64; 2]) -> View {
        let mut out = self.clone();
        out.offset.x += by[0] as f32 * self.zoom;
        out.offset.y += by[1] as f32 * self.zoom;
        out
    }

    pub fn to_sheet(&self, p: Pos2) -> [f32; 2] {
        [
            (p.x - self.offset.x) / self.zoom,
            (p.y - self.offset.y) / self.zoom,
        ]
    }

    pub fn fit(&mut self, area: Rect, size: PageSize) {
        let margin = 24.0;
        let zoom = ((area.width() - margin * 2.0) / size.width.max(1.0))
            .min((area.height() - margin * 2.0) / size.height.max(1.0))
            .clamp(0.01, 64.0);
        self.zoom = zoom;
        self.offset = area.center().to_vec2()
            - vec2(size.width * zoom * 0.5, size.height * zoom * 0.5);
        self.fit_requested = false;
    }

    /// Zooms so the point of the sheet under `anchor` stays under `anchor`.
    pub fn zoom_at(&mut self, anchor: Pos2, factor: f32) {
        let before = self.to_sheet(anchor);
        self.zoom = (self.zoom * factor).clamp(0.02, 64.0);
        self.offset = anchor.to_vec2() - vec2(before[0] * self.zoom, before[1] * self.zoom);
    }
}

/// Rendered tiles, kept on the GPU until the budget runs out and the
/// least recently used ones are dropped.
#[derive(Default)]
pub struct Tiles {
    map: HashMap<TileKey, TextureHandle>,
    order: Vec<TileKey>,
    /// Tiles that still show markups where they were before a save. They go
    /// on being drawn, so nothing flickers, and are asked for again.
    stale: std::collections::HashSet<TileKey>,
}

const TILE_BUDGET: usize = 256;

/// `HYPERVIEW_NO_PREFETCH=1` turns drawing ahead off, so a benchmark can say
/// what it is worth.
static NO_PREFETCH: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var_os("HYPERVIEW_NO_PREFETCH").is_some());

impl Tiles {
    /// Keeps a tile. `current` says it was drawn from the file as it is now;
    /// one that wasn't is kept for showing but still counts as wanted.
    pub fn insert_drawn(&mut self, key: TileKey, handle: TextureHandle, current: bool) {
        self.insert(key, handle);
        if !current {
            self.stale.insert(key);
        }
    }

    /// Every tile of these sheets is out of date.
    pub fn mark_stale(&mut self, pages: &std::collections::BTreeSet<u32>) {
        for key in &self.order {
            if pages.contains(&key.page) {
                self.stale.insert(*key);
            }
        }
    }

    /// Whether a tile is here and shows the file as it is now.
    pub fn fresh(&self, key: &TileKey) -> bool {
        self.map.contains_key(key) && !self.stale.contains(key)
    }

    pub fn insert(&mut self, key: TileKey, handle: TextureHandle) {
        self.stale.remove(&key);
        if self.map.insert(key, handle).is_none() {
            self.order.push(key);
        }
        while self.order.len() > TILE_BUDGET {
            let oldest = self.order.remove(0);
            self.map.remove(&oldest);
            self.stale.remove(&oldest);
        }
    }

    pub fn get(&mut self, key: &TileKey) -> Option<&TextureHandle> {
        if let Some(at) = self.order.iter().position(|k| k == key) {
            let k = self.order.remove(at);
            self.order.push(k);
        }
        self.map.get(key)
    }

    pub fn has(&self, key: &TileKey) -> bool {
        self.map.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
}


impl App {
    pub fn canvas(&mut self, ctx: &egui::Context) {
        let frame = egui::Frame::new().fill(self.chrome.theme.surround);
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            if self.doc().is_none() {
                self.welcome(ui);
                return;
            }
            let whole = ui.available_rect_before_wrap();
            self.last_canvas_whole = whole;
            let split = self.doc().map(|d| d.split).unwrap_or_default();
            let side = self.doc().map(|d| d.side).unwrap_or(0);
            let (first, second) = halves(whole, split);

            // The pane being worked in is whichever side it is on. Everything
            // below goes on treating `doc.view` and `doc.page` as the only
            // ones there are, which is why a split needed no changes anywhere
            // but here.
            // When the work is on the second side, the two rectangles swap
            // round; with no split there is only ever one.
            let (mine, theirs) = match (side, second) {
                (0, other) => (first, other),
                (_, Some(other)) => (other, Some(first)),
                (_, None) => (first, None),
            };
            self.last_canvas = mine;

            // The other pane first, so the one in use is painted over it and
            // its divider sits on top.
            if let Some(theirs) = theirs {
                self.paint_other_pane(ui, theirs);
            }

            let response = ui.allocate_rect(mine, Sense::click_and_drag());
            let painter = ui.painter_at(mine);

            let size = self.doc().unwrap().size();
            if self.doc().unwrap().view.fit_requested {
                self.doc_mut().unwrap().view.fit(mine, size);
            }

            self.navigate(ui, &response, mine);
            self.follow_the_scroll(mine);
            self.showing = self.sheets_showing(mine);
            self.tool_input(ui, &response, mine);
            if self.tool == Tool::Select && !self.typing_in_place() {
                self.markup_menu(&response);
            }
            self.request_tiles(mine);
            self.paint_sheet(&painter, mine, size);
            if self.dimmed {
                // Everything but the markups goes faint. Painted over the
                // sheet and under the markups, which is the whole of what
                // "dim the drawing" means.
                painter.rect_filled(mine, 0.0, Color32::from_white_alpha(150));
            }
            self.paint_grid(&painter, mine, size);
            self.paint_markups(&painter, mine);
            self.paint_paragraphs(&painter);
            self.paint_plugin_findings(&painter);
            self.paint_found(&painter, mine);
            self.paint_fill(&painter, mine);
            self.paint_readout(ui, &painter, mine);
            self.paint_snap(&painter, mine);

            if let Some(theirs) = theirs {
                self.pane_edges(ui, mine, theirs, split);
                self.carry_across(mine, theirs);
            }
        });
    }

    /// The mark that says what the pointer has caught: a square on the end of
    /// a line, a cross where two cross, a triangle at a midpoint, a ring on a
    /// centre, a bow tie on a line — the marks Revu uses, so nobody has to
    /// learn new ones — with the name beside it.
    fn paint_snap(&self, painter: &egui::Painter, area: Rect) {
        use takeoff::snap::Kind;
        let Some(hit) = self.snap_hit else { return };
        let Some(doc) = self.doc() else { return };
        let c = doc.view.to_screen([hit.at[0] as f32, hit.at[1] as f32]);
        if !area.contains(c) {
            return;
        }
        let colour = Color32::from_rgb(40, 220, 90);
        let under = Stroke::new(3.5_f32, Color32::from_black_alpha(160)); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
        let over = Stroke::new(1.6_f32, colour); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
        let r = 6.0;
        let shape = |stroke: Stroke| -> Vec<egui::Shape> {
            match hit.kind {
                Kind::Endpoint => vec![egui::Shape::rect_stroke(
                    Rect::from_center_size(c, egui::vec2(r * 2.0, r * 2.0)),
                    0.0,
                    stroke,
                    egui::StrokeKind::Middle,
                )],
                Kind::Intersection => vec![
                    egui::Shape::line_segment([c + egui::vec2(-r, -r), c + egui::vec2(r, r)], stroke),
                    egui::Shape::line_segment([c + egui::vec2(-r, r), c + egui::vec2(r, -r)], stroke),
                ],
                Kind::Midpoint => vec![egui::Shape::closed_line(
                    vec![c + egui::vec2(0.0, -r), c + egui::vec2(r, r * 0.8), c + egui::vec2(-r, r * 0.8)],
                    stroke,
                )],
                Kind::Centre => vec![
                    egui::Shape::circle_stroke(c, r, stroke),
                    egui::Shape::line_segment([c + egui::vec2(-2.0, 0.0), c + egui::vec2(2.0, 0.0)], stroke),
                    egui::Shape::line_segment([c + egui::vec2(0.0, -2.0), c + egui::vec2(0.0, 2.0)], stroke),
                ],
                Kind::OnLine => vec![egui::Shape::closed_line(
                    vec![
                        c + egui::vec2(-r, -r),
                        c + egui::vec2(r, r),
                        c + egui::vec2(r, -r),
                        c + egui::vec2(-r, r),
                    ],
                    stroke,
                )],
            }
        };
        painter.extend(shape(under));
        painter.extend(shape(over));
        let label = hit.kind.name();
        let at = c + egui::vec2(r + 6.0, r + 4.0);
        let galley = painter.layout_no_wrap(label.to_string(), egui::FontId::proportional(11.0), colour);
        let back = Rect::from_min_size(at, galley.size()).expand(2.0);
        painter.rect_filled(back, 2.0, Color32::from_black_alpha(170));
        painter.galley(at, galley, colour);
    }

    /// Draws the pane that is not being worked in.
    ///
    /// Read-only on purpose: markups go on the sheet somebody is working on,
    /// and a click over here moves the work rather than drawing something
    /// where they were only looking.
    fn paint_other_pane(&mut self, ui: &mut egui::Ui, area: Rect) {
        let Some(doc) = self.doc() else { return };
        let Some(other) = doc.other.as_ref() else {
            return;
        };
        let (page, size) = (
            other.page,
            doc.pages
                .get(other.page as usize)
                .copied()
                .unwrap_or(crate::render::PageSize {
                    width: 612.0,
                    height: 792.0,
                }),
        );
        let other = other.clone();
        let fit_wanted = other.view.fit_requested;
        if fit_wanted {
            if let Some(other) = self.doc_mut().and_then(|d| d.other.as_mut()) {
                other.view.fit(area, size);
            }
        }


        let painter = ui.painter_at(area);
        let theme = self.chrome.theme;
        let view = self
            .doc()
            .and_then(|d| d.other.as_ref().map(|o| o.view.clone()));
        let Some(view) = view else { return };
        let Some(doc) = self.doc_mut() else { return };
        paint_one(&painter, doc, &view, page, area, size, theme);

        // A sheet somebody is only looking at is dimmed a little, so which one
        // a measurement is about to land on is never in doubt.
        painter.rect_filled(area, 0.0, theme.chrome.gamma_multiply(0.18));
    }

    /// Dynamic Fill's half of the canvas: drawing boundaries, and the click
    /// that starts a fill.
    fn fill_input(
        &mut self,
        _ui: &mut egui::Ui,
        response: &egui::Response,
        pointer: egui::Pos2,
        area: Rect,
    ) {
        let Some(doc) = self.doc() else { return };
        let s = doc.view.to_sheet(pointer);
        let here = [s[0] as f64, s[1] as f64];
        let page = doc.page;

        if self.tool == Tool::FillBoundary {
            // A boundary is drawn freehand, the way somebody would strike a
            // line across a doorway with a pencil.
            if response.drag_started() {
                self.filling.drawing = Some(vec![here]);
            } else if response.dragged() {
                if let Some(line) = self.filling.drawing.as_mut() {
                    let far = line
                        .last()
                        .map(|p| (p[0] - here[0]).powi(2) + (p[1] - here[1]).powi(2))
                        .unwrap_or(f64::MAX);
                    if far > 2.0 {
                        line.push(here);
                    }
                }
            } else if response.drag_stopped() {
                if let Some(line) = self.filling.drawing.take() {
                    if line.len() > 1 {
                        self.filling.strokes.push(line);
                        // A new boundary changes what a fill would find, so
                        // the last one stops being an answer.
                        self.filling.clear();
                    }
                }
            }
            return;
        }

        if !response.clicked() {
            return;
        }
        // Only what can be seen is looked at, which is what makes an escape
        // detectable rather than an ever-widening search.
        let top_left = doc.view.to_sheet(area.left_top());
        let bottom_right = doc.view.to_sheet(area.right_bottom());
        let look = [
            top_left[0] as f64,
            top_left[1] as f64,
            bottom_right[0] as f64,
            bottom_right[1] as f64,
        ];
        let id = doc.id;
        let strokes = self.filling.strokes.clone();
        let darkness = self.filling.darkness;
        let job = self.filling.begin(page);
        self.svc.send(render::ToWorker::DynamicFill {
            doc: id,
            page,
            job,
            area: look,
            seed: here,
            strokes,
            darkness,
        });
    }

    /// Draws the boundaries somebody has struck and the shape a fill found.
    fn paint_fill(&mut self, painter: &egui::Painter, _area: Rect) {
        if !self.tool.is_filling() && self.filling.outcome.is_none() {
            return;
        }
        let theme = self.chrome.theme;
        let Some(doc) = self.doc() else { return };
        let view = &doc.view;
        let to_screen = |p: &[f64; 2]| view.to_screen([p[0] as f32, p[1] as f32]);

        // The lines drawn to close openings, in the warning colour: they are
        // something the user added, not something the drawing says.
        let mut lines: Vec<&Vec<[f64; 2]>> = self.filling.strokes.iter().collect();
        if let Some(drawing) = self.filling.drawing.as_ref() {
            lines.push(drawing);
        }
        for line in lines {
            let points: Vec<egui::Pos2> = line.iter().map(to_screen).collect();
            if points.len() > 1 {
                painter.add(egui::Shape::line(
                    points,
                    Stroke::new(2.5_f32, theme.warn), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                ));
            }
        }

        // Only on the sheet it was found on.
        if self.filling.page != doc.page {
            return;
        }
        let Some(crate::render::FillOutcome::Found { outline, holes, .. }) =
            self.filling.outcome.as_ref()
        else {
            return;
        };
        let points: Vec<egui::Pos2> = outline.iter().map(to_screen).collect();
        if points.len() < 3 {
            return;
        }
        fill_shape(painter, &points, theme.accent.gamma_multiply(0.42));
        painter.add(egui::Shape::closed_line(
            points,
            Stroke::new(2.0_f32, theme.accent_text), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
        ));
        // The openings taken out, drawn as what they are: not part of it.
        for hole in holes {
            let ring: Vec<egui::Pos2> = hole.iter().map(to_screen).collect();
            if ring.len() >= 3 {
                fill_shape(painter, &ring, theme.surround.gamma_multiply(0.9));
                painter.add(egui::Shape::closed_line(
                    ring,
                    Stroke::new(1.5_f32, theme.warn), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                ));
            }
        }
    }

    /// Moves the other pane with this one, when the two are tied together.
    ///
    /// Tied by the middle of the sheet rather than by the offset, so two panes
    /// of different sizes stay looking at the same part of the building rather
    /// than drifting apart by however much wider one of them is.
    fn carry_across(&mut self, mine: Rect, theirs: Rect) {
        let Some(doc) = self.doc() else { return };
        if !doc.sync_panes || doc.other.is_none() {
            return;
        }
        let middle = doc.view.to_sheet(mine.center());
        let zoom = doc.view.zoom;
        if let Some(other) = self.doc_mut().and_then(|d| d.other.as_mut()) {
            other.view.zoom = zoom;
            other.view.centre_on([middle[0] as f64, middle[1] as f64], theirs);
        }
    }

    /// The line between the panes, and the click that moves the work across.
    fn pane_edges(&mut self, ui: &mut egui::Ui, mine: Rect, theirs: Rect, split: Split) {
        let theme = self.chrome.theme;
        let painter = ui.painter();
        painter.rect_filled(
            match split {
                Split::Vertical => Rect::from_min_max(
                    pos2(mine.left().max(theirs.left()) - 1.0, mine.top()),
                    pos2(mine.left().max(theirs.left()) + 1.0, mine.bottom()),
                ),
                _ => Rect::from_min_max(
                    pos2(mine.left(), mine.top().max(theirs.top()) - 1.0),
                    pos2(mine.right(), mine.top().max(theirs.top()) + 1.0),
                ),
            },
            0.0,
            theme.line,
        );
        // The pane being worked in is edged, so it is never a guess which one
        // a measurement is about to land on.
        painter.rect_stroke(
            mine.shrink(1.0),
            0.0,
            Stroke::new(1.5_f32, theme.accent_text.gamma_multiply(0.7)), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
            egui::StrokeKind::Inside,
        );

        let over = ui
            .input(|i| i.pointer.hover_pos())
            .map(|p| theirs.contains(p))
            .unwrap_or(false);
        let clicked = ui.input(|i| i.pointer.any_pressed());
        if over && clicked {
            if let Some(doc) = self.doc_mut() {
                doc.switch_pane();
            }
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let theme = self.chrome.theme;
        // Set inside the closure below and acted on after it, because picking a
        // file needs `self` and the closure already has it borrowed.
        let mut want_chest = false;
        let area = ui.available_rect_before_wrap();
        let painter = ui.painter_at(area);

        // The mark, big, above the middle, with the name beside it.
        let block = ui::mark::wordmark(
            &painter,
            egui::pos2(
                area.center().x - 175.0,
                area.center().y - 150.0,
            ),
            120.0,
            theme,
        );
        painter.text(
            egui::pos2(block.left(), block.bottom() + 26.0),
            egui::Align2::LEFT_TOP,
            "Drawing viewer, markup and takeoff.",
            egui::FontId::proportional(15.0),
            theme.faint,
        );

        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                egui::pos2(block.left(), block.bottom() + 66.0),
                egui::vec2(420.0, 200.0),
            )),
            |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Open a drawing set…").clicked() {
                        self.pick_and_open();
                    }
                    if ui.button("Connect to a server…").clicked() {
                        self.chrome.fire("Server.Connect");
                    }
                });
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("or drag a PDF onto this window")
                        .color(theme.faint)
                        .size(12.0),
                );
                if let Some(profile) = self.profile.as_ref() {
                    let tools: usize = profile.sets.iter().map(|s| s.tools.len()).sum();
                    ui.add_space(14.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · {tools} tools in {} sets",
                            profile.name,
                            profile.sets.len()
                        ))
                        .color(theme.faint)
                        .size(11.0),
                    );
                } else {
                    ui.add_space(14.0);
                    ui.label(
                        egui::RichText::new("No tool chest loaded.")
                            .color(theme.faint)
                            .size(11.0),
                    );
                    ui.add_space(6.0);
                    // A button rather than an instruction. Somebody's tools,
                    // their columns and their unit weights are the difference
                    // between this program and a PDF reader, and "copy a file
                    // into a folder beside the program" is a sentence that
                    // loses people before they have seen any of it.
                    if ui.button("Load a tool chest…").clicked() {
                        want_chest = true;
                    }
                }
            },
        );
        if want_chest {
            self.pick_and_load_chest();
        }
    }

    /// Marks what the search found on the sheet being looked at.
    ///
    /// Every answer on this sheet is shaded; the one being pointed at gets a
    /// ring as well, because "where on this sheet" is the question somebody
    /// clicking a search result is actually asking.
    fn paint_found(&self, painter: &egui::Painter, area: Rect) {
        let Some(doc) = self.doc() else { return };
        let Some(hits) = self.search.hits.get(&doc.page) else {
            return;
        };
        let theme = self.chrome.theme;
        let view = &doc.view;
        for (index, hit) in hits.iter().enumerate() {
            let a = view.to_screen([hit.area[0] as f32, hit.area[1] as f32]);
            let b = view.to_screen([hit.area[2] as f32, hit.area[3] as f32]);
            let rect = Rect::from_min_max(a, b).expand(2.0);
            if !area.intersects(rect) {
                continue;
            }
            let chosen = self.search.showing == Some((doc.page, index));
            painter.rect_filled(
                rect,
                2.0,
                if chosen {
                    theme.warn.gamma_multiply(0.55)
                } else {
                    theme.accent.gamma_multiply(0.35)
                },
            );
            if chosen {
                painter.rect_stroke(
                    rect.expand(2.0),
                    3.0,
                    Stroke::new(1.5_f32, theme.warn), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                    egui::StrokeKind::Middle,
                );
            }
        }
    }

    /// Two fingers move and zoom the sheet, whatever tool is in hand, and a
    /// touch that becomes two fingers takes back whatever its first finger had
    /// started: the start of a box, a stroke of ink, a markup nudged, a loop
    /// being drawn. One finger on bare paper with the Select tool moves the
    /// sheet, which is what a finger on a tablet expects to do.
    ///
    /// True while the fingers have the sheet, when no tool should act on it.
    fn touch(&mut self, ui: &mut egui::Ui, response: &egui::Response) -> bool {
        let (touching, pinch) = ui.input(|i| (i.any_touches(), i.multi_touch()));
        if !touching {
            // The last finger has come off. If the fingers had the sheet,
            // nothing they did is a tool's: not on this frame, where the
            // lift arrives, and not on the next, where a lift that never
            // moved can still read as a tap.
            let was = std::mem::take(&mut self.touch);
            if was.pinching || was.panning {
                self.touch.settling = true;
                return true;
            }
            return false;
        }
        self.touch.settling = false;
        if !self.touch.down {
            self.touch.down = true;
            self.touch.draft_before = self.doc().map(|d| d.draft.clone());
        }
        if let Some(pinch) = pinch.filter(|p| p.num_touches >= 2) {
            if !self.touch.pinching {
                self.touch.pinching = true;
                self.touch.panning = false;
                self.lasso = None;
                if let Some(moving) = self.moving.take() {
                    if moving.moved {
                        if let Some(doc) = self.doc_mut() {
                            doc.undo();
                        }
                    }
                }
                if let (Some(before), Some(doc)) = (self.touch.draft_before.take(), self.doc_mut()) {
                    doc.draft = before;
                }
            }
            if let Some(doc) = self.doc_mut() {
                doc.view.offset += pinch.translation_delta;
                if pinch.zoom_delta != 1.0 {
                    doc.view.zoom_at(pinch.center_pos, pinch.zoom_delta);
                }
            }
            ui.ctx().request_repaint();
            return true;
        }
        if self.touch.pinching {
            return true;
        }
        if self.touch.panning {
            if let Some(doc) = self.doc_mut() {
                doc.view.offset += response.drag_delta();
            }
            return true;
        }
        false
    }

    fn navigate(&mut self, ui: &mut egui::Ui, response: &egui::Response, area: Rect) {
        if self.touch(ui, response) {
            return;
        }
        let wheel_zooms = self.wheel_zooms;
        let space = ui.input(|i| i.key_down(egui::Key::Space));
        let panning = self.tool == Tool::Pan || space;
        let doc = self.doc_mut().unwrap();

        if response.dragged_by(egui::PointerButton::Middle)
            || (panning && response.dragged_by(egui::PointerButton::Primary))
        {
            doc.view.offset += response.drag_delta();
        }

        if response.hovered() {
            let (scroll, zoom_gesture, modifiers) = ui.input(|i| {
                (
                    i.smooth_scroll_delta,
                    i.zoom_delta(),
                    i.modifiers,
                )
            });
            let anchor = ui
                .input(|i| i.pointer.hover_pos())
                .unwrap_or_else(|| area.center());
            if zoom_gesture != 1.0 {
                doc.view.zoom_at(anchor, zoom_gesture);
            } else if scroll.y != 0.0 {
                if modifiers.shift {
                    // Shift turns the wheel into a sideways pan, the way every
                    // other drawing program does.
                    doc.view.offset.x += scroll.y;
                } else if wheel_zooms != modifiers.ctrl {
                    doc.view.zoom_at(anchor, (scroll.y * 0.0022).exp());
                } else {
                    doc.view.offset.y += scroll.y;
                }
            }
            if scroll.x != 0.0 {
                doc.view.offset.x += scroll.x;
            }
        }
    }

    /// What a point being put down snaps to: the drawing's own line-work —
    /// ends, crossings, centres, midpoints, anywhere along a line — a corner
    /// of a markup already on the sheet, a point of the one being drawn, or a
    /// grid line.
    ///
    /// Each can be turned off on its own, because they suit different work:
    /// snapping to line-work is what makes a takeoff trace a wall, and is
    /// exactly what gets in the way when somebody is drawing a cloud. Holding
    /// Ctrl turns all of it off for as long as it is held, which is how Revu
    /// does it too.
    pub fn snap(&self, at: [f64; 2], reach: f64) -> Option<takeoff::snap::Hit> {
        use takeoff::snap::{Hit, Kind};
        let doc = self.doc()?;
        let frame = doc.frame();
        // Corners of markups count as ends of lines: nearest wins, and they
        // beat anything of a lower kind the same way.
        let mut best: Option<(Hit, f64)> = None;
        let mut consider = |p: [f64; 2], kind: Kind, handicap: f64| {
            let d = ((p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2)).sqrt();
            if d > reach {
                return;
            }
            let score = d + handicap * reach;
            if best.map_or(true, |(_, s)| score < s) {
                best = Some((Hit { at: p, kind }, score));
            }
        };
        if self.prefs.snap_to_markup {
            for (_, mark) in doc.on_page(doc.page) {
                for p in mark.on_sheet(&frame) {
                    consider(p, Kind::Endpoint, 0.0);
                }
            }
        }
        // The markup being drawn always snaps to itself, whatever the
        // preferences say: closing a shape on its own first corner is what
        // finishing it means.
        if let Some(draft) = doc.draft.as_ref() {
            for p in &draft.points {
                consider(*p, Kind::Endpoint, 0.0);
            }
        }
        if self.prefs.snap_to_content {
            if let Some(geometry) = doc.geometry.get(&doc.page) {
                if let Some(hit) = geometry.snap(at, reach, &|_| true) {
                    let handicap = match hit.kind {
                        Kind::Endpoint => 0.0,
                        Kind::Intersection => 0.12,
                        Kind::Centre => 0.18,
                        Kind::Midpoint => 0.3,
                        Kind::OnLine => 0.65,
                    };
                    consider(hit.at, hit.kind, handicap);
                }
            }
        }
        if self.prefs.snap_to_grid {
            if let Some(p) = crate::places::on_grid(at, self.grid_spacing(), reach) {
                consider(p, Kind::Intersection, 0.4);
            }
        }
        best.map(|(hit, _)| hit)
    }

    /// How far apart the grid lines are on this sheet, in the sheet's own
    /// points.
    ///
    /// The spacing is in real units — a foot, say — so on a quarter-inch plan
    /// the grid is a foot on the building, not a foot on the paper. A sheet
    /// with no scale gets an inch on the paper, because an unscaled sheet has
    /// no real units to be a foot of.
    pub fn grid_spacing(&self) -> f64 {
        let wanted = self.prefs.grid_spacing.max(0.01);
        match self.doc().and_then(|d| d.scale()) {
            Some(scale) => scale.points_for(wanted).max(0.5),
            None => 72.0,
        }
    }

    /// Where a click actually lands: snapped to something already drawn, or
    /// squared up to the last point when shift is held.
    fn placed_point(&mut self, ui: &egui::Ui, raw: Pos2) -> [f64; 2] {
        self.snap_hit = None;
        let doc = self.doc().unwrap();
        let s = doc.view.to_sheet(raw);
        let sheet = [s[0] as f64, s[1] as f64];
        // Ten screen pixels, whatever the zoom: close enough to catch what the
        // eye is on, not so far it jumps to the next line over.
        let reach = 10.0 / doc.view.zoom as f64;
        // Picking things up does not snap, except when moving a markup's
        // corners, which is drawing by another name.
        let selecting = self.tool == Tool::Select && self.points_mode.is_none();
        let off = ui.input(|i| i.modifiers.ctrl) || !self.tool.snaps() || selecting;
        if !off {
            if let Some(hit) = self.snap(sheet, reach) {
                self.snap_hit = Some(hit);
                return hit.at;
            }
        }
        let doc = self.doc().unwrap();
        if ui.input(|i| i.modifiers.shift) {
            if let Some(last) = doc.draft.as_ref().and_then(|d| d.last()) {
                let (dx, dy) = (sheet[0] - last[0], sheet[1] - last[1]);
                if dx.abs() > dy.abs() * 2.0 {
                    return [sheet[0], last[1]];
                }
                if dy.abs() > dx.abs() * 2.0 {
                    return [last[0], sheet[1]];
                }
                let run = (dx.abs() + dy.abs()) * 0.5;
                return [last[0] + run * dx.signum(), last[1] + run * dy.signum()];
            }
        }
        sheet
    }

    /// The grips of what is selected: one markup, or one group of them (a
    /// Cloud+), on the sheet in view and not locked. Each with its markup.
    pub fn grips_showing(&self) -> Option<Vec<(usize, crate::grips::Handle)>> {
        if self.tool != Tool::Select || self.points_mode.is_some() || self.typing_in_place() {
            return None;
        }
        let doc = self.doc()?;
        let held = doc.selection();
        if !crate::groups::one_thing(&doc.marks, &held) {
            return None;
        }
        let frame = doc.frame();
        let mut out = Vec::new();
        for index in held {
            let mark = doc.marks.get(index)?;
            if mark.gone || mark.page != doc.page || locked(&mark.markup) {
                return None;
            }
            for handle in crate::grips::handles(&mark.markup, &frame) {
                out.push((index, handle));
            }
        }
        (!out.is_empty()).then_some(out)
    }

    /// Whether a text box's words are being typed on the sheet itself.
    pub fn typing_in_place(&self) -> bool {
        let Some(index) = self.editing_text else { return false };
        self.doc()
            .and_then(|doc| doc.marks.get(index).map(|m| (m, doc.page)))
            .is_some_and(|(m, page)| m.page == page && m.markup.subtype() == annot::Subtype::FreeText)
    }

    /// Grips first: a drag that starts on one changes the markup's size or
    /// its points rather than moving it. Comes back true when it took the
    /// input.
    fn grip_input(&mut self, ui: &egui::Ui, response: &egui::Response, pointer: Pos2) -> bool {
        use crate::grips;
        if let Some(mut g) = self.gripping.take() {
            ui.ctx().set_cursor_icon(grips::cursor(g.grip));
            if ui.input(|i| i.pointer.primary_down()) {
                let shift = ui.input(|i| i.modifiers.shift);
                let doc = self.docs.get_mut(self.current).unwrap();
                let s = doc.view.to_sheet(pointer);
                let by = [s[0] as f64 - g.from[0], s[1] as f64 - g.from[1]];
                if !g.moved && (by[0] != 0.0 || by[1] != 0.0) {
                    doc.checkpoint_named(match g.grip {
                        grips::Grip::Side(_) => "Resize",
                        grips::Grip::Point(_) => "Move a point",
                        grips::Grip::Leader(_) => "Move the leader",
                    });
                    g.moved = true;
                }
                if g.moved {
                    let frame = doc.frame();
                    let changed = grips::dragged(&g.original, &frame, g.grip, by, shift);
                    // A Cloud+'s words keep pointing at the same part of the
                    // cloud as the cloud changes size.
                    let resized_cloud = matches!(g.grip, grips::Grip::Side(_))
                        && g.original.subtype() != annot::Subtype::FreeText;
                    if resized_cloud {
                        if let (Some(was), Some(now)) = (g.original.drawn_box(), changed.drawn_box()) {
                            for (partner, original) in &g.partners {
                                if original.subtype() == annot::Subtype::FreeText {
                                    if let Some(mark) = doc.marks.get_mut(*partner) {
                                        mark.markup = grips::follow_cloud(original, was, now);
                                        mark.changed = true;
                                    }
                                }
                            }
                        }
                    }
                    if let Some(mark) = doc.marks.get_mut(g.index) {
                        mark.markup = changed;
                        mark.changed = true;
                    }
                    doc.dirty = true;
                    doc.remeasure_selection();
                }
                self.gripping = Some(g);
            } else if g.moved {
                self.save_soon();
                self.status = match g.grip {
                    grips::Grip::Side(_) if grips::is_picture(&g.original) => {
                        "Resized. Hold Shift to stretch it instead; Undo puts it back.".into()
                    }
                    grips::Grip::Side(_) => {
                        "Resized. Hold Shift on a corner to keep its shape; Undo puts it back.".into()
                    }
                    _ => "Moved. Undo puts it back.".into(),
                };
            }
            return true;
        }
        let Some(handles) = self.grips_showing() else {
            return false;
        };
        let doc = self.doc().unwrap();
        let reach = 8.0 / doc.view.zoom as f64;
        let under = |at: [f64; 2]| {
            handles
                .iter()
                .map(|(i, h)| (*i, h.grip, (h.at[0] - at[0]).powi(2) + (h.at[1] - at[1]).powi(2)))
                .filter(|(_, _, d)| *d <= reach * reach)
                .min_by(|a, b| a.2.total_cmp(&b.2))
                .map(|(i, g, _)| (i, g))
        };
        let s = doc.view.to_sheet(pointer);
        if let Some((_, grip)) = under([s[0] as f64, s[1] as f64]) {
            ui.ctx().set_cursor_icon(grips::cursor(grip));
        }
        if response.drag_started() {
            let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(pointer);
            let s = doc.view.to_sheet(press);
            let from = [s[0] as f64, s[1] as f64];
            if let Some((index, grip)) = under(from) {
                let original = doc.marks[index].markup.clone();
                let partners = crate::groups::group_of(&doc.marks, index)
                    .into_iter()
                    .filter(|i| *i != index)
                    .map(|i| (i, doc.marks[i].markup.clone()))
                    .collect();
                self.gripping = Some(grips::Gripping { index, grip, from, original, moved: false, partners });
                return true;
            }
        }
        false
    }

    /// The Cloud+ tool: drag out the cloud, then click where its words go.
    fn cloud_plus_input(&mut self, response: &egui::Response, at: [f64; 2]) {
        let waiting = self.cloud_waiting
            && self.doc().is_some_and(|d| {
                d.draft.as_ref().is_some_and(|draft| draft.tool == Tool::CloudPlus && draft.points.len() == 2)
            });
        self.cloud_waiting = waiting;
        if waiting {
            if response.clicked() {
                self.place_cloud_plus(at);
            }
            return;
        }
        if response.drag_started() {
            self.start_draft();
            self.push_point(at);
            self.push_point(at);
        } else if response.dragged() {
            if let Some(draft) = self.doc_mut().and_then(|d| d.draft.as_mut()) {
                if draft.points.len() == 2 {
                    draft.points[1] = at;
                }
            }
        } else if response.drag_stopped() {
            let Some(doc) = self.doc_mut() else { return };
            let zoom = doc.view.zoom as f64;
            let big_enough = doc.draft.as_ref().is_some_and(|d| {
                d.points.len() == 2 && a_real_drag(d.points[0], d.points[1], zoom)
            });
            if big_enough {
                self.cloud_waiting = true;
                self.status = "Now click where the words go.".into();
            } else {
                doc.draft = None;
            }
        }
    }

    /// Puts down a Cloud+: the cloud that was dragged out, and a callout from
    /// it to a box of words where the click was, then opens the box for
    /// typing.
    fn place_cloud_plus(&mut self, click: [f64; 2]) {
        self.cloud_waiting = false;
        let pen = self.pen.clone();
        let Some(doc) = self.doc_mut() else { return };
        let Some(draft) = doc.draft.take() else { return };
        let frame = doc.frame();
        let scale = doc.scale();
        let Some(mut cloud) = draft.into_markup(&frame, scale.as_ref()) else { return };
        let Some(cloud_box) = cloud.drawn_box() else { return };
        cloud.set_subject(crate::groups::CLOUD_PLUS);
        let name = annot::name::fresh();
        cloud.set_name(&name);

        // The box of words goes out from the click, away from the cloud.
        let p = frame.to_pdf(click);
        let (w, h) = (200.0, 54.0);
        let middle = [(cloud_box[0] + cloud_box[2]) * 0.5, (cloud_box[1] + cloud_box[3]) * 0.5];
        let (x0, x1) = if p[0] < middle[0] { (p[0] - w, p[0]) } else { (p[0], p[0] + w) };
        let (y0, y1) = if p[1] > middle[1] { (p[1], p[1] + h) } else { (p[1] - h, p[1]) };
        let words_box = [x0, y0, x1, y1];

        let mut words = annot::Markup::new(annot::Subtype::FreeText);
        pen.onto(&mut words);
        // The fill is the cloud's; the words sit on the drawing.
        words.dict.remove("IC");
        words.set_multiply(false);
        if words.width() <= 0.0 {
            words.set_width(1.0);
        }
        words.set_box(words_box);
        words.set_subject(crate::groups::CLOUD_PLUS);
        words.set("IT", pdf::Object::name("FreeTextCallout"));
        words.set(crate::groups::PART_OF, pdf::Object::text(&name));
        words.set_name(&annot::name::fresh());
        let line = crate::grips::leader_between(cloud_box, words_box);
        words.set(
            "CL",
            pdf::Object::Array(line.iter().flat_map(|q| [pdf::Object::real(q[0]), pdf::Object::real(q[1])]).collect()),
        );

        doc.checkpoint_named("Draw Cloud+");
        let page = doc.page;
        doc.marks.push(crate::sheet::Mark::new(page, cloud));
        let head = doc.marks.len() - 1;
        doc.marks.push(crate::sheet::Mark::new(page, words));
        let typed = doc.marks.len() - 1;
        doc.dirty = true;
        doc.choose(Some(head));
        self.editing_text = Some(typed);
        self.typing_fit = Some(typed);
        self.status = "Type what the cloud is about. Click anywhere else when it's done.".into();
    }

    fn tool_input(&mut self, ui: &mut egui::Ui, response: &egui::Response, _area: Rect) {
        if self.tool == Tool::Pan || self.touch.pinching || self.touch.panning || self.touch.settling {
            return;
        }
        // Words being typed into a box on the sheet have the pointer to
        // themselves; a click elsewhere finishes them and does nothing else.
        if self.typing_in_place() {
            return;
        }
        // That click, still on its way up. Letting it through put a fresh
        // empty text box wherever somebody clicked to finish the last one.
        if self.finishing_click {
            if !ui.input(|i| i.pointer.any_down()) {
                self.finishing_click = false;
            }
            return;
        }
        let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        let at = self.placed_point(ui, pointer);
        let tool = self.tool;

        if tool.is_filling() {
            self.fill_input(ui, response, pointer, _area);
            return;
        }

        if tool == Tool::EditText {
            self.edit_text_input(ui, response, pointer);
            return;
        }

        if tool == Tool::Select {
            // A point mode takes the click before selection does, so clicking
            // a corner changes it rather than picking something else up.
            if response.clicked() && self.points_mode.is_some() && self.edit_points_at(at) {
                return;
            }
            if self.grip_input(ui, response, pointer) {
                return;
            }
            // A right-click is about what is under the pointer: it becomes
            // what is selected, unless it already is part of it.
            if response.secondary_clicked() {
                let doc = self.doc_mut().unwrap();
                let frame = doc.frame();
                let page = doc.page;
                let reach = (12.0 / doc.view.zoom as f64).powi(2);
                let s = doc.view.to_sheet(pointer);
                match hit_at(doc, &frame, page, [s[0] as f64, s[1] as f64], reach) {
                    Some(i) if !doc.selection().contains(&i) => doc.choose(Some(i)),
                    Some(_) => {}
                    None => doc.choose(None),
                }
                self.take_pen_from_selection();
            }
            // Double-clicking a text box or a callout opens its words for
            // typing, right there on the sheet.
            if response.double_clicked() && self.points_mode.is_none() {
                let doc = self.doc_mut().unwrap();
                let frame = doc.frame();
                let page = doc.page;
                let reach = (12.0 / doc.view.zoom as f64).powi(2);
                let s = doc.view.to_sheet(pointer);
                if let Some(i) = hit_at(doc, &frame, page, [s[0] as f64, s[1] as f64], reach) {
                    // A Cloud+'s cloud opens its words too.
                    let words = if doc.marks[i].markup.subtype() == annot::Subtype::FreeText {
                        Some(i)
                    } else {
                        crate::groups::words_of(&doc.marks, i)
                    };
                    if let Some(w) = words.filter(|w| !locked(&doc.marks[*w].markup)) {
                        doc.choose(Some(i));
                        doc.checkpoint_named("Edit text");
                        self.editing_text = Some(w);
                        return;
                    }
                }
            }
            if response.clicked() {
                // Holding Ctrl or Shift adds to what is already held, which is
                // how somebody picks out six beams to line up or delete.
                let adding = ui.input(|i| i.modifiers.command || i.modifiers.shift);
                let brushing = self.painting_format.is_some();
                let doc = self.doc_mut().unwrap();
                let frame = doc.frame();
                let page = doc.page;
                let reach = (12.0 / doc.view.zoom as f64).powi(2);
                let s = doc.view.to_sheet(pointer);
                let sheet = [s[0] as f64, s[1] as f64];
                let hit = hit_at(doc, &frame, page, sheet, reach);
                let mut brush_onto = None;
                match (brushing, adding, hit) {
                    // The Format Painter is in hand: a click puts the look on
                    // rather than changing what is held.
                    (true, _, Some(i)) => {
                        doc.choose(Some(i));
                        brush_onto = Some(i);
                    }
                    (true, _, None) => {}
                    (false, true, Some(i)) => doc.choose_also(i),
                    (false, true, None) => {}
                    (false, false, found) => doc.choose(found),
                }
                if let Some(i) = brush_onto {
                    self.brush_format_onto(i);
                }
            }
            // Dragging a markup moves it — and everything held with it.
            // Dragging on empty paper draws a box round what is wanted, the
            // same as the Lasso tool does with a loop.
            if response.drag_started() && self.points_mode.is_none() {
                let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(pointer);
                let adding = ui.input(|i| i.modifiers.command || i.modifiers.shift);
                let doc = self.doc_mut().unwrap();
                let frame = doc.frame();
                let page = doc.page;
                let reach = (12.0 / doc.view.zoom as f64).powi(2);
                let s = doc.view.to_sheet(press);
                let from = [s[0] as f64, s[1] as f64];
                if let Some(i) = hit_at(doc, &frame, page, from, reach) {
                    if !doc.selection().contains(&i) {
                        if adding {
                            doc.choose_also(i);
                        } else {
                            doc.choose(Some(i));
                        }
                    }
                    // A Cloud+'s box of words moves on its own, its leader
                    // still pointing at the cloud. The cloud moves the lot.
                    let words_alone = doc.marks[i].markup.subtype() == annot::Subtype::FreeText
                        && crate::groups::head_of(&doc.marks, i) != i;
                    self.moving = Some(crate::app::Moving {
                        last: from,
                        moved: false,
                        only: words_alone.then_some(i),
                    });
                    return;
                }
            }
            if let Some(moving) = self.moving.as_mut() {
                if response.dragged() {
                    let doc = self.docs.get_mut(self.current).unwrap();
                    let s = doc.view.to_sheet(pointer);
                    let now = [s[0] as f64, s[1] as f64];
                    let frame = doc.frame();
                    let (a, b) = (frame.to_pdf(moving.last), frame.to_pdf(now));
                    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                    if dx != 0.0 || dy != 0.0 {
                        let picked: Vec<usize> = match moving.only {
                            Some(one) => vec![one],
                            None => doc.selection(),
                        }
                        .into_iter()
                        .filter(|i| !locked(&doc.marks[*i].markup))
                        .collect();
                        if !picked.is_empty() {
                            if !moving.moved {
                                doc.checkpoint_named(if picked.len() == 1 { "Move" } else { "Move markups" });
                                moving.moved = true;
                            }
                            for i in picked {
                                if moving.only.is_some() {
                                    crate::grips::move_box_only(&mut doc.marks[i].markup, dx, dy);
                                } else {
                                    doc.marks[i].markup.move_by(dx, dy);
                                }
                                doc.marks[i].changed = true;
                            }
                            doc.dirty = true;
                        }
                        moving.last = now;
                    }
                    return;
                }
                if response.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
                    let moved = moving.moved;
                    self.moving = None;
                    if moved {
                        self.status = "Moved. Undo puts it back; the arrow keys nudge it.".into();
                        self.save_soon();
                    }
                    return;
                }
            }
            if response.drag_started() && self.touch.down && crate::platform::tablet() {
                // A finger on bare paper moves the sheet. Picking several
                // markups at once is the Lasso tool's job on a tablet.
                self.touch.panning = true;
                return;
            }
            if response.drag_started() {
                self.lasso = Some(vec![at]);
            } else if response.dragged() {
                if let Some(loop_) = self.lasso.as_mut() {
                    if loop_.len() == 1 {
                        loop_.push(at);
                    } else {
                        let last = loop_.len() - 1;
                        loop_[last] = at;
                    }
                }
            } else if response.drag_stopped() {
                self.close_lasso(true);
            }
            return;
        }

        if tool == Tool::CloudPlus {
            self.cloud_plus_input(response, at);
            return;
        }

        if tool == Tool::Signature {
            if response.clicked() {
                self.place_signature(at);
            }
            return;
        }

        if tool == Tool::Lasso {
            if response.drag_started() {
                self.lasso = Some(vec![at]);
            } else if response.dragged() {
                if let Some(loop_) = self.lasso.as_mut() {
                    let far = loop_
                        .last()
                        .map(|p| (p[0] - at[0]).abs() + (p[1] - at[1]).abs() > 2.0)
                        .unwrap_or(true);
                    if far {
                        loop_.push(at);
                    }
                }
            } else if response.drag_stopped() {
                self.close_lasso(false);
            }
            return;
        }

        match tool.shaping() {
            crate::app::Shaping::None => return,
            crate::app::Shaping::MultiClick => {
                let finish = response.double_clicked()
                    || response.secondary_clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Enter));
                if finish {
                    self.finish_draft();
                    return;
                }
                // A plate is a rectangle, and a rectangle took four clicks.
                //
                // These tools collect corners one at a time, which is right
                // for an irregular shape and tedious for the commonest shape
                // there is. So they drag as well: press at one corner, let go
                // at the opposite one. Only for the shapes that close back on
                // themselves -- dragging out a polyline means nothing.
                //
                // A drag only starts one when nothing is part-drawn already,
                // so somebody halfway round a footing who moves the mouse with
                // the button down does not have their work replaced by a box.
                if tool.is_closed() {
                    let part_drawn = self.doc().map(|d| d.draft.is_some()).unwrap_or(false);
                    if response.drag_started() && !part_drawn {
                        self.dragged_box = Some(at);
                        self.start_draft();
                        self.set_box(at, at);
                        return;
                    }
                    if let Some(from) = self.dragged_box {
                        if response.dragged() {
                            self.set_box(from, at);
                            return;
                        }
                        if response.drag_stopped() {
                            self.dragged_box = None;
                            // A press that barely moved is somebody putting
                            // down a first corner, not drawing a box. Leave
                            // the one point and let them carry on clicking.
                            if a_real_drag(from, at, self.zoom_now()) {
                                self.set_box(from, at);
                                self.finish_draft();
                            } else {
                                self.set_points(vec![at]);
                            }
                            return;
                        }
                    }
                }
                if response.clicked() {
                    self.push_point(at);
                    // A callout is two clicks, what it is about and where
                    // the words go, and then the words: the way Revu's is.
                    let two = self
                        .doc()
                        .and_then(|d| d.draft.as_ref())
                        .is_some_and(|d| d.points.len() >= 2);
                    if tool == Tool::Callout && two {
                        self.finish_draft();
                    }
                }
                return;
            }
            // Three clicks and it finishes itself, because a circle is
            // decided by its third point and waiting for a double click
            // after it only invites a fourth.
            crate::app::Shaping::ThreePoint => {
                if response.clicked() {
                    self.push_point(at);
                    let enough = self
                        .doc()
                        .and_then(|d| d.draft.as_ref())
                        .map(|d| d.points.len() >= 3)
                        .unwrap_or(false);
                    if enough {
                        self.finish_draft();
                    }
                }
                return;
            }
            _ => {}
        }

        match tool {
            Tool::Count => {
                if response.clicked() {
                    self.start_draft();
                    self.push_point(at);
                    self.finish_draft();
                }
            }
            Tool::Note | Tool::Flag | Tool::Stamp | Tool::FileAttachment => {
                if response.clicked() {
                    self.start_draft();
                    self.push_point(at);
                    self.finish_draft();
                    let doc = self.doc_mut().unwrap();
                    let last = doc.marks.len().checked_sub(1);
                    if tool == Tool::Note {
                        self.editing_text = last;
                    }
                }
            }
            // Dragged out, a text box is the size it was dragged and its
            // words wrap inside it. Put down with a click, it starts small and
            // grows with the words, the way Revu's does. Either way the words
            // are typed straight away, on the sheet, where they will be.
            Tool::Text | Tool::Typewriter => {
                if response.drag_started() {
                    self.dragged_box = Some(at);
                    self.start_draft();
                    self.set_points(vec![at, at]);
                } else if response.dragged() && self.dragged_box.is_some() {
                    if let Some(from) = self.dragged_box {
                        self.set_points(vec![from, at]);
                    }
                } else if response.drag_stopped() && self.dragged_box.is_some() {
                    let from = self.dragged_box.take().unwrap_or(at);
                    if a_real_drag(from, at, self.zoom_now()) {
                        self.set_points(vec![from, at]);
                        self.finish_draft();
                        let doc = self.doc_mut().unwrap();
                        self.editing_text = doc.marks.len().checked_sub(1);
                        self.typing_fit = None;
                    } else {
                        if let Some(doc) = self.doc_mut() {
                            doc.draft = None;
                        }
                        self.put_text_box(from);
                    }
                } else if response.clicked() {
                    self.put_text_box(at);
                }
            }
            Tool::Eraser => {
                if response.dragged() || response.drag_started() {
                    self.rub_out(at);
                }
            }
            Tool::Ink => {
                if response.drag_started() {
                    self.start_draft();
                    if let Some(draft) = self.doc_mut().unwrap().draft.as_mut() {
                        draft.strokes.push(vec![at]);
                    }
                } else if response.dragged() {
                    let far = {
                        let doc = self.doc().unwrap();
                        doc.draft
                            .as_ref()
                            .and_then(|d| d.strokes.last())
                            .and_then(|s| s.last())
                            .map(|last| {
                                let d = (last[0] - at[0]).powi(2) + (last[1] - at[1]).powi(2);
                                d > (2.0 / doc.view.zoom as f64).powi(2)
                            })
                            .unwrap_or(true)
                    };
                    if far {
                        if let Some(draft) = self.doc_mut().unwrap().draft.as_mut() {
                            if let Some(stroke) = draft.strokes.last_mut() {
                                stroke.push(at);
                            }
                        }
                    }
                } else if response.drag_stopped() {
                    self.finish_draft();
                }
            }
            _ if tool.shaping() == crate::app::Shaping::Drag => {
                if response.drag_started() {
                    self.start_draft();
                    self.push_point(at);
                    self.push_point(at);
                } else if response.dragged() {
                    if let Some(draft) = self.doc_mut().unwrap().draft.as_mut() {
                        if draft.points.len() == 2 {
                            draft.points[1] = at;
                        }
                    }
                } else if response.drag_stopped() {
                    // A few tools need something more than a rectangle before
                    // they are a markup: a picture, an address, a name. They
                    // ask for it now, while the person still has in mind what
                    // they just dragged out.
                    match tool {
                        Tool::Image => self.ask_for_picture(),
                        Tool::Snapshot => self.take_snapshot(),
                        // A form field needs a name nothing else is using:
                        // two fields with the same name are, in a PDF, the
                        // same field shown twice.
                        _ if tool.is_form() => {
                            let field = crate::forms::Field::of_tool(tool);
                            let name = field.map(|field| {
                                let taken = self
                                    .doc()
                                    .map(|d| {
                                        let mut taken =
                                            crate::forms::names_taken(&d.file);
                                        for mark in d.marks.iter().filter(|m| !m.gone) {
                                            if let Some(name) =
                                                mark.markup.dict.get("T").and_then(|o| o.as_text())
                                            {
                                                taken.insert(name);
                                            }
                                        }
                                        taken
                                    })
                                    .unwrap_or_default();
                                crate::forms::free_name(field, &taken)
                            });
                            if let (Some(name), Some(draft)) =
                                (name, self.doc_mut().and_then(|d| d.draft.as_mut()))
                            {
                                draft.says = name;
                            }
                            self.finish_draft();
                            let last = self.doc().map(|d| d.marks.len().saturating_sub(1));
                            if let Some(at) = last {
                                self.editing_field = Some(at);
                            }
                        }
                        Tool::Hyperlink => self.ask_for_address(),
                        Tool::Redaction => {
                            self.finish_draft();
                            self.status = "Marked for redaction. Nothing comes out of the \
                                           drawing until you apply the redactions."
                                .into();
                        }
                        _ => self.finish_draft(),
                    }
                }
            }
            _ => {}
        }
    }

    /// Takes hold of everything the loop went round.
    ///
    /// `as_box` is the Select tool's drag, which is a rectangle between the
    /// two corners rather than a traced loop. Either way a markup has to be
    /// wholly inside to be taken: half a beam is not a beam, and taking it
    /// because the loop clipped its end is how somebody moves something they
    /// did not mean to touch.
    pub fn close_lasso(&mut self, as_box: bool) {
        let Some(loop_) = self.lasso.take() else { return };
        if loop_.len() < 2 {
            return;
        }
        let ring: Vec<[f64; 2]> = if as_box {
            let (a, b) = (loop_[0], loop_[loop_.len() - 1]);
            if (a[0] - b[0]).abs() < 3.0 && (a[1] - b[1]).abs() < 3.0 {
                return;
            }
            vec![[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]]
        } else {
            loop_
        };

        let Some(doc) = self.doc_mut() else { return };
        let frame = doc.frame();
        let page = doc.page;
        let caught: Vec<usize> = doc
            .marks
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.gone && m.page == page)
            .filter(|(_, m)| {
                let points = m.on_sheet(&frame);
                !points.is_empty() && points.iter().all(|p| inside(&ring, *p))
            })
            .map(|(i, _)| i)
            .collect();
        let count = caught.len();
        doc.selected = caught.first().copied();
        doc.also = caught.into_iter().skip(1).collect();
        self.status = if count == 0 {
            "Nothing was inside the loop.".into()
        } else {
            format!(
                "{count} markup{} taken hold of.",
                if count == 1 { "" } else { "s" }
            )
        };
    }

    /// Asks for the picture an image markup is about to show, then places it.
    fn ask_for_picture(&mut self) {
        self.filing.answer(
            crate::files::Choose::open()
                .filter("Pictures", &["png", "jpg", "jpeg"])
                .title("The picture to put on the sheet"),
            |app, mut chosen| app.place_picture(chosen.pop()),
        );
    }

    fn place_picture(&mut self, chosen: Option<std::path::PathBuf>) {
        let Some(path) = chosen else {
            // Nothing chosen means nothing drawn: an empty box would be a
            // markup nobody asked for.
            if let Some(doc) = self.doc_mut() {
                doc.draft = None;
            }
            return;
        };
        match crate::picture::read(&path) {
            Ok(picture) => {
                if let Some(draft) = self.doc_mut().and_then(|d| d.draft.as_mut()) {
                    draft.picture = Some(picture);
                }
                self.finish_draft();
            }
            Err(why) => {
                if let Some(doc) = self.doc_mut() {
                    doc.draft = None;
                }
                self.error = Some(why);
            }
        }
    }

    /// Asks where a hyperlink goes before putting it down.
    fn ask_for_address(&mut self) {
        let box_drawn = self
            .doc()
            .and_then(|d| d.draft.as_ref())
            .map(|d| d.points.len() >= 2)
            .unwrap_or(false);
        if !box_drawn {
            if let Some(doc) = self.doc_mut() {
                doc.draft = None;
            }
            return;
        }
        self.asking_address = Some(String::new());
    }

    /// Rubs out freehand strokes under the pointer, and nothing else.
    ///
    /// An eraser that took anything it touched would be a way to lose a
    /// measurement by leaning on the mouse. This one only ever removes ink,
    /// which is the only thing anybody expects to rub out.
    fn rub_out(&mut self, at: [f64; 2]) {
        let Some(doc) = self.doc_mut() else { return };
        let page = doc.page;
        let frame = doc.frame();
        let reach = (10.0 / doc.view.zoom as f64).powi(2);
        let hit: Vec<usize> = doc
            .marks
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                !m.gone
                    && m.page == page
                    && m.markup.subtype() == annot::Subtype::Ink
                    && nearest(&m.on_sheet(&frame), at) <= reach
            })
            .map(|(i, _)| i)
            .collect();
        if hit.is_empty() {
            return;
        }
        doc.checkpoint();
        for i in hit {
            if let Some(mark) = doc.marks.get_mut(i) {
                mark.gone = true;
            }
        }
        doc.dirty = true;
        doc.selected = None;
    }

    /// The four corners of the box between two opposite ones, going round it
    /// rather than crossing over.
    /// A text box put down with a click: small, at the click, and made to
    /// grow with its words as they are typed.
    fn put_text_box(&mut self, at: [f64; 2]) {
        self.start_draft();
        self.set_points(vec![at, [at[0] + 72.0, at[1] + 20.0]]);
        self.finish_draft();
        let typed = self.doc().and_then(|d| d.marks.len().checked_sub(1));
        self.editing_text = typed;
        self.typing_fit = typed;
    }

    fn set_box(&mut self, from: [f64; 2], to: [f64; 2]) {
        self.set_points(vec![from, [to[0], from[1]], to, [from[0], to[1]]]);
    }

    fn set_points(&mut self, points: Vec<[f64; 2]>) {
        if let Some(doc) = self.doc_mut() {
            if let Some(draft) = doc.draft.as_mut() {
                draft.points = points;
            }
        }
    }

    /// The zoom on the drawing being worked on, so a drag is judged in the
    /// sheet's own units rather than in screen pixels.
    fn zoom_now(&self) -> f64 {
        self.doc().map(|d| d.view.zoom as f64).unwrap_or(1.0)
    }

    fn start_draft(&mut self) {
        let tool = self.tool;
        let subject = self.subject.clone();
        let colour = self.color;
        let template = self.template.clone();
        let pen = Some(self.pen.clone());
        if let Some(doc) = self.doc_mut() {
            if doc.draft.is_none() {
                doc.draft = Some(crate::sheet::Draft {
                    tool,
                    points: Vec::new(),
                    strokes: Vec::new(),
                    subject,
                    colour,
                    template,
                    pen,
                    ..crate::sheet::Draft::default()
                });
            }
        }
    }

    fn push_point(&mut self, at: [f64; 2]) {
        self.start_draft();
        if let Some(draft) = self.doc_mut().unwrap().draft.as_mut() {
            draft.points.push(at);
        }
    }

    /// Ends the markup being drawn and puts it on the sheet.
    pub fn finish_draft(&mut self) {
        let tool = self.tool;
        let Some(doc) = self.doc_mut() else { return };
        let Some(draft) = doc.draft.take() else { return };

        if tool == Tool::Calibrate {
            if draft.points.len() >= 2 {
                let a = draft.points[0];
                let b = draft.points[draft.points.len() - 1];
                let span = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                self.calibrating = Some(Calibrating {
                    page: doc.page,
                    points: span,
                    typed: String::new(),
                    everywhere: false,
                    error: None,
                });
            }
            return;
        }

        let enough = match tool {
            Tool::Count | Tool::Note | Tool::Flag | Tool::Stamp | Tool::FileAttachment => {
                !draft.points.is_empty()
            }
            Tool::Ink => draft.strokes.iter().any(|s| !s.is_empty()),
            Tool::Arc | Tool::Diameter | Tool::Radius | Tool::Angle => draft.points.len() >= 3,
            _ if tool.is_closed() => draft.points.len() >= 3,
            _ => draft.points.len() >= 2,
        };
        if !enough {
            return;
        }

        // A count is one symbol per click, drawn where the click landed.
        let draft = if tool == Tool::Count {
            let at = draft.points[0];
            let mut ring = Vec::with_capacity(24);
            for k in 0..24 {
                let a = k as f64 / 24.0 * std::f64::consts::TAU;
                ring.push([at[0] + 9.0 * a.cos(), at[1] + 9.0 * a.sin()]);
            }
            crate::sheet::Draft {
                points: ring,
                ..draft
            }
        } else {
            draft
        };

        let frame = doc.frame();
        let scale = doc.scale();
        let Some(markup) = draft.into_markup(&frame, scale.as_ref()) else {
            return;
        };
        doc.checkpoint_named(&format!("Draw {}", tool.label()));
        doc.marks.push(crate::sheet::Mark::new(doc.page, markup));
        let at = doc.marks.len() - 1;
        doc.choose(Some(at));

        // A callout is for its words: they are typed straight into its box,
        // and the box grows with them.
        if tool == Tool::Callout {
            self.editing_text = Some(at);
            self.typing_fit = Some(at);
        }

        // A volume is an area with a depth, and without the depth it is not a
        // volume. Asking now, while the shape they just drew is in front of
        // them, beats letting it sit there measuring nothing.
        if tool == Tool::Volume {
            self.asking_depth = Some(crate::app::AskingDepth {
                markup: at,
                typed: String::new(),
                error: None,
            });
        }
    }

    /// Asks the worker for everything on screen, in both panes when there are
    /// two.
    ///
    /// One list for the whole window rather than one per pane: the worker keeps
    /// the most recent list and drops the rest, so two panes each sending their
    /// own would take turns cancelling each other and neither would finish.
    fn request_tiles(&mut self, area: Rect) {
        let Some(doc) = self.doc() else { return };
        let (page, view) = (doc.page, doc.view.clone());
        let other = doc.other.as_ref().map(|o| (o.page, o.view.clone()));
        let split = doc.split;
        let side = doc.side;

        let (first, second) = halves(self.last_canvas_whole, split);
        let (mine, theirs) = match (side, second) {
            (0, o) => (first, o),
            (_, Some(o)) => (o, Some(first)),
            (_, None) => (first, None),
        };
        let _ = area;

        // The pane being worked in comes first, so its tiles are drawn first.
        let mut keys = self.tiles_for(mine, page, &view);
        // Then the sheets around it, in whatever arrangement is in use.
        for one in self.sheets_showing(mine) {
            if one.page == page {
                continue;
            }
            keys.extend(self.tiles_for(mine, one.page, &view.shifted(one.at)));
        }
        if let (Some(theirs), Some((other_page, other_view))) = (theirs, other) {
            keys.extend(self.tiles_for(theirs, other_page, &other_view));
        }
        keys.truncate(240);
        self.missing_tiles = keys.len();

        // Once the screen is sharp, the sheets either side of this one are
        // drawn ahead, at the same zoom and position. Flipping between sheets
        // of a set holds the zoom, so the next sheet is then already there:
        // a flip is instant instead of a third of a second of blur.
        let mut ahead: Vec<u32> = Vec::new();
        if keys.is_empty() && !*NO_PREFETCH {
            if let Some(doc) = self.doc() {
                let here = doc.pages.get(page as usize).copied();
                for step in [1i64, -1, 2] {
                    let next = page as i64 + step;
                    if next < 0 || next as usize >= doc.pages.len() {
                        continue;
                    }
                    let next = next as u32;
                    let same = here.zip(doc.pages.get(next as usize).copied()).is_some_and(|(a, b)| {
                        (a.width - b.width).abs() < 0.5 && (a.height - b.height).abs() < 0.5
                    });
                    if same {
                        ahead.push(next);
                    }
                }
            }
            for next in &ahead {
                let more = self.tiles_for(mine, *next, &view);
                keys.extend(more);
                if keys.len() >= 96 {
                    break;
                }
            }
            keys.truncate(96);
        }

        // The line-work of the sheet in hand, for snapping, once the screen is
        // sharp so it never holds up the drawing itself.
        let wants_lines = self.prefs.snap_to_content && self.missing_tiles == 0;
        let Some(doc) = self.doc_mut() else { return };
        let id = doc.id;
        if wants_lines && !doc.geometry_asked.contains(&page) {
            doc.geometry_asked.insert(page);
            self.svc.send(render::ToWorker::Geometry { doc: id, page });
        }
        let Some(doc) = self.doc_mut() else { return };
        // Their low-resolution pictures too, so even a sheet not yet drawn
        // sharp is never a blank page. The worker ignores a second ask for
        // one it is already making.
        let previews: Vec<u32> =
            ahead.iter().copied().filter(|p| !doc.previews.contains_key(p)).collect();
        for p in previews {
            self.svc.send(render::ToWorker::Preview { doc: id, page: p });
        }
        let Some(doc) = self.doc_mut() else { return };
        if keys != doc.sent {
            doc.sent = keys.clone();
            self.svc.send(render::ToWorker::Want { tiles: keys });
        }
    }

    /// The tiles one pane needs, nearest the middle of it first.
    fn tiles_for(&self, area: Rect, page: u32, view: &View) -> Vec<TileKey> {
        let Some(doc) = self.doc() else {
            return Vec::new();
        };
        let id = doc.id;
        let size = doc
            .pages
            .get(page as usize)
            .copied()
            .unwrap_or(PageSize {
                width: 612.0,
                height: 792.0,
            });
        let bucket =
            render::bucket_for_zoom(view.zoom).clamp(render::MIN_BUCKET, render::MAX_BUCKET);
        let scale = render::bucket_scale(bucket);

        // Ask for a little beyond the edges of the window so a pan lands on
        // tiles that are already there.
        let reach = 220.0;
        let top_left = view.to_sheet(pos2(area.left() - reach, area.top() - reach));
        let bottom_right = view.to_sheet(pos2(area.right() + reach, area.bottom() + reach));

        let raster = |v: f32| (v as f64 * scale / TILE as f64).floor() as i64;
        let across = ((size.width as f64 * scale) / TILE as f64).ceil() as i64;
        let down = ((size.height as f64 * scale) / TILE as f64).ceil() as i64;
        let x0 = raster(top_left[0]).max(0);
        let y0 = raster(top_left[1]).max(0);
        let x1 = raster(bottom_right[0]).min(across - 1);
        let y1 = raster(bottom_right[1]).min(down - 1);

        let centre = view.to_sheet(area.center());
        let mut wanted: Vec<(TileKey, f32)> = Vec::new();
        for ty in y0..=y1.max(y0 - 1) {
            for tx in x0..=x1.max(x0 - 1) {
                if tx < 0 || ty < 0 {
                    continue;
                }
                let key = TileKey {
                    doc: id,
                    page,
                    bucket,
                    tx: tx as u32,
                    ty: ty as u32,
                };
                if doc.tiles.fresh(&key) {
                    continue;
                }
                let cx = (tx as f64 + 0.5) * TILE as f64 / scale;
                let cy = (ty as f64 + 0.5) * TILE as f64 / scale;
                let d = (cx - centre[0] as f64).powi(2) + (cy - centre[1] as f64).powi(2);
                wanted.push((key, d as f32));
            }
        }
        wanted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        wanted.truncate(160);
        wanted.into_iter().map(|(k, _)| k).collect()
    }

    fn paint_sheet(&mut self, painter: &egui::Painter, area: Rect, size: PageSize) {
        let theme = self.chrome.theme;
        let placed = self.sheets_showing(area);
        let doc = self.doc_mut().unwrap();
        let view = doc.view.clone();
        let page = doc.page;
        // The neighbours first, so the sheet being worked on is painted over
        // them and its markups sit on top.
        for one in &placed {
            if one.page == page {
                continue;
            }
            let Some(size) = doc.pages.get(one.page as usize).copied() else {
                continue;
            };
            let shifted = view.shifted(one.at);
            paint_one(painter, doc, &shifted, one.page, area, size, theme);
        }
        paint_one(painter, doc, &view, page, area, size, theme);
    }

    /// Which sheets are on the screen, and where, for the arrangement in use.
    pub fn sheets_showing(&self, area: Rect) -> Vec<crate::layout::Placed> {
        let Some(doc) = self.doc() else {
            return Vec::new();
        };
        let sizes: Vec<(f64, f64)> = doc
            .pages
            .iter()
            .map(|p| (p.width as f64, p.height as f64))
            .collect();
        // How far up and down to bother laying sheets out: what the window
        // covers in the sheet's own points, with a little to spare so a scroll
        // lands on something already drawn.
        let reach = (area.height() as f64 / doc.view.zoom.max(0.01) as f64) + 400.0;
        crate::layout::placed(doc.layout, doc.page, &sizes, reach)
    }

    /// In a scrolling arrangement, makes whichever sheet is mostly in the
    /// window the one being worked on.
    ///
    /// Without this somebody scrolls down to the next sheet and marks up the
    /// one they have just scrolled away from.
    fn follow_the_scroll(&mut self, area: Rect) {
        let scrolls = self.doc().map(|d| d.layout.scrolls()).unwrap_or(false);
        if !scrolls {
            return;
        }
        let placed = self.sheets_showing(area);
        let Some(doc) = self.doc() else { return };
        let sizes: Vec<(f64, f64)> = doc
            .pages
            .iter()
            .map(|p| (p.width as f64, p.height as f64))
            .collect();
        let top_left = doc.view.to_sheet(area.left_top());
        let bottom_right = doc.view.to_sheet(area.right_bottom());
        let window = [
            top_left[0] as f64,
            top_left[1] as f64,
            bottom_right[0] as f64,
            bottom_right[1] as f64,
        ];
        let Some(now) = crate::layout::mostly_showing(&placed, &sizes, window) else {
            return;
        };
        if now == doc.page {
            return;
        }
        // Re-base: the new sheet becomes the origin, and the view moves by
        // exactly as much, so nothing on the screen appears to jump.
        let Some(shift) = placed.iter().find(|p| p.page == now).map(|p| p.at) else {
            return;
        };
        let zoom = doc.view.zoom;
        let Some(doc) = self.doc_mut() else { return };
        doc.page = now;
        doc.view.offset.x += shift[0] as f32 * zoom;
        doc.view.offset.y += shift[1] as f32 * zoom;
        doc.choose(None);
    }

    /// Draws the grid over the sheet.
    ///
    /// Over the drawing rather than under it, but faint, because a grid under
    /// a plotted sheet is invisible and a grid drawn solidly over one hides
    /// what somebody is trying to measure.
    fn paint_grid(&mut self, painter: &egui::Painter, area: Rect, size: PageSize) {
        if !self.prefs.grid {
            return;
        }
        let spacing = self.grid_spacing();
        let Some(doc) = self.doc() else { return };
        let view = &doc.view;
        // Only the part of the sheet on the screen, clipped to the paper: a
        // grid running off across the surround belongs to nothing.
        let top_left = view.to_sheet(area.left_top());
        let bottom_right = view.to_sheet(area.right_bottom());
        let on_sheet = [
            (top_left[0] as f64).max(0.0),
            (top_left[1] as f64).max(0.0),
            (bottom_right[0] as f64).min(size.width as f64),
            (bottom_right[1] as f64).min(size.height as f64),
        ];
        if on_sheet[2] <= on_sheet[0] || on_sheet[3] <= on_sheet[1] {
            return;
        }
        let (xs, ys) = crate::places::grid_lines(on_sheet, spacing, 400);
        if xs.is_empty() && ys.is_empty() {
            return;
        }
        let colour = Color32::from_rgba_unmultiplied(70, 110, 170, 60);
        let stroke = Stroke::new(1.0_f32, colour); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
        for x in xs {
            let a = view.to_screen([x as f32, on_sheet[1] as f32]);
            let b = view.to_screen([x as f32, on_sheet[3] as f32]);
            painter.line_segment([a, b], stroke);
        }
        for y in ys {
            let a = view.to_screen([on_sheet[0] as f32, y as f32]);
            let b = view.to_screen([on_sheet[2] as f32, y as f32]);
            painter.line_segment([a, b], stroke);
        }
    }

    fn paint_markups(&mut self, painter: &egui::Painter, _area: Rect) {
        let lasso = self.lasso.clone();
        let doc = self.doc().unwrap();
        let frame = doc.frame();
        let page = doc.page;
        let held = doc.selection();
        let every_label = self.prefs.markup_text;
        for (index, mark) in doc.on_page(page) {
            let selected = held.contains(&index);
            paint_shape(
                painter,
                &doc.view,
                &mark.on_sheet(&frame),
                &Look::of(&mark.markup, selected).labelled(every_label || selected),
            );
            paint_leader(painter, &doc.view, &frame, &mark.markup);
        }
        // The grips on what is selected, so it can be taken hold of.
        if let Some(handles) = self.grips_showing() {
            let blue = Color32::from_rgb(90, 170, 255);
            for (_, handle) in handles {
                let at = doc.view.to_screen([handle.at[0] as f32, handle.at[1] as f32]);
                match handle.grip {
                    crate::grips::Grip::Side(_) => {
                        let r = Rect::from_center_size(at, egui::vec2(8.0, 8.0));
                        painter.rect_filled(r, 1.0, Color32::WHITE);
                        painter.rect_stroke(r, 1.0, Stroke::new(1.5_f32, blue), egui::StrokeKind::Middle); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                    }
                    crate::grips::Grip::Point(_) => {
                        painter.circle_filled(at, 4.5, Color32::WHITE);
                        painter.circle_stroke(at, 4.5, Stroke::new(1.5_f32, blue)); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                    }
                    crate::grips::Grip::Leader(_) => {
                        painter.circle_filled(at, 4.5, Color32::from_rgb(255, 196, 64));
                        painter.circle_stroke(at, 4.5, Stroke::new(1.5_f32, Color32::from_gray(40))); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                    }
                }
            }
        }
        // The corners of whatever is selected, while a point mode is in hand,
        // so there is something to aim at.
        if self.points_mode.is_some() {
            if let Some(mark) = doc.selected.and_then(|i| doc.marks.get(i)) {
                for point in mark.on_sheet(&frame) {
                    let at = doc.view.to_screen([point[0] as f32, point[1] as f32]);
                    painter.circle_filled(at, 4.0, Color32::from_rgb(90, 170, 255));
                    painter.circle_stroke(at, 4.0, Stroke::new(1.0_f32, Color32::WHITE)); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                }
            }
        }

        // Form fields, while the form editor is on: outlined and named, so a
        // sheet of twenty boxes can be gone through.
        if self.form_editor {
            for (_, mark) in doc.on_page(page) {
                if !crate::forms::is_field(&mark.markup) {
                    continue;
                }
                let corners = mark.on_sheet(&frame);
                if corners.is_empty() {
                    continue;
                }
                let screen: Vec<Pos2> = corners
                    .iter()
                    .map(|p| doc.view.to_screen([p[0] as f32, p[1] as f32]))
                    .collect();
                let rect = bounding(&screen);
                painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(90, 170, 255, 40));
                painter.rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(1.0_f32, Color32::from_rgb(90, 170, 255)), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                    egui::StrokeKind::Middle,
                );
                let name = mark
                    .markup
                    .dict
                    .get("T")
                    .and_then(|o| o.as_text())
                    .unwrap_or_default();
                let kind = crate::forms::field_of(&mark.markup)
                    .map(|f| f.name())
                    .unwrap_or("Field");
                painter.text(
                    rect.left_top() + egui::vec2(3.0, -14.0),
                    egui::Align2::LEFT_TOP,
                    format!("{name} · {kind}"),
                    FontId::proportional(10.0),
                    Color32::from_rgb(90, 170, 255),
                );
            }
        }

        // The neighbouring sheets' markups, so a continuous view shows the
        // whole drawing set rather than bare paper either side.
        for one in &self.showing {
            if one.page == page {
                continue;
            }
            let shifted = doc.view.shifted(one.at);
            let frame = doc.frame_of(one.page);
            for (_, mark) in doc.on_page(one.page) {
                paint_shape(
                    painter,
                    &shifted,
                    &mark.on_sheet(&frame),
                    &Look::of(&mark.markup, false).labelled(every_label),
                );
            }
        }

        // The loop being dragged, while it is being dragged.
        if let Some(loop_) = lasso.as_ref().filter(|l| l.len() >= 2) {
            let as_box = self.tool == Tool::Select;
            let ring: Vec<[f64; 2]> = if as_box {
                let (a, b) = (loop_[0], loop_[loop_.len() - 1]);
                vec![[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]]
            } else {
                loop_.clone()
            };
            let screen: Vec<Pos2> = ring
                .iter()
                .map(|p| doc.view.to_screen([p[0] as f32, p[1] as f32]))
                .collect();
            painter.add(egui::Shape::closed_line(
                screen,
                Stroke::new(1.5_f32, Color32::from_rgb(90, 170, 255)), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
            ));
        }
        if let Some(draft) = doc.draft.as_ref() {
            let colour = Color32::from_rgb(draft.colour[0], draft.colour[1], draft.colour[2]);
            let (subtype, _) = draft
                .tool
                .shape()
                .unwrap_or((annot::Subtype::Line, None));
            let points: Vec<[f64; 2]> = if draft.strokes.is_empty() {
                draft.points.clone()
            } else {
                draft.strokes.iter().flatten().copied().collect()
            };
            paint_shape(
                painter,
                &doc.view,
                &points,
                &Look {
                    subtype,
                    colour,
                    interior: None,
                    width: 2.0,
                    caption: String::new(),
                    selected: true,
                    cloud: draft.tool.is_cloudy().then_some(9.0),
                    picture: false,
                },
            );
            // A callout waiting for where its words go: the leader runs from
            // what it points at to the pointer, with the box on the end of it,
            // out from the pointer and away from the tip, as it will be put.
            if draft.tool == Tool::Callout && draft.points.len() == 1 {
                if let Some(pointer) = painter.ctx().input(|i| i.pointer.hover_pos()) {
                    let tip = doc.view.to_screen([draft.points[0][0] as f32, draft.points[0][1] as f32]);
                    let stroke = Stroke::new(1.5_f32, colour); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                    painter.line_segment([tip, pointer], stroke);
                    let (w, h) = (96.0 * doc.view.zoom, 20.0 * doc.view.zoom);
                    let x0 = if pointer.x >= tip.x { pointer.x } else { pointer.x - w };
                    painter.rect_stroke(
                        Rect::from_min_size(pos2(x0, pointer.y - h * 0.5), vec2(w, h)),
                        0.0,
                        stroke,
                        egui::StrokeKind::Middle,
                    );
                }
            }
            // A Cloud+ waiting for where its words go: the leader and the box
            // follow the pointer until the click.
            if self.cloud_waiting && draft.points.len() == 2 {
                if let Some(pointer) = painter.ctx().input(|i| i.pointer.hover_pos()) {
                    let s = doc.view.to_sheet(pointer);
                    let at = [s[0] as f64, s[1] as f64];
                    let (a, b) = (draft.points[0], draft.points[1]);
                    let cloud = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
                    let middle = [(cloud[0] + cloud[2]) * 0.5, (cloud[1] + cloud[3]) * 0.5];
                    // Sheet space runs down, so "above" is a smaller y.
                    let (x0, x1) = if at[0] < middle[0] { (at[0] - 200.0, at[0]) } else { (at[0], at[0] + 200.0) };
                    let (y0, y1) = if at[1] < middle[1] { (at[1] - 54.0, at[1]) } else { (at[1], at[1] + 54.0) };
                    let words = [x0, y0, x1, y1];
                    let line = crate::grips::leader_between(cloud, words);
                    let screen = |p: [f64; 2]| doc.view.to_screen([p[0] as f32, p[1] as f32]);
                    let stroke = Stroke::new(1.5_f32, colour); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                    painter.line_segment([screen(line[0]), screen(line[1])], stroke);
                    painter.rect_stroke(
                        Rect::from_two_pos(screen([words[0], words[1]]), screen([words[2], words[3]])),
                        0.0,
                        stroke,
                        egui::StrokeKind::Middle,
                    );
                }
            }
        }
    }

    fn paint_readout(&mut self, ui: &mut egui::Ui, painter: &egui::Painter, area: Rect) {
        let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        if !area.contains(pointer) || self.tool == Tool::Pan {
            return;
        }
        let doc = self.doc().unwrap();
        let Some(draft) = doc.draft.as_ref() else {
            return;
        };
        let Some(last) = draft.last() else { return };
        let here = doc.view.to_sheet(pointer);
        let here = [here[0] as f64, here[1] as f64];
        let leg = ((here[0] - last[0]).powi(2) + (here[1] - last[1]).powi(2)).sqrt();
        let so_far = takeoff::geometry::length(&draft.points) + leg;

        let text = match (doc.scale(), self.tool) {
            (Some(scale), Tool::Length | Tool::Calibrate | Tool::Area) => {
                if self.tool == Tool::Area && draft.points.len() >= 2 {
                    let mut closed = draft.points.clone();
                    closed.push(here);
                    format!(
                        "{}   (round {})",
                        scale.area(takeoff::geometry::area(&closed)),
                        scale.length(takeoff::geometry::perimeter(&closed))
                    )
                } else {
                    format!(
                        "{}   (this leg {})",
                        scale.length(so_far),
                        scale.length(leg)
                    )
                }
            }
            (None, Tool::Calibrate) => format!("{:.2} in across the sheet", so_far / 72.0),
            (None, _) => "no scale on this sheet".to_string(),
            _ => return,
        };

        let at = pointer + vec2(16.0, 16.0);
        let galley = painter.layout_no_wrap(text, FontId::proportional(13.0), Color32::WHITE);
        let back = Rect::from_min_size(at, galley.size()).expand(4.0);
        painter.rect_filled(back, 3.0, Color32::from_black_alpha(200));
        painter.galley(at, galley, Color32::WHITE);
    }
}

fn colour_of(markup: &annot::Markup) -> Color32 {
    let c = markup.colour();
    Color32::from_rgb(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
    )
}

fn interior_of(markup: &annot::Markup) -> Option<Color32> {
    // A multiplied fill is drawn properly into the sheet once it is saved,
    // which happens straight away; here it is only a tint, so it never hides
    // the lines the multiply is there to keep.
    let share = if markup.multiplies() { 0.25 } else { 1.0 };
    markup.interior().map(|c| {
        Color32::from_rgba_unmultiplied(
            (c[0] * 255.0) as u8,
            (c[1] * 255.0) as u8,
            (c[2] * 255.0) as u8,
            (markup.fill_opacity() * share * 255.0) as u8,
        )
    })
}

/// Whether a point is inside a closed loop, by the crossing rule.
fn inside(ring: &[[f64; 2]], at: [f64; 2]) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let mut within = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a[1] > at[1]) != (b[1] > at[1]) {
            let span = b[1] - a[1];
            if span.abs() > f64::EPSILON {
                let x = a[0] + (at[1] - a[1]) / span * (b[0] - a[0]);
                if at[0] < x {
                    within = !within;
                }
            }
        }
        j = i;
    }
    within
}

/// Squared distance from a point to the nearest part of a run of points.
/// Whether a markup is locked against being moved (Revu's lock).
pub fn locked(markup: &annot::Markup) -> bool {
    markup.dict.get("BSILock").and_then(|o| o.as_bool()).unwrap_or(false)
}

/// The markup under a point on the sheet: on its outline, or anywhere inside
/// a shape that encloses something — a text box is picked up by its words,
/// not only by its corners. The one on top wins a tie.
pub fn hit_at(doc: &crate::sheet::Doc, frame: &annot::Frame, page: u32, at: [f64; 2], reach: f64) -> Option<usize> {
    use annot::Subtype;
    doc.marks
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.gone && m.page == page)
        .map(|(i, m)| {
            let points = m.on_sheet(frame);
            let closed = matches!(
                m.markup.subtype(),
                Subtype::Square
                    | Subtype::Circle
                    | Subtype::Stamp
                    | Subtype::FreeText
                    | Subtype::Text
                    | Subtype::Link
                    | Subtype::Polygon
                    | Subtype::Highlight
                    | Subtype::Underline
                    | Subtype::StrikeOut
                    | Subtype::Squiggly
            );
            let mut d = nearest(&points, at);
            if closed && points.len() >= 3 {
                let back = [points[points.len() - 1], points[0]];
                d = d.min(nearest(&back, at));
                if inside(&points, at) {
                    d = 0.0;
                }
            }
            (i, d)
        })
        .filter(|(_, d)| *d <= reach)
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal).then(b.0.cmp(&a.0)))
        .map(|(i, _)| i)
}

fn nearest(points: &[[f64; 2]], at: [f64; 2]) -> f64 {
    if points.is_empty() {
        return f64::MAX;
    }
    let mut best = f64::MAX;
    for p in points {
        best = best.min((p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2));
    }
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        let t = if len2 <= f64::EPSILON {
            0.0
        } else {
            (((at[0] - a[0]) * dx + (at[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
        };
        let (px, py) = (a[0] + dx * t, a[1] + dy * t);
        best = best.min((px - at[0]).powi(2) + (py - at[1]).powi(2));
    }
    best
}

/// Everything about how one markup should look, so the canvas does not need a
/// growing list of arguments to draw it.
#[derive(Clone)]
pub struct Look {
    pub subtype: annot::Subtype,
    pub colour: Color32,
    pub interior: Option<Color32>,
    pub width: f64,
    pub caption: String,
    pub selected: bool,
    /// How deep the scallops are, when this is a cloud. What the viewer shows
    /// has to be what the file says, or somebody draws a cloud here and finds
    /// a box when they open it anywhere else.
    pub cloud: Option<f64>,
    /// A picture — a signature, a photograph — is the picture, with no box
    /// drawn round it.
    pub picture: bool,
}

/// What a measurement says on the sheet, with `×6` after it when one markup
/// stands for six. On screen only; the file keeps Revu's own caption.
fn caption_with_quantity(markup: &annot::Markup) -> String {
    let caption = markup.contents();
    if markup.subtype() == annot::Subtype::FreeText || !markup.dict.has(takeoff::row::QUANTITY_KEY) {
        return caption;
    }
    let quantity = takeoff::row::quantity_of(markup);
    if (quantity - 1.0).abs() < 1e-9 {
        return caption;
    }
    let times = format!("×{}", takeoff::row::quantity_text(quantity));
    if caption.is_empty() {
        times
    } else {
        format!("{caption}  {times}")
    }
}

/// The longest label shown over the sheet. A markup whose text is a
/// paragraph — a highlight over a drawing index, a note — is cut short with an
/// ellipsis rather than laid across the sheet; the whole of it is in the
/// markups list and the properties.
const LABEL_LIMIT: usize = 48;

impl Look {
    /// Keeps the label only when it is wanted, and never for a text box,
    /// whose words are the markup itself and are already on the sheet.
    pub fn labelled(mut self, wanted: bool) -> Look {
        if !wanted || self.subtype == annot::Subtype::FreeText {
            self.caption.clear();
            return self;
        }
        let one_line = self.caption.split_whitespace().collect::<Vec<_>>().join(" ");
        self.caption = if one_line.chars().count() > LABEL_LIMIT {
            let cut: String = one_line.chars().take(LABEL_LIMIT - 1).collect();
            format!("{}…", cut.trim_end())
        } else {
            one_line
        };
        self
    }

    pub fn of(markup: &annot::Markup, selected: bool) -> Look {
        Look {
            subtype: markup.subtype(),
            colour: colour_of(markup),
            interior: interior_of(markup),
            width: markup.width(),
            caption: caption_with_quantity(markup),
            selected,
            picture: markup.picture.is_some() || markup.subtype() == annot::Subtype::Stamp,
            cloud: annot::appearance::cloud_radius_for(
                markup,
                markup.bounds().unwrap_or([0.0, 0.0, 0.0, 0.0]),
            ),
        }
    }
}

/// Draws one markup on the canvas. What goes in the file is the appearance
/// stream; this is the live version, drawn from the same geometry.
fn paint_shape(painter: &egui::Painter, view: &View, sheet: &[[f64; 2]], look: &Look) {
    use annot::Subtype as S;
    let Look {
        subtype,
        colour,
        interior,
        width,
        selected,
        ..
    } = *look;
    let caption = look.caption.as_str();
    if sheet.is_empty() {
        return;
    }
    let points: Vec<Pos2> = sheet
        .iter()
        .map(|p| view.to_screen([p[0] as f32, p[1] as f32]))
        .collect();
    let thickness = (width as f32 * view.zoom.clamp(0.4, 2.0)).max(1.5);
    let stroke = Stroke::new(thickness, colour);

    // A cloud is drawn as the scallops themselves, because that is what the
    // file will show everywhere else.
    if let Some(radius) = look.cloud {
        let closed = !matches!(subtype, S::PolyLine | S::Line);
        let edge: Vec<Pos2> = if matches!(subtype, S::Square | S::Circle) {
            let rect = bounding(&points);
            vec![
                rect.left_bottom(),
                rect.right_bottom(),
                rect.right_top(),
                rect.left_top(),
            ]
        } else {
            points.clone()
        };
        let scallops = scalloped(&edge, closed, (radius as f32 * view.zoom).max(2.0));
        if let Some(fill) = interior.filter(|_| closed) {
            fill_shape(painter, &scallops, fill);
        }
        painter.add(if closed {
            egui::Shape::closed_line(scallops, stroke)
        } else {
            egui::Shape::line(scallops, stroke)
        });
        if selected {
            painter.rect_stroke(
                bounding(&points).expand(6.0),
                2.0,
                Stroke::new(1.5_f32, Color32::from_rgb(90, 170, 255)), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                egui::StrokeKind::Outside,
            );
        }
        return;
    }

    match subtype {
        S::Line => {
            if points.len() >= 2 {
                painter.line_segment([points[0], points[points.len() - 1]], stroke);
                for p in [points[0], points[points.len() - 1]] {
                    painter.circle_filled(p, thickness * 1.3, colour);
                }
            }
        }
        S::PolyLine | S::Ink => {
            painter.add(egui::Shape::line(points.clone(), stroke));
        }
        S::Polygon => {
            // Cut into triangles first. An area markup round an L-shaped room
            // or a slab with a notch is concave, and egui fans a path from its
            // first point — which paints the notch as though it were floor.
            if let Some(fill) = interior {
                fill_shape(painter, &points, fill);
            }
            painter.add(egui::Shape::closed_line(points.clone(), stroke));
        }
        S::Square | S::Highlight | S::FreeText | S::Underline | S::StrikeOut | S::Squiggly
        | S::Stamp | S::Link | S::Text => {
            let rect = bounding(&points);
            if let Some(fill) = interior {
                painter.rect_filled(rect, 0.0, fill);
            }
            // A text box with no border has none on the screen either: a
            // Typewriter's words sit on the sheet with nothing round them.
            let borderless = subtype == S::FreeText && width <= 0.0;
            if !look.picture && !borderless {
                painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Middle);
            }
        }
        S::Circle => {
            let rect = bounding(&points);
            let centre = rect.center();
            let (rx, ry) = (rect.width() * 0.5, rect.height() * 0.5);
            let ring: Vec<Pos2> = (0..=48)
                .map(|i| {
                    let a = i as f32 / 48.0 * std::f32::consts::TAU;
                    pos2(centre.x + rx * a.cos(), centre.y + ry * a.sin())
                })
                .collect();
            if let Some(fill) = interior {
                // An ellipse is convex, so this is the easy case, but it goes
                // through the same path so there is only one to be wrong.
                fill_shape(painter, &ring, fill);
            }
            painter.add(egui::Shape::line(ring, stroke));
        }
        _ => {
            painter.add(egui::Shape::line(points.clone(), stroke));
        }
    }

    if !caption.is_empty() {
        let rect = bounding(&points);
        let galley =
            painter.layout_no_wrap(caption.to_string(), FontId::proportional(12.0), Color32::WHITE);
        let at = rect.center() - galley.size() * 0.5;
        painter.rect_filled(
            Rect::from_min_size(at, galley.size()).expand(4.0),
            3.0,
            Color32::from_black_alpha(190),
        );
        painter.galley(at, galley, Color32::WHITE);
    }

    if selected {
        painter.rect_stroke(
            bounding(&points).expand(6.0),
            2.0,
            Stroke::new(1.5_f32, Color32::from_rgb(90, 170, 255)), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
            egui::StrokeKind::Outside,
        );
    }
}

/// A callout's leader, live, so it follows while its tip or its box is being
/// dragged rather than showing where it was at the last save.
fn paint_leader(painter: &egui::Painter, view: &View, frame: &annot::Frame, markup: &annot::Markup) {
    if markup.subtype() != annot::Subtype::FreeText {
        return;
    }
    let line: Vec<[f64; 2]> = markup
        .dict
        .get("CL")
        .map(|o| o.numbers())
        .unwrap_or_default()
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| frame.to_sheet([c[0], c[1]]))
        .collect();
    if line.len() < 2 {
        return;
    }
    let colour = colour_of(markup);
    let width = (markup.width().max(1.0) as f32 * view.zoom.clamp(0.4, 2.0)).max(1.2);
    let points: Vec<Pos2> = line.iter().map(|p| view.to_screen([p[0] as f32, p[1] as f32])).collect();
    painter.add(egui::Shape::line(points.clone(), Stroke::new(width, colour)));
    // The head, pointing at what the callout is about.
    let (tip, from) = (points[0], points[1]);
    let along = (tip - from).normalized();
    if along.is_finite() {
        let reach = (width * 4.0).max(7.0);
        let side = egui::vec2(-along.y, along.x) * reach * 0.45;
        let back = tip - along * reach;
        painter.add(egui::Shape::convex_polygon(vec![tip, back + side, back - side], colour, Stroke::NONE));
    }
}

/// A run of screen points turned into the scalloped edge of a cloud.
///
/// The same construction the appearance stream uses, sampled rather than drawn
/// with curves, because the canvas draws polylines and nobody can tell the
/// difference at the sizes a cloud is looked at.
fn scalloped(edge: &[Pos2], closed: bool, radius: f32) -> Vec<Pos2> {
    if edge.len() < 2 || radius <= 0.0 {
        return edge.to_vec();
    }
    let middle = {
        let n = edge.len() as f32;
        pos2(
            edge.iter().map(|p| p.x).sum::<f32>() / n,
            edge.iter().map(|p| p.y).sum::<f32>() / n,
        )
    };
    let mut out: Vec<Pos2> = Vec::new();
    let count = if closed { edge.len() } else { edge.len() - 1 };
    for i in 0..count {
        let a = edge[i];
        let b = edge[(i + 1) % edge.len()];
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let length = (dx * dx + dy * dy).sqrt();
        if length < 0.5 {
            continue;
        }
        let bumps = ((length / (radius * 2.0)).round() as usize).max(1);
        let along = length / bumps as f32;
        let r = along * 0.5;
        let (ux, uy) = (dx / length, dy / length);
        let (mut nx, mut ny) = (-uy, ux);
        if closed {
            let mid = pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
            if nx * (mid.x - middle.x) + ny * (mid.y - middle.y) < 0.0 {
                nx = -nx;
                ny = -ny;
            }
        }
        for bump in 0..bumps {
            let start = pos2(a.x + ux * along * bump as f32, a.y + uy * along * bump as f32);
            let centre = pos2(start.x + ux * r, start.y + uy * r);
            const STEPS: usize = 7;
            for k in 0..=STEPS {
                let t = std::f32::consts::PI * (1.0 - k as f32 / STEPS as f32);
                // Round the half circle from this end to the next, bulging out.
                let (c, s) = (t.cos(), t.sin());
                out.push(pos2(
                    centre.x - ux * r * c + nx * r * s,
                    centre.y - uy * r * c + ny * r * s,
                ));
            }
        }
    }
    if out.is_empty() {
        edge.to_vec()
    } else {
        out
    }
}

fn bounding(points: &[Pos2]) -> Rect {
    let mut rect = Rect::from_min_max(points[0], points[0]);
    for p in points {
        rect.min.x = rect.min.x.min(p.x);
        rect.min.y = rect.min.y.min(p.y);
        rect.max.x = rect.max.x.max(p.x);
        rect.max.y = rect.max.y.max(p.y);
    }
    rect
}


/// Draws one pane: the sheet, its preview underneath and whatever tiles have
/// arrived. Used for the pane being worked in and for the other one alike, so
/// a split cannot end up with two panes that draw differently.
fn paint_one(
    painter: &egui::Painter,
    doc: &mut crate::sheet::Doc,
    view: &View,
    page: u32,
    area: Rect,
    size: PageSize,
    _theme: ui::Theme,
) {
    let id = doc.id;
    let page_rect = Rect::from_min_max(
        view.to_screen([0.0, 0.0]),
        view.to_screen([size.width, size.height]),
    );
    painter.rect_filled(
        page_rect.translate(vec2(3.0, 3.0)),
        2.0,
        Color32::from_black_alpha(90),
    );
    painter.rect_filled(page_rect, 0.0, Color32::WHITE);

    // The low resolution page sits underneath so there is never a blank window
    // while the sharp tiles are still coming.
    if let Some((_, preview)) = doc.previews.get(&page) {
        painter.image(
            preview.id(),
            page_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    let bucket =
        render::bucket_for_zoom(view.zoom).clamp(render::MIN_BUCKET, render::MAX_BUCKET);
    let scale = render::bucket_scale(bucket);
    let mut drawn: Vec<TileKey> = Vec::new();
    for ty in 0..=((size.height as f64 * scale / TILE as f64).ceil() as u32) {
        for tx in 0..=((size.width as f64 * scale / TILE as f64).ceil() as u32) {
            let key = TileKey {
                doc: id,
                page,
                bucket,
                tx,
                ty,
            };
            if doc.tiles.has(&key) {
                drawn.push(key);
            }
        }
    }
    let ppp = painter.ctx().pixels_per_point();
    let round = |v: f32| (v * ppp).round() / ppp;
    for key in drawn {
        let Some((x0, y0, w, h)) = render::tile_extent(size, bucket, key.tx, key.ty) else {
            continue;
        };
        let sheet_x0 = x0 / scale;
        let sheet_y0 = y0 / scale;
        let sheet_x1 = ((x0 + w as f64) / scale).min(size.width as f64);
        let sheet_y1 = ((y0 + h as f64) / scale).min(size.height as f64);
        let min = view.to_screen([sheet_x0 as f32, sheet_y0 as f32]);
        let max = view.to_screen([sheet_x1 as f32, sheet_y1 as f32]);
        let rect = Rect::from_min_max(
            pos2(round(min.x), round(min.y)),
            pos2(round(max.x), round(max.y)),
        );
        if !rect.intersects(area) {
            continue;
        }
        if let Some(texture) = doc.tiles.get(&key) {
            // The last tile in a row or column is cut short by the page edge.
            // How much of its texture that is depends on how big the texture
            // really is, so it is asked rather than assumed.
            let [u, v] = render::tile_uv((w, h), texture.size());
            painter.image(
                texture.id(),
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(u, v)),
                Color32::WHITE,
            );
        }
    }
    painter.rect_stroke(
        page_rect,
        0.0,
        Stroke::new(1.0_f32, Color32::from_gray(90)), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
        egui::StrokeKind::Outside,
    );
}

/// Divides the sheet area. `None` when there is only one pane.
fn halves(whole: Rect, split: Split) -> (Rect, Option<Rect>) {
    match split {
        Split::None => (whole, None),
        Split::Vertical => {
            let middle = whole.center().x;
            (
                Rect::from_min_max(whole.min, pos2(middle - 1.0, whole.max.y)),
                Some(Rect::from_min_max(pos2(middle + 1.0, whole.min.y), whole.max)),
            )
        }
        Split::Horizontal => {
            let middle = whole.center().y;
            (
                Rect::from_min_max(whole.min, pos2(whole.max.x, middle - 1.0)),
                Some(Rect::from_min_max(pos2(whole.min.x, middle + 1.0), whole.max)),
            )
        }
    }
}


/// Fills a shape, concave or not.
///
/// egui fans a path from its first point, which is right for a convex shape and
/// wrong for every other. A takeoff is mostly the other kind — L-shaped rooms,
/// slabs with notches, whatever Dynamic Fill traced round a stair — so the
/// shape is cut into triangles first. A shape that crosses itself has no inside
/// to fill and gets its outline only, rather than a plausible-looking lie.
fn fill_shape(painter: &egui::Painter, points: &[egui::Pos2], colour: Color32) {
    if points.len() < 3 {
        return;
    }
    if let Some(mesh) = ui::tess::filled(points, colour) {
        painter.add(egui::Shape::mesh(mesh));
    }
}

/// What the fingers are doing on the sheet, between the first touching the
/// glass and the last leaving it.
#[derive(Default)]
pub struct Touch {
    /// A finger is down.
    pub down: bool,
    /// Two or more, moving and zooming the sheet.
    pub pinching: bool,
    /// One, on bare paper with the Select tool: moving the sheet.
    pub panning: bool,
    /// The fingers have just let go of the sheet.
    pub settling: bool,
    /// The sheet's draft when the first finger came down.
    pub draft_before: Option<Option<crate::sheet::Draft>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_text(subtype: annot::Subtype, text: &str) -> Look {
        let mut markup = annot::Markup::new(subtype);
        markup.set_contents(text);
        Look::of(&markup, false)
    }

    #[test]
    fn markup_text_is_off_unless_it_is_wanted() {
        let look = with_text(annot::Subtype::Line, "24'-6\"").labelled(false);
        assert!(look.caption.is_empty());
        let look = with_text(annot::Subtype::Line, "24'-6\"").labelled(true);
        assert_eq!(look.caption, "24'-6\"");
    }

    #[test]
    fn a_text_box_never_gets_a_label_of_its_own_words() {
        assert!(with_text(annot::Subtype::FreeText, "SEE DETAIL 4").labelled(true).caption.is_empty());
    }

    #[test]
    fn a_paragraph_is_cut_short_on_one_line() {
        let index = "COVER SHEET\nEXISTING CONDITIONS PLAN\nDEMOLITION PLAN 08/31/26 08/31/26 08/31/26 08/31/26 STANDARD DETAILS";
        let look = with_text(annot::Subtype::Highlight, index).labelled(true);
        assert!(look.caption.chars().count() <= LABEL_LIMIT, "{}", look.caption);
        assert!(look.caption.ends_with('…'));
        assert!(!look.caption.contains('\n'));
        assert!(look.caption.starts_with("COVER SHEET EXISTING CONDITIONS PLAN"));
    }
}


/// Whether a press-and-release was a drag or just a click that wobbled.
///
/// Judged on the sheet rather than on the screen: four pixels at a zoom of
/// eight is half a sheet unit, and somebody zoomed right in on a connection
/// should not have a twitch turned into a plate.
fn a_real_drag(from: [f64; 2], to: [f64; 2], zoom: f64) -> bool {
    let zoom = if zoom > 0.0 { zoom } else { 1.0 };
    let enough = 4.0 / zoom;
    (to[0] - from[0]).abs() > enough && (to[1] - from[1]).abs() > enough
}

#[cfg(test)]
mod dragging_out_a_box {
    use super::a_real_drag;

    #[test]
    fn a_click_that_wobbled_is_not_a_drag() {
        assert!(!a_real_drag([100.0, 100.0], [101.0, 101.0], 1.0));
        assert!(!a_real_drag([100.0, 100.0], [100.0, 100.0], 1.0));
    }

    #[test]
    fn a_box_dragged_out_is() {
        assert!(a_real_drag([100.0, 100.0], [340.0, 260.0], 1.0));
    }

    #[test]
    fn a_line_is_not_a_box_however_long_it_is() {
        // Dragged along one axis only: that is not a rectangle, it is a
        // line, and turning it into a zero-area plate would be wrong.
        assert!(!a_real_drag([100.0, 100.0], [900.0, 100.0], 1.0));
        assert!(!a_real_drag([100.0, 100.0], [100.0, 900.0], 1.0));
    }

    #[test]
    fn zoomed_right_in_a_smaller_movement_still_counts() {
        // Two sheet units is a big movement at 8x and a twitch at 1x.
        assert!(a_real_drag([10.0, 10.0], [12.0, 12.0], 8.0));
        assert!(!a_real_drag([10.0, 10.0], [12.0, 12.0], 1.0));
    }
}
