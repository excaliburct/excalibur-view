//! The 3D tab of a model window: the model itself, turned with the mouse.
//!
//! Drawing is `view3d`'s, on a thread and a graphics device of its own; the
//! shapes are the `model` crate's, which draws every part uncut in a moment
//! and then cuts the holes and copes in on the other cores. This file is the
//! part in between: which part number is which piece of steel, what colour
//! each is, what's hidden and selected, and the mouse.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use egui::{Color32, RichText, Sense};
use model::shapes::{Group, News};
use view3d::{Background, Camera, Look, Preset, Probe, Snap, State, View};

/// What decides a part's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColourBy {
    Kind,
    Profile,
    Assembly,
    /// Where the takeoff got the part's weight: the check an estimator
    /// wants before trusting the total.
    WeightFrom,
    Plain,
}

impl ColourBy {
    const ALL: [ColourBy; 5] = [ColourBy::Kind, ColourBy::Profile, ColourBy::Assembly, ColourBy::WeightFrom, ColourBy::Plain];

    fn name(self) -> &'static str {
        match self {
            ColourBy::Kind => "Kind",
            ColourBy::Profile => "Profile",
            ColourBy::Assembly => "Assembly",
            ColourBy::WeightFrom => "Where the weight came from",
            ColourBy::Plain => "One colour",
        }
    }
}

const SELECTED: [u8; 4] = [0xF2, 0x8C, 0x28, 255];
const PLAIN: [u8; 4] = [0x8C, 0x99, 0xA6, 255];
const FROM_BASE: [u8; 4] = [0x4E, 0x9F, 0x6E, 255];
const FROM_EXPORTER: [u8; 4] = [0x4F, 0x86, 0xC6, 255];
const FROM_NOWHERE: [u8; 4] = [0xD9, 0x4F, 0x4A, 255];
const NOT_A_PART: [u8; 4] = [0xC4, 0xC9, 0xCF, 255];

fn group_colour(group: Group) -> [u8; 4] {
    match group {
        Group::Column => [0x35, 0x61, 0x8F, 255],
        Group::Beam => [0x8D, 0xA9, 0xC4, 255],
        Group::Brace => [0x5B, 0xA0, 0x8A, 255],
        Group::Plate => [0xD2, 0x9B, 0x3F, 255],
        Group::Bolt => [0x8C, 0x6A, 0x3B, 255],
        Group::Concrete => [0xBD, 0xB8, 0xAE, 255],
        Group::Other => [0xA3, 0xAC, 0xB6, 255],
    }
}

/// The `n`th of a run of colours that stay apart from their neighbours:
/// hues a golden angle apart, at one steady strength.
fn nth_colour(n: usize) -> [u8; 4] {
    let hue = (n as f32 * 137.508 + 205.0) % 360.0;
    let (s, l) = (0.46, 0.56);
    let c = (1.0 - (2.0 * l - 1.0f32).abs()) * s;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (hue / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [byte(r), byte(g), byte(b), 255]
}

fn faded(c: [u8; 4]) -> [u8; 4] {
    let lift = |v: u8| (v as f32 + (255.0 - v as f32) * 0.5) as u8;
    [lift(c[0]), lift(c[1]), lift(c[2]), c[3]]
}

/// One drawn thing: which entity it is and what the takeoff knows of it.
struct Drawn {
    id: u32,
    group: Group,
    name: String,
    /// Its place in `Model::parts`, when it's a part.
    part: Option<usize>,
    /// Its place in `Model::bolts`, when it's a bolt.
    bolt: Option<usize>,
}

/// How far drawing has got.
#[derive(Default)]
struct Progress {
    rough: bool,
    to_cut: usize,
    cut: usize,
    uncuttable: usize,
    not_drawn: Vec<(u32, String)>,
    finished: bool,
}

/// What the view was last asked to draw, so it isn't asked twice.
#[derive(Clone, Copy, PartialEq)]
struct Asked {
    camera: Camera,
    width: u32,
    height: u32,
    look: Look,
    /// Which set of colours the view had been sent.
    colours: u64,
}

pub struct Scene {
    view: View,
    meshing: Option<model::shapes::Meshing>,
    drawn: Vec<Drawn>,
    number: HashMap<u32, u32>,
    part_of: HashMap<u32, usize>,
    bolt_of: HashMap<u32, usize>,
    by_profile: HashMap<(model::Kind, String), [u8; 4]>,
    by_assembly: HashMap<String, [u8; 4]>,
    progress: Progress,

    pub camera: Camera,
    fitted: bool,
    pub look: Look,
    colour_by: ColourBy,
    hidden_groups: HashSet<Group>,
    hidden: HashSet<u32>,
    pub selected: BTreeSet<u32>,
    cutting: bool,
    cut_at: f32,
    turning: bool,

    colours: u64,
    colours_sent: u64,
    asked: Option<Asked>,
    /// The serial of the last picture asked for, and of the one on screen.
    waiting: u64,
    shown: u64,
    /// After the last shape arrives, a moment for its picture to follow.
    settle_until: Option<Instant>,
    texture: Option<egui::TextureHandle>,
    picking: Option<(mpsc::Receiver<Option<u32>>, Click)>,
    /// Measuring: clicks put down points instead of selecting.
    measuring: bool,
    marks: Vec<Probe>,
    probing: Option<mpsc::Receiver<Option<Probe>>>,
    saving: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    /// Save picture… was pressed: the name to suggest, waiting for somewhere
    /// to put it. The window around this asks where.
    pub picture_wanted: Option<String>,
    pub status: Option<String>,
    aspect: f32,
}

#[derive(Clone, Copy)]
enum Click {
    Select,
    Toggle,
    Frame,
}

impl Scene {
    pub fn new(bytes: Arc<Vec<u8>>, model: &model::Model) -> Scene {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).saturating_sub(1).clamp(1, 8);
        let mut by_profile = HashMap::new();
        for (n, row) in model.by_profile().iter().enumerate() {
            by_profile.insert((row.kind, profile_key(&row.profile)), nth_colour(n));
        }
        let assemblies: BTreeSet<&str> =
            model.parts.iter().map(|p| p.assembly.as_str()).filter(|a| !a.is_empty()).collect();
        let by_assembly = assemblies.into_iter().enumerate().map(|(n, a)| (a.to_string(), nth_colour(n))).collect();
        Scene {
            view: View::start(),
            meshing: Some(model::shapes::start(bytes, threads)),
            drawn: Vec::new(),
            number: HashMap::new(),
            part_of: model.parts.iter().enumerate().map(|(i, p)| (p.id, i)).collect(),
            bolt_of: model.bolts.iter().enumerate().map(|(i, b)| (b.id, i)).collect(),
            by_profile,
            by_assembly,
            progress: Progress::default(),
            camera: Camera::default(),
            fitted: false,
            look: Look::default(),
            colour_by: ColourBy::Kind,
            hidden_groups: HashSet::new(),
            hidden: HashSet::new(),
            selected: BTreeSet::new(),
            cutting: false,
            cut_at: 0.0,
            turning: false,
            colours: 1,
            colours_sent: 0,
            asked: None,
            waiting: 0,
            shown: 0,
            settle_until: None,
            texture: None,
            picking: None,
            measuring: false,
            marks: Vec::new(),
            probing: None,
            saving: None,
            picture_wanted: None,
            status: None,
            aspect: 1.5,
        }
    }

    /// Selects every drawn part whose entity is one of `ids`, and frames
    /// them.
    pub fn select_ids(&mut self, ids: &HashSet<u32>) {
        self.selected = ids.iter().filter_map(|id| self.number.get(id).copied()).collect();
        self.hidden.retain(|n| !self.selected.contains(n));
        self.colours += 1;
        if let Some(bounds) = self.view.bounds_of(self.selected.iter().copied()) {
            self.camera.fit(bounds, self.aspect);
        }
    }

    /// Takes in what the drawing threads have finished. A few hundred at a
    /// time, so a burst doesn't hold up the window.
    fn hear(&mut self) {
        let Some(meshing) = &self.meshing else { return };
        let mut news = Vec::new();
        let mut gone = false;
        for _ in 0..400 {
            match meshing.news.try_recv() {
                Ok(n) => news.push(n),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    gone = true;
                    break;
                }
            }
        }
        for n in news {
            match n {
                News::Rough { shapes, failed, to_cut } => {
                    for shape in shapes {
                        self.add(shape);
                    }
                    self.progress.rough = true;
                    self.progress.to_cut = to_cut;
                    self.progress.not_drawn = failed;
                    self.colours += 1;
                }
                News::Cut(shape) => {
                    self.progress.cut += 1;
                    if !self.number.contains_key(&shape.id) {
                        self.colours += 1;
                    }
                    self.add(shape);
                }
                News::Uncuttable(..) => self.progress.uncuttable += 1,
                News::Finished => {
                    self.progress.finished = true;
                    gone = true;
                }
            }
        }
        if gone {
            self.meshing = None;
            self.settle_until = Some(Instant::now() + Duration::from_millis(900));
            if !self.progress.rough {
                self.status = Some("The model's shapes couldn't be read.".into());
            }
        }
    }

    fn add(&mut self, shape: model::shapes::Shape) {
        let number = match self.number.get(&shape.id) {
            Some(&n) => n,
            None => {
                let n = self.drawn.len() as u32;
                self.number.insert(shape.id, n);
                self.drawn.push(Drawn {
                    id: shape.id,
                    group: shape.group,
                    name: shape.name.clone(),
                    part: self.part_of.get(&shape.id).copied(),
                    bolt: self.bolt_of.get(&shape.id).copied(),
                });
                n
            }
        };
        self.view.set_part(
            number,
            view3d::Mesh { origin: shape.origin, positions: shape.positions, normals: shape.normals, indices: shape.indices },
        );
    }

    fn palette(&self, model: &model::Model) -> Vec<[u8; 4]> {
        let fade = !self.selected.is_empty();
        self.drawn
            .iter()
            .enumerate()
            .map(|(n, d)| {
                let n = n as u32;
                if self.hidden_groups.contains(&d.group) || self.hidden.contains(&n) {
                    return [0; 4];
                }
                if self.selected.contains(&n) {
                    return SELECTED;
                }
                let part = d.part.map(|i| &model.parts[i]);
                let colour = match (self.colour_by, part) {
                    (ColourBy::Kind, _) => group_colour(d.group),
                    (ColourBy::Plain, _) => PLAIN,
                    (ColourBy::Profile, Some(p)) => {
                        self.by_profile.get(&(p.kind, profile_key(&p.profile))).copied().unwrap_or(PLAIN)
                    }
                    (ColourBy::Assembly, Some(p)) => self.by_assembly.get(&p.assembly).copied().unwrap_or(PLAIN),
                    (ColourBy::WeightFrom, Some(p)) => match p.weight_from {
                        model::Source::BaseQuantities => FROM_BASE,
                        model::Source::Exporter | model::Source::Shape => FROM_EXPORTER,
                        model::Source::Nowhere => FROM_NOWHERE,
                    },
                    (_, None) => {
                        if self.colour_by == ColourBy::WeightFrom {
                            NOT_A_PART
                        } else {
                            group_colour(d.group)
                        }
                    }
                };
                if fade {
                    faded(colour)
                } else {
                    colour
                }
            })
            .collect()
    }


    /// The whole tab: the picture, and the panel beside it.
    pub fn ui(&mut self, ui: &mut egui::Ui, model: &model::Model, theme: &ui::chrome::Theme, stem: &str) {
        self.hear();
        self.answers();
        egui::SidePanel::right(ui.id().with("3d side"))
            .resizable(false)
            .exact_width(240.0)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.side(ui, model, theme, stem));
            });
        if self.colours_sent != self.colours {
            self.view.set_colours(self.palette(model));
            self.colours_sent = self.colours;
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show_inside(ui, |ui| self.picture(ui, theme));

        let ctx = ui.ctx();
        let settling = self.settle_until.is_some_and(|t| Instant::now() < t);
        if self.meshing.is_some() || settling {
            ctx.request_repaint_after(Duration::from_millis(60));
        }
        if self.waiting > self.shown || self.picking.is_some() || self.probing.is_some() {
            ctx.request_repaint_after(Duration::from_millis(8));
        }
        if self.saving.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.turning {
            ctx.request_repaint();
        }
    }

    /// Clicks answered and pictures saved since the last frame.
    fn answers(&mut self) {
        if let Some((rx, click)) = &self.picking {
            match rx.try_recv() {
                Ok(found) => {
                    let click = *click;
                    self.picking = None;
                    self.clicked(found, click);
                }
                Err(mpsc::TryRecvError::Disconnected) => self.picking = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.probing {
            match rx.try_recv() {
                Ok(found) => {
                    self.probing = None;
                    if let Some(probe) = found {
                        if self.marks.len() >= 2 {
                            self.marks.clear();
                        }
                        self.marks.push(probe);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => self.probing = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.saving {
            match rx.try_recv() {
                Ok(done) => {
                    self.status = Some(match done {
                        Ok(path) => format!("Picture saved to {}", path.display()),
                        Err(why) => why,
                    });
                    self.saving = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.saving = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    fn clicked(&mut self, found: Option<u32>, click: Click) {
        match (click, found) {
            (Click::Select, Some(n)) => self.selected = [n].into(),
            (Click::Select, None) => self.selected.clear(),
            (Click::Toggle, Some(n)) => {
                if !self.selected.remove(&n) {
                    self.selected.insert(n);
                }
            }
            (Click::Toggle, None) => {}
            (Click::Frame, Some(n)) => {
                self.selected = [n].into();
                if let Some(bounds) = self.view.bounds_of([n]) {
                    self.camera.fit(bounds, self.aspect);
                    // Enough around it to see what it connects to.
                    self.camera.distance *= 2.5;
                }
            }
            (Click::Frame, None) => self.frame_selection(),
        }
        self.colours += 1;
    }

    fn frame_selection(&mut self) {
        let bounds = if self.selected.is_empty() {
            self.visible_bounds()
        } else {
            self.view.bounds_of(self.selected.iter().copied())
        };
        if let Some(bounds) = bounds {
            self.camera.fit(bounds, self.aspect);
        }
    }

    /// Around everything not hidden.
    fn visible_bounds(&self) -> Option<view3d::Bounds> {
        if self.hidden.is_empty() && self.hidden_groups.is_empty() {
            return self.view.bounds();
        }
        self.view.bounds_of(
            self.drawn
                .iter()
                .enumerate()
                .filter(|(n, d)| !self.hidden.contains(&(*n as u32)) && !self.hidden_groups.contains(&d.group))
                .map(|(n, _)| n as u32),
        )
    }

    fn picture(&mut self, ui: &mut egui::Ui, theme: &ui::chrome::Theme) {
        let size = ui.available_size().max(egui::vec2(64.0, 64.0));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme.sunken);
        let state = self.view.state().clone();
        if let State::Failed(why) = &state {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("The 3D view can't start on this computer.\n{why}\nThe takeoff tabs still work."),
                egui::FontId::proportional(13.0),
                theme.faint,
            );
            return;
        }
        let ppp = ui.ctx().pixels_per_point();
        let width = (rect.width() * ppp).round().max(1.0) as u32;
        let height = (rect.height() * ppp).round().max(1.0) as u32;
        self.aspect = rect.width() / rect.height().max(1.0);

        if !self.fitted {
            if let Some(bounds) = self.view.bounds() {
                self.camera.fit(bounds, self.aspect);
                self.cut_at = bounds.max[2];
                self.fitted = true;
            }
        }

        // Left drag turns it, right or middle drag (or Shift and left)
        // slides it, the wheel or a pinch zooms to the pointer.
        let (shift, command) = ui.input(|i| (i.modifiers.shift, i.modifiers.command || i.modifiers.ctrl));
        let delta = response.drag_delta();
        if response.dragged_by(egui::PointerButton::Primary) && !shift {
            self.camera.orbit(delta.x, delta.y);
            self.turning = false;
        } else if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
            || (response.dragged_by(egui::PointerButton::Primary) && shift)
        {
            self.camera.pan(delta.x, delta.y, rect.height());
        }
        if response.hovered() {
            let (scroll, pinch, pointer) =
                ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos()));
            let mut factor = 1.0;
            if scroll != 0.0 {
                factor *= (-scroll * 0.0018).exp();
            }
            if pinch != 1.0 {
                factor /= pinch;
            }
            if factor != 1.0 {
                let at = pointer.map_or([0.0, 0.0], |p| {
                    [(p.x - rect.center().x) / (rect.width() * 0.5), -(p.y - rect.center().y) / (rect.height() * 0.5)]
                });
                self.camera.zoom(factor, at, self.aspect);
            }
            let (fit, clear) = ui.input(|i| (i.key_pressed(egui::Key::F), i.key_pressed(egui::Key::Escape)));
            if fit {
                self.frame_selection();
            }
            if clear && !self.marks.is_empty() {
                self.marks.clear();
            } else if clear && !self.selected.is_empty() {
                self.selected.clear();
                self.colours += 1;
            }
        }
        if self.measuring {
            if let (true, Some(pos)) = (response.clicked(), response.interact_pointer_pos()) {
                let x = ((pos.x - rect.min.x) * ppp).max(0.0) as u32;
                let y = ((pos.y - rect.min.y) * ppp).max(0.0) as u32;
                self.probing = Some(self.view.probe(self.camera, width, height, self.look, x, y));
            }
        }
        let click = if self.measuring {
            None
        } else if response.double_clicked() {
            Some(Click::Frame)
        } else if response.clicked() {
            Some(if command || shift { Click::Toggle } else { Click::Select })
        } else {
            None
        };
        if let (Some(click), Some(pos)) = (click, response.interact_pointer_pos()) {
            let x = ((pos.x - rect.min.x) * ppp).max(0.0) as u32;
            let y = ((pos.y - rect.min.y) * ppp).max(0.0) as u32;
            self.picking = Some((self.view.pick(self.camera, width, height, self.look, x, y), click));
        }
        if self.turning {
            let dt = ui.input(|i| i.stable_dt).min(0.1);
            self.camera.yaw += dt * 0.35;
        }

        self.look.cut_above = self.cutting.then_some(self.cut_at);
        let asked = Asked { camera: self.camera, width, height, look: self.look, colours: self.colours_sent };
        if self.asked != Some(asked) && matches!(state, State::Ready(_)) {
            self.waiting = self.view.draw(self.camera, width, height, self.look);
            self.asked = Some(asked);
        }
        if let Some(frame) = self.view.frame() {
            self.shown = frame.serial;
            let image =
                egui::ColorImage::from_rgba_premultiplied([frame.width as usize, frame.height as usize], &frame.rgba);
            match &mut self.texture {
                Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
                None => self.texture = Some(ui.ctx().load_texture("3d view", image, egui::TextureOptions::LINEAR)),
            }
        }
        match &self.texture {
            Some(texture) => {
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                painter.image(texture.id(), rect, uv, Color32::WHITE);
            }
            None => {
                let words = if matches!(state, State::Starting) { "Starting the 3D view…" } else { "Drawing the model…" };
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, words, egui::FontId::proportional(13.0), theme.faint);
            }
        }
        self.draw_marks(&painter, rect);
        if self.measuring {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }

    /// The measured points and the line between them, over the picture.
    fn draw_marks(&self, painter: &egui::Painter, rect: egui::Rect) {
        if self.marks.is_empty() {
            return;
        }
        let m = self.view.view_proj(self.camera, self.aspect);
        let on_screen = |p: [f32; 3]| -> Option<egui::Pos2> {
            let c = view3d::camera::project(&m, p);
            (c[3] > 1e-6).then(|| {
                egui::pos2(
                    rect.min.x + (c[0] / c[3] * 0.5 + 0.5) * rect.width(),
                    rect.min.y + (0.5 - c[1] / c[3] * 0.5) * rect.height(),
                )
            })
        };
        let ink = Color32::from_rgb(0xE8, 0x3E, 0x8C);
        let points: Vec<Option<egui::Pos2>> = self.marks.iter().map(|mark| on_screen(mark.point)).collect();
        if let [Some(a), Some(b)] = points[..] {
            painter.line_segment([a, b], egui::Stroke::new(2.0_f32, ink)); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
            let words = feet_and_inches(distance(self.marks[0].point, self.marks[1].point) as f64);
            let middle = a + (b - a) * 0.5 + egui::vec2(0.0, -12.0);
            let galley = painter.layout_no_wrap(words, egui::FontId::proportional(13.0), Color32::WHITE);
            let back = egui::Rect::from_center_size(middle, galley.size() + egui::vec2(10.0, 4.0));
            painter.rect_filled(back, 3.0, ink);
            painter.galley(back.min + egui::vec2(5.0, 2.0), galley, Color32::WHITE);
        }
        for (mark, at) in self.marks.iter().zip(points) {
            let Some(at) = at else { continue };
            match mark.snapped {
                Snap::Corner => {
                    painter.rect_filled(egui::Rect::from_center_size(at, egui::vec2(9.0, 9.0)), 0.0, ink);
                }
                Snap::Edge => {
                    painter.circle_stroke(at, 5.0, egui::Stroke::new(2.0_f32, ink)); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                }
                Snap::Face => {
                    painter.circle_filled(at, 3.0, ink);
                }
            }
        }
    }

    fn side(&mut self, ui: &mut egui::Ui, model: &model::Model, theme: &ui::chrome::Theme, stem: &str) {
        let heading = |ui: &mut egui::Ui, text: &str| {
            ui.add_space(8.0);
            ui.label(RichText::new(text).strong().size(11.5));
        };
        let small = |text: String| RichText::new(text).size(10.5).color(theme.faint);

        // How far along the drawing is.
        let p = &self.progress;
        if !p.rough {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(small("Reading the shapes…".into()));
            });
        } else if !p.finished && p.to_cut > 0 {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(small(format!(
                    "Cutting holes and copes: {} of {}",
                    thousands(p.cut + p.uncuttable),
                    thousands(p.to_cut)
                )));
            });
        }
        if p.uncuttable > 0 {
            ui.label(small(format!(
                "{} part{} shown without {} cuts: the geometry library couldn't make them.",
                p.uncuttable,
                if p.uncuttable == 1 { " is" } else { "s are" },
                if p.uncuttable == 1 { "its" } else { "their" },
            )));
        }
        if !p.not_drawn.is_empty() {
            let reasons: Vec<String> =
                p.not_drawn.iter().take(8).map(|(id, why)| format!("#{id}: {why}")).collect();
            ui.label(RichText::new(format!("{} things in the file couldn't be drawn.", p.not_drawn.len())).size(10.5).color(theme.warn))
                .on_hover_text(reasons.join("\n"));
        }
        if let Some(status) = &self.status {
            ui.label(small(status.clone()));
        }

        heading(ui, "View");
        ui.horizontal_wrapped(|ui| {
            for preset in Preset::ALL {
                if ui.small_button(preset.name()).clicked() {
                    self.camera.look(preset);
                    if let Some(bounds) = self.visible_bounds() {
                        self.camera.fit(bounds, self.aspect);
                    }
                    self.turning = false;
                }
            }
            if ui.small_button("Fit").on_hover_text("Frame what's selected, or the whole model (F)").clicked() {
                self.frame_selection();
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.camera.orthographic, "Parallel")
                .on_hover_text("No perspective: elevations and plans measure true.");
            ui.checkbox(&mut self.turning, "Turn").on_hover_text("Turn the model slowly, for a screen at a show or a meeting.");
        });

        heading(ui, "Measure");
        if ui
            .selectable_label(self.measuring, if self.measuring { "Measuring: click two points" } else { "Start measuring" })
            .on_hover_text("Click two points on the model. A corner or an edge within a few pixels catches the pointer.")
            .clicked()
        {
            self.measuring = !self.measuring;
            if !self.measuring {
                self.marks.clear();
            }
        }
        if !self.marks.is_empty() {
            let base = self.view.anchor()[2];
            let snap = |s: Snap| match s {
                Snap::Corner => "corner",
                Snap::Edge => "edge",
                Snap::Face => "face",
            };
            let mut rows: Vec<(&str, String)> = Vec::new();
            for (name, mark) in ["First", "Second"].into_iter().zip(&self.marks) {
                rows.push((name, format!("{} at elevation {}", snap(mark.snapped), feet_and_inches(base + mark.point[2] as f64))));
            }
            if let [a, b] = self.marks[..] {
                let d = [b.point[0] - a.point[0], b.point[1] - a.point[1], b.point[2] - a.point[2]];
                rows.push(("Distance", feet_and_inches(distance(a.point, b.point) as f64)));
                rows.push(("Along X", feet_and_inches(d[0].abs() as f64)));
                rows.push(("Along Y", feet_and_inches(d[1].abs() as f64)));
                rows.push(("Up", feet_and_inches(d[2] as f64)));
                rows.push(("Level", feet_and_inches((d[0] * d[0] + d[1] * d[1]).sqrt() as f64)));
            }
            egui::Grid::new(ui.id().with("measured")).num_columns(2).spacing([8.0, 2.0]).show(ui, |ui| {
                for (key, value) in rows {
                    ui.label(RichText::new(key).size(10.5).color(theme.faint));
                    ui.label(RichText::new(value).size(11.0));
                    ui.end_row();
                }
            });
            if ui.small_button("Clear").clicked() {
                self.marks.clear();
            }
        }

        heading(ui, "Colour by");
        let before = self.colour_by;
        egui::ComboBox::from_id_salt(ui.id().with("colour by"))
            .selected_text(self.colour_by.name())
            .width(210.0)
            .show_ui(ui, |ui| {
                for choice in ColourBy::ALL {
                    ui.selectable_value(&mut self.colour_by, choice, choice.name());
                }
            });
        if self.colour_by != before {
            self.colours += 1;
        }
        if self.colour_by == ColourBy::WeightFrom {
            let mut counts = [0usize; 3];
            for d in &self.drawn {
                if let Some(part) = d.part.map(|i| &model.parts[i]) {
                    counts[match part.weight_from {
                        model::Source::BaseQuantities => 0,
                        model::Source::Nowhere => 2,
                        _ => 1,
                    }] += 1;
                }
            }
            for (colour, words, count) in [
                (FROM_BASE, "Base quantities", counts[0]),
                (FROM_EXPORTER, "The exporter's quantities", counts[1]),
                (FROM_NOWHERE, "Not stated: left out of the total", counts[2]),
            ] {
                ui.horizontal(|ui| {
                    swatch(ui, colour);
                    ui.label(RichText::new(format!("{words} ({})", thousands(count))).size(11.0));
                });
            }
        }

        heading(ui, "Show");
        let mut counts: HashMap<Group, usize> = HashMap::new();
        for d in &self.drawn {
            *counts.entry(d.group).or_default() += 1;
        }
        for group in Group::ALL {
            let Some(&count) = counts.get(&group) else { continue };
            ui.horizontal(|ui| {
                let mut shown = !self.hidden_groups.contains(&group);
                if ui.checkbox(&mut shown, "").changed() {
                    if shown {
                        self.hidden_groups.remove(&group);
                    } else {
                        self.hidden_groups.insert(group);
                    }
                    self.colours += 1;
                }
                if self.colour_by == ColourBy::Kind {
                    swatch(ui, group_colour(group));
                }
                ui.label(RichText::new(format!("{} ({})", group.name(), thousands(count))).size(11.0));
            });
        }
        if !self.hidden.is_empty() && ui.small_button(format!("Show the {} hidden by hand", self.hidden.len())).clicked() {
            self.hidden.clear();
            self.colours += 1;
        }

        heading(ui, "Look");
        ui.horizontal(|ui| {
            ui.label(RichText::new("Background").size(11.0));
            egui::ComboBox::from_id_salt(ui.id().with("background"))
                .selected_text(self.look.background.name())
                .show_ui(ui, |ui| {
                    for background in Background::ALL {
                        ui.selectable_value(&mut self.look.background, background, background.name());
                    }
                });
        });
        ui.checkbox(&mut self.look.edges, "Outline every part");
        if let Some(bounds) = self.view.bounds() {
            ui.checkbox(&mut self.cutting, "Cut away everything above");
            if self.cutting {
                let base = self.view.anchor()[2];
                ui.add(egui::Slider::new(&mut self.cut_at, bounds.min[2]..=bounds.max[2]).show_value(false));
                ui.label(small(format!("Elevation {}", feet_and_inches(base + self.cut_at as f64))));
            }
        }

        heading(ui, "Selected");
        self.selection(ui, model, theme);

        heading(ui, "Picture");
        let saving = self.saving.is_some();
        if ui
            .add_enabled(!saving, egui::Button::new("Save picture…"))
            .on_hover_text("This view as a PNG, at twice the size it is on screen. With the Transparent background it drops onto a web page or a slide as it is.")
            .clicked()
        {
            self.picture_wanted = Some(stem.to_string());
        }

        ui.add_space(10.0);
        ui.label(small(
            "Drag to turn. Right-drag or Shift-drag to move. Scroll to zoom to the pointer. \
             Click a part to select it, Ctrl-click to add to the selection, double-click to go to it. \
             Esc clears a measurement, then the selection. F frames what's selected."
                .into(),
        ));
    }

    fn selection(&mut self, ui: &mut egui::Ui, model: &model::Model, theme: &ui::chrome::Theme) {
        if self.selected.is_empty() {
            ui.label(RichText::new("Click a part to see what it is.").size(11.0).color(theme.faint));
            return;
        }
        let grid = |ui: &mut egui::Ui, rows: Vec<(&str, String)>| {
            egui::Grid::new(ui.id().with("selected")).num_columns(2).spacing([8.0, 2.0]).show(ui, |ui| {
                for (key, value) in rows {
                    ui.label(RichText::new(key).size(10.5).color(theme.faint));
                    ui.label(RichText::new(value).size(11.0));
                    ui.end_row();
                }
            });
        };
        if self.selected.len() == 1 {
            let n = *self.selected.iter().next().expect("one");
            let Some(d) = self.drawn.get(n as usize) else { return };
            if let Some(part) = d.part.map(|i| &model.parts[i]) {
                let mut rows = vec![("Kind", part.kind.name().to_string()), ("Profile", part.profile.clone())];
                for (key, value) in [("Mark", &part.mark), ("Assembly", &part.assembly), ("Material", &part.material)] {
                    if !value.is_empty() {
                        rows.push((key, value.clone()));
                    }
                }
                if let Some(feet) = part.feet {
                    rows.push(("Length", feet_and_inches(feet * 0.3048)));
                }
                rows.push((
                    "Weight",
                    match part.pounds {
                        Some(lb) => format!("{lb:.1} lb, from {}", part.weight_from.name()),
                        None => "not stated in the model".into(),
                    },
                ));
                rows.push(("In the file", format!("#{}", part.id)));
                grid(ui, rows);
            } else if let Some(bolt) = d.bolt.map(|i| &model.bolts[i]) {
                grid(ui, vec![("Kind", "Bolt".into()), ("Size", bolt.size()), ("Name", bolt.name.clone()), ("In the file", format!("#{}", bolt.id))]);
            } else {
                let mut rows = vec![("Kind", d.group.name().to_string())];
                if !d.name.is_empty() {
                    rows.push(("Name", d.name.clone()));
                }
                rows.push(("In the file", format!("#{}", d.id)));
                grid(ui, rows);
            }
        } else {
            let parts: Vec<&model::Part> = self
                .selected
                .iter()
                .filter_map(|&n| self.drawn.get(n as usize).and_then(|d| d.part))
                .map(|i| &model.parts[i])
                .collect();
            let pounds: f64 = parts.iter().filter_map(|p| p.pounds).sum();
            let unweighed = parts.iter().filter(|p| p.pounds.is_none()).count();
            let mut rows = vec![("Parts", thousands(parts.len()))];
            if parts.len() < self.selected.len() {
                rows.push(("Other things", thousands(self.selected.len() - parts.len())));
            }
            rows.push(("Weight", format!("{} lb, {:.2} tons", model::thousands(pounds), pounds / 2000.0)));
            if unweighed > 0 {
                rows.push(("Unweighed", thousands(unweighed)));
            }
            grid(ui, rows);
        }
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("Go to").clicked() {
                self.frame_selection();
            }
            if ui.small_button("Show only these").clicked() {
                self.hidden = (0..self.drawn.len() as u32).filter(|n| !self.selected.contains(n)).collect();
                self.colours += 1;
                self.frame_selection();
            }
            if ui.small_button("Hide").clicked() {
                self.hidden.extend(self.selected.iter().copied());
                self.selected.clear();
                self.colours += 1;
            }
            if ui.small_button("Clear").clicked() {
                self.selected.clear();
                self.colours += 1;
            }
        });
    }

    /// Saves this view as a PNG at twice its size on screen, drawn and
    /// written off the window's thread.
    pub fn save_picture_to(&mut self, path: PathBuf) {
        let Some(asked) = self.asked else { return };
        let (mut width, mut height) = (asked.width * 2, asked.height * 2);
        let longest = width.max(height);
        if longest > 7680 {
            width = width * 7680 / longest;
            height = height * 7680 / longest;
        }
        let drawn = self.view.picture(self.camera, width, height, self.look);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = match drawn.recv() {
                Ok(Ok(frame)) => {
                    let frame = frame.unpremultiplied();
                    image::save_buffer(&path, &frame.rgba, frame.width, frame.height, image::ExtendedColorType::Rgba8)
                        .map(|()| path)
                        .map_err(|e| format!("Could not write the picture: {e}"))
                }
                Ok(Err(why)) => Err(format!("Could not draw the picture: {why}")),
                Err(_) => Err("The 3D view stopped before the picture was drawn.".into()),
            };
            let _ = tx.send(result);
        });
        self.saving = Some(rx);
        self.status = Some("Saving the picture…".into());
    }
}

/// Sections that differ only in case or spaces are one section, the same
/// rule `Model::by_profile` groups by.
pub fn profile_key(profile: &str) -> String {
    profile.to_uppercase().replace(' ', "")
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn swatch(ui: &mut egui::Ui, colour: [u8; 4]) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(11.0, 11.0), Sense::hover());
    ui.painter().rect_filled(rect, 2.0, Color32::from_rgb(colour[0], colour[1], colour[2]));
}

fn thousands(n: usize) -> String {
    model::thousands(n as f64)
}

/// Metres as feet and inches to the sixteenth: `102'-6 1/2"`.
pub fn feet_and_inches(metres: f64) -> String {
    let sixteenths = (metres.abs() / 0.0254 * 16.0).round() as i64;
    let feet = sixteenths / (12 * 16);
    let rest = (sixteenths % (12 * 16)) as f64 / 16.0;
    let sign = if metres < 0.0 && sixteenths > 0 { "-" } else { "" };
    // Under an inch reads 0 1/2", the way it's written on a drawing.
    let zero = if rest > 0.0 && rest < 1.0 { "0 " } else { "" };
    format!("{sign}{feet}'-{zero}{}", model::inches(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevations_read_the_way_a_detailer_writes_them() {
        assert_eq!(feet_and_inches(0.0), "0'-0\"");
        assert_eq!(feet_and_inches(3.6576), "12'-0\"");
        assert_eq!(feet_and_inches(31.2547), "102'-6 1/2\"");
        assert_eq!(feet_and_inches(-0.3175), "-1'-0 1/2\"");
    }

    #[test]
    fn neighbouring_colours_are_told_apart() {
        for n in 0..40 {
            let (a, b) = (nth_colour(n), nth_colour(n + 1));
            let apart: i32 = (0..3).map(|k| (a[k] as i32 - b[k] as i32).abs()).sum();
            assert!(apart > 60, "{n}: {a:?} {b:?}");
        }
    }
}
