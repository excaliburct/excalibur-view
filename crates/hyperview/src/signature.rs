//! Your own signature, made once and put on any sheet.
//!
//! Draw it with the mouse or a pen, or take it from a scan or a photograph of
//! it on paper. It is kept as a picture in your own profile on this computer —
//! not beside the program, where everybody using a shared install would have
//! it, and never sent to the office server. Putting it on a sheet makes a
//! stamp with the picture in it, which sizes and moves like any markup and
//! keeps its shape as it's resized.
//!
//! It is blended onto the sheet by multiplying, so the paper round a scanned
//! signature disappears and the lines of the drawing under it still show.
//!
//! This is a picture of a signature, the way Fill & Sign makes one. It is not
//! a certificate, and nothing here says who put it there.

use std::path::{Path, PathBuf};

use crate::app::App;

/// How wide a signature goes down on a sheet, in points, before anybody
/// resizes it: two inches.
pub const PLACED_WIDTH: f64 = 144.0;

/// One signature kept on this computer.
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub name: String,
    pub path: PathBuf,
}

/// Where this person's signatures are kept. Always their own profile.
pub fn folder() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("Excalibur Hyperview").join("signatures"))
}

/// The signatures kept in a folder, by name.
pub fn saved_in(folder: &Path) -> Vec<Saved> {
    let mut out: Vec<Saved> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")))
                .map(|path| Saved {
                    name: path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                    path,
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

pub fn saved() -> Vec<Saved> {
    folder().map(|f| saved_in(&f)).unwrap_or_default()
}

/// A name that is safe as a file name and not already taken in the folder.
pub fn free_name(folder: &Path, wanted: &str) -> String {
    let tidy: String = wanted
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let base = if tidy.is_empty() { "Signature".to_string() } else { tidy };
    if !folder.join(format!("{base}.png")).exists() {
        return base;
    }
    (2..1000)
        .map(|n| format!("{base} {n}"))
        .find(|n| !folder.join(format!("{n}.png")).exists())
        .unwrap_or(base)
}

/// Keeps a signature, as a PNG with the paper see-through.
pub fn save_in(folder: &Path, name: &str, picture: &image::RgbaImage) -> Result<Saved, String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let name = free_name(folder, name);
    let path = folder.join(format!("{name}.png"));
    picture
        .save_with_format(&path, image::ImageFormat::Png)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Saved { name, path })
}

/// Strokes drawn on the pad, turned into a picture: ink on nothing, three
/// pixels to every point on the pad so it stays sharp when printed, cropped
/// to the ink with a little room round it.
pub fn from_strokes(strokes: &[Vec<[f32; 2]>], ink: [u8; 3], width: f32) -> Option<image::RgbaImage> {
    const SCALE: f32 = 3.0;
    let points: Vec<[f32; 2]> = strokes.iter().flatten().copied().collect();
    let first = points.first()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first[0], first[1], first[0], first[1]);
    for p in &points {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    let pad = width * 2.0 + 2.0;
    let (ox, oy) = (x0 - pad, y0 - pad);
    let w = ((x1 - x0 + pad * 2.0) * SCALE).ceil().max(1.0) as u32;
    let h = ((y1 - y0 + pad * 2.0) * SCALE).ceil().max(1.0) as u32;
    let mut coverage = vec![0f32; (w * h) as usize];
    let radius = width * SCALE * 0.5;
    let mut dab = |a: [f32; 2], b: [f32; 2]| {
        let (ax, ay) = ((a[0] - ox) * SCALE, (a[1] - oy) * SCALE);
        let (bx, by) = ((b[0] - ox) * SCALE, (b[1] - oy) * SCALE);
        let reach = radius + 1.0;
        let (lx, ly) = ((ax.min(bx) - reach).floor().max(0.0) as u32, (ay.min(by) - reach).floor().max(0.0) as u32);
        let (hx, hy) = (
            ((ax.max(bx) + reach).ceil() as u32).min(w - 1),
            ((ay.max(by) + reach).ceil() as u32).min(h - 1),
        );
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = (dx * dx + dy * dy).max(1e-6);
        for y in ly..=hy {
            for x in lx..=hx {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let t = (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0);
                let (qx, qy) = (ax + dx * t, ay + dy * t);
                let d = ((px - qx).powi(2) + (py - qy).powi(2)).sqrt();
                // Soft edge a pixel wide, so the line is smooth rather than
                // stepped.
                let c = (radius + 0.5 - d).clamp(0.0, 1.0);
                let at = (y * w + x) as usize;
                if c > coverage[at] {
                    coverage[at] = c;
                }
            }
        }
    };
    for stroke in strokes {
        match stroke.as_slice() {
            [] => {}
            [one] => dab(*one, *one),
            many => {
                for pair in many.windows(2) {
                    dab(pair[0], pair[1]);
                }
            }
        }
    }
    let mut out = image::RgbaImage::new(w, h);
    for (i, pixel) in out.pixels_mut().enumerate() {
        *pixel = image::Rgba([ink[0], ink[1], ink[2], (coverage[i] * 255.0).round() as u8]);
    }
    Some(out)
}

/// A scan or a photograph of a signature on paper, with the paper made
/// see-through and the picture cropped to the ink. Anything lighter than
/// `paper` counts as paper; between that and the ink it fades, so the edges
/// of the strokes stay smooth.
pub fn from_paper(picture: &image::RgbaImage, paper: u8) -> Option<image::RgbaImage> {
    let (w, h) = picture.dimensions();
    let mut out = image::RgbaImage::new(w, h);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let paper = paper as f32;
    for (x, y, p) in picture.enumerate_pixels() {
        let light = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
        let ink = ((paper - light) / paper.max(1.0)).clamp(0.0, 1.0);
        // Darken what is kept, so a pale biro on grey paper still reads.
        let alpha = ((ink * 1.6).min(1.0) * p[3] as f32).round() as u8;
        out.put_pixel(x, y, image::Rgba([p[0], p[1], p[2], alpha]));
        if alpha > 40 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 > x1 || y0 > y1 {
        return None;
    }
    let margin = ((x1 - x0).max(y1 - y0) / 40).max(2);
    let (cx0, cy0) = (x0.saturating_sub(margin), y0.saturating_sub(margin));
    let (cx1, cy1) = ((x1 + margin).min(w - 1), (y1 + margin).min(h - 1));
    Some(image::imageops::crop_imm(&out, cx0, cy0, cx1 - cx0 + 1, cy1 - cy0 + 1).to_image())
}

/// Where a signature goes on a sheet, in PDF space, for a click at `at`:
/// centred on it, `PLACED_WIDTH` wide, the picture's own shape.
pub fn placed_at(at: [f64; 2], picture: (u32, u32)) -> [f64; 4] {
    let (w, h) = (picture.0.max(1) as f64, picture.1.max(1) as f64);
    let width = PLACED_WIDTH;
    let height = width * h / w;
    [at[0] - width * 0.5, at[1] - height * 0.5, at[0] + width * 0.5, at[1] + height * 0.5]
}

/// What the signature window is doing.
#[derive(Default)]
pub struct Signing {
    /// The window is up.
    pub open: bool,
    /// Making a new one, rather than picking.
    pub making: Option<Making>,
    /// The one to put down with the next click on the sheet.
    pub chosen: Option<Saved>,
    /// Pictures of the saved ones, for the window, by file.
    pub shown: std::collections::HashMap<PathBuf, egui::TextureHandle>,
    pub error: Option<String>,
}

/// A signature being made.
pub struct Making {
    pub name: String,
    pub from_paper: bool,
    pub strokes: Vec<Vec<[f32; 2]>>,
    pub blue: bool,
    /// A scan, as it came, and the paper made see-through.
    pub scan: Option<image::RgbaImage>,
    pub kept: Option<image::RgbaImage>,
    pub kept_shown: Option<egui::TextureHandle>,
}

impl Making {
    pub fn new(name: &str) -> Making {
        Making {
            name: name.to_string(),
            from_paper: false,
            strokes: Vec::new(),
            blue: false,
            scan: None,
            kept: None,
            kept_shown: None,
        }
    }

    fn ink(&self) -> [u8; 3] {
        if self.blue {
            [20, 45, 140]
        } else {
            [15, 15, 20]
        }
    }

    /// The picture that would be kept, if there is one yet.
    fn picture(&self) -> Option<image::RgbaImage> {
        if self.from_paper {
            self.kept.clone()
        } else {
            from_strokes(&self.strokes, self.ink(), 2.2)
        }
    }
}

impl App {
    /// The Signature tool: pick one of yours, or make one.
    pub fn begin_signature(&mut self) {
        self.signing.open = true;
        self.signing.error = None;
        if saved().is_empty() && self.signing.making.is_none() {
            self.signing.making = Some(Making::new("Signature"));
        }
    }

    /// Puts the chosen signature on the sheet, centred where it was clicked.
    /// `at` is in sheet space.
    pub fn place_signature(&mut self, at: [f64; 2]) {
        let Some(chosen) = self.signing.chosen.clone() else {
            self.begin_signature();
            return;
        };
        let picture = match crate::picture::read(&chosen.path) {
            Ok(p) => p,
            Err(why) => {
                self.error = Some(why);
                return;
            }
        };
        let author = self.author.clone();
        let Some(doc) = self.doc_mut() else { return };
        let frame = doc.frame();
        let area = placed_at(frame.to_pdf(at), (picture.width, picture.height));
        let mut markup = annot::Markup::new(annot::Subtype::Stamp).showing(picture);
        markup.set_box(area);
        markup.set_subject(if chosen.name.to_lowercase().starts_with("initial") { "Initials" } else { "Signature" });
        markup.set_multiply(true);
        if !author.is_empty() {
            markup.set("T", pdf::Object::text(&author));
        }
        doc.checkpoint_named("Sign");
        let page = doc.page;
        doc.marks.push(crate::sheet::Mark::new(page, markup));
        let at = doc.marks.len() - 1;
        doc.choose(Some(at));
        doc.dirty = true;
        // One signature per click. Back to picking things up, so it can be
        // moved and sized straight away.
        self.tool = crate::app::Tool::Select;
        self.status = "Signed. Drag a corner to size it; it keeps its shape.".into();
        self.save_soon();
    }

    /// The signature window: yours to pick from, and making a new one.
    pub fn signature_window(&mut self, ctx: &egui::Context) {
        if !self.signing.open {
            return;
        }
        let theme = self.chrome.theme;
        let mut open = true;
        let mut pick: Option<Saved> = None;
        let mut forget: Option<Saved> = None;
        let mut start_new = false;
        let mut keep_new = false;
        let mut cancel_new = false;
        let mut choose_scan = false;
        let list = saved();
        // Pictures of the saved ones, loaded once.
        for one in &list {
            if !self.signing.shown.contains_key(&one.path) {
                if let Ok(img) = image::open(&one.path) {
                    let rgba = img.to_rgba8();
                    let colour = egui::ColorImage::from_rgba_unmultiplied(
                        [rgba.width() as usize, rgba.height() as usize],
                        rgba.as_raw(),
                    );
                    let handle = ctx.load_texture(one.path.display().to_string(), colour, Default::default());
                    self.signing.shown.insert(one.path.clone(), handle);
                }
            }
        }
        let title = if self.signing.making.is_some() { "New signature" } else { "Signatures" };
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .max_width(560.0)
            .show(ctx, |ui| {
                if let Some(making) = self.signing.making.as_mut() {
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut making.name).desired_width(180.0));
                        if ui.small_button("Signature").clicked() {
                            making.name = "Signature".into();
                        }
                        if ui.small_button("Initials").clicked() {
                            making.name = "Initials".into();
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut making.from_paper, false, "Draw it");
                        ui.selectable_value(&mut making.from_paper, true, "From a scan or photo");
                    });
                    ui.add_space(6.0);
                    if !making.from_paper {
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(500.0, 170.0), egui::Sense::drag());
                        let painter = ui.painter_at(rect);
                        painter.rect_filled(rect, 4.0, egui::Color32::WHITE);
                        painter.line_segment(
                            [rect.left_bottom() + egui::vec2(24.0, -40.0), rect.right_bottom() + egui::vec2(-24.0, -40.0)],
                            egui::Stroke::new(1.0_f32, egui::Color32::from_gray(200)), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                        );
                        if let Some(p) = response.interact_pointer_pos() {
                            let local = [p.x - rect.left(), p.y - rect.top()];
                            if response.drag_started() || making.strokes.is_empty() && response.dragged() {
                                making.strokes.push(vec![local]);
                            } else if response.dragged() {
                                if let Some(last) = making.strokes.last_mut() {
                                    let far = last.last().is_none_or(|q| (q[0] - local[0]).abs() + (q[1] - local[1]).abs() > 0.8);
                                    if far {
                                        last.push(local);
                                    }
                                }
                            }
                        }
                        let ink = making.ink();
                        let colour = egui::Color32::from_rgb(ink[0], ink[1], ink[2]);
                        for stroke in &making.strokes {
                            let points: Vec<egui::Pos2> =
                                stroke.iter().map(|q| rect.left_top() + egui::vec2(q[0], q[1])).collect();
                            if points.len() == 1 {
                                painter.circle_filled(points[0], 1.2, colour);
                            } else {
                                painter.add(egui::Shape::line(points, egui::Stroke::new(2.2_f32, colour))); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                            }
                        }
                        if making.strokes.is_empty() {
                            painter.text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "Sign here with the mouse or a pen",
                                egui::FontId::proportional(14.0),
                                egui::Color32::from_gray(170),
                            );
                        }
                        ui.horizontal(|ui| {
                            if ui.small_button("Clear").clicked() {
                                making.strokes.clear();
                            }
                            if ui.small_button("Take back a stroke").clicked() {
                                making.strokes.pop();
                            }
                            ui.add_space(12.0);
                            ui.selectable_value(&mut making.blue, false, "Black");
                            ui.selectable_value(&mut making.blue, true, "Blue");
                        });
                    } else {
                        if ui.button("Choose a picture…").clicked() {
                            choose_scan = true;
                        }
                        ui.add_space(4.0);
                        match (&making.kept, making.kept_shown.as_ref()) {
                            (Some(kept), Some(shown)) => {
                                let (w, h) = (kept.width() as f32, kept.height() as f32);
                                let scale = (480.0 / w).min(160.0 / h).min(1.0);
                                let size = egui::vec2(w * scale, h * scale);
                                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                                ui.painter().rect_filled(rect, 2.0, egui::Color32::WHITE);
                                ui.painter().image(
                                    shown.id(),
                                    rect,
                                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                    egui::Color32::WHITE,
                                );
                            }
                            _ => {
                                ui.label(
                                    egui::RichText::new(
                                        "Sign a white sheet of paper, then scan it or take a photo \
                                         straight on in good light. The paper is taken away and \
                                         the picture cut down to the ink.",
                                    )
                                    .color(theme.faint)
                                    .size(11.0),
                                );
                            }
                        }
                    }
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(
                            "Kept on this computer, in your own profile. It is never sent to the \
                             office server.",
                        )
                        .color(theme.faint)
                        .size(10.0),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let ready = if making.from_paper { making.kept.is_some() } else { !making.strokes.is_empty() };
                        if ui.add_enabled(ready, egui::Button::new("Keep it")).clicked() {
                            keep_new = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel_new = true;
                        }
                    });
                } else {
                    if list.is_empty() {
                        ui.label("You haven't made a signature yet.");
                    } else {
                        ui.label(
                            egui::RichText::new("Pick one, then click the sheet where it goes.")
                                .color(theme.faint)
                                .size(11.0),
                        );
                        ui.add_space(6.0);
                        for one in &list {
                            ui.horizontal(|ui| {
                                let (rect, response) =
                                    ui.allocate_exact_size(egui::vec2(220.0, 64.0), egui::Sense::click());
                                let chosen = self.signing.chosen.as_ref() == Some(one);
                                ui.painter().rect_filled(rect, 4.0, egui::Color32::WHITE);
                                if chosen || response.hovered() {
                                    ui.painter().rect_stroke(
                                        rect,
                                        4.0,
                                        egui::Stroke::new(2.0_f32, theme.accent), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                if let Some(texture) = self.signing.shown.get(&one.path) {
                                    let [w, h] = texture.size();
                                    let scale = ((rect.width() - 12.0) / w as f32).min((rect.height() - 12.0) / h as f32);
                                    let size = egui::vec2(w as f32 * scale, h as f32 * scale);
                                    ui.painter().image(
                                        texture.id(),
                                        egui::Rect::from_center_size(rect.center(), size),
                                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                        egui::Color32::WHITE,
                                    );
                                }
                                if response.clicked() {
                                    pick = Some(one.clone());
                                }
                                ui.vertical(|ui| {
                                    ui.label(&one.name);
                                    if ui.small_button("Forget it").clicked() {
                                        forget = Some(one.clone());
                                    }
                                });
                            });
                            ui.add_space(4.0);
                        }
                    }
                    ui.add_space(6.0);
                    if ui.button("New signature…").clicked() {
                        start_new = true;
                    }
                }
                if let Some(why) = &self.signing.error {
                    ui.add_space(6.0);
                    ui.colored_label(egui::Color32::from_rgb(235, 120, 120), why);
                }
            });

        if choose_scan {
            let ctx = ctx.clone();
            self.filing.one(
                crate::files::Choose::open()
                    .filter("Pictures", &["png", "jpg", "jpeg"])
                    .title("A scan or photo of your signature"),
                move |app, path| app.take_signature_scan(&ctx, &path),
            );
        }
        if keep_new {
            let made = self.signing.making.as_ref().and_then(|m| m.picture().map(|p| (m.name.clone(), p)));
            match (made, folder()) {
                (Some((name, picture)), Some(folder)) => match save_in(&folder, &name, &picture) {
                    Ok(saved) => {
                        self.signing.making = None;
                        self.signing.chosen = Some(saved.clone());
                        self.signing.open = false;
                        self.tool = crate::app::Tool::Signature;
                        self.status = format!("{} kept. Click the sheet where it goes.", saved.name);
                    }
                    Err(why) => self.signing.error = Some(why),
                },
                (_, None) => self.signing.error = Some("There's nowhere in your profile to keep it.".into()),
                (None, _) => {}
            }
        }
        if cancel_new {
            self.signing.making = None;
            if list.is_empty() {
                self.signing.open = false;
            }
        }
        if start_new {
            let name = if list.iter().any(|s| s.name == "Signature") { "Initials" } else { "Signature" };
            self.signing.making = Some(Making::new(name));
        }
        if let Some(one) = forget {
            let _ = std::fs::remove_file(&one.path);
            self.signing.shown.remove(&one.path);
            if self.signing.chosen.as_ref() == Some(&one) {
                self.signing.chosen = None;
            }
        }
        if let Some(one) = pick {
            self.signing.chosen = Some(one.clone());
            self.signing.open = false;
            self.tool = crate::app::Tool::Signature;
            self.status = format!("Click the sheet where {} goes.", one.name);
        }
        if !open {
            self.signing.open = false;
            self.signing.making = None;
        }
    }
}

impl App {
    /// A scan or photo of a signature, with the paper taken out of it.
    fn take_signature_scan(&mut self, ctx: &egui::Context, path: &std::path::Path) {
        match image::open(&path) {
            Ok(img) => {
                let scan = img.to_rgba8();
                match from_paper(&scan, 200) {
                    Some(kept) => {
                        let colour = egui::ColorImage::from_rgba_unmultiplied(
                            [kept.width() as usize, kept.height() as usize],
                            kept.as_raw(),
                        );
                        let handle = ctx.load_texture("signature-scan", colour, Default::default());
                        if let Some(making) = self.signing.making.as_mut() {
                            making.scan = Some(scan);
                            making.kept = Some(kept);
                            making.kept_shown = Some(handle);
                        }
                        self.signing.error = None;
                    }
                    None => {
                        self.signing.error =
                            Some("There's no ink in that picture that can be told from the paper.".into())
                    }
                }
            }
            Err(e) => self.signing.error = Some(format!("{}: {e}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xev-signature-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_drawn_signature_is_ink_on_nothing_cropped_to_the_ink() {
        let strokes = vec![vec![[10.0, 10.0], [60.0, 30.0]], vec![[20.0, 40.0]]];
        let pic = from_strokes(&strokes, [0, 0, 0], 2.0).unwrap();
        // Three pixels to the point, the ink plus a little room.
        assert!(pic.width() > 150 && pic.width() < 190, "{}", pic.width());
        let corner = pic.get_pixel(0, 0);
        assert_eq!(corner[3], 0, "the paper is see-through");
        assert!(pic.pixels().any(|p| p[3] == 255), "the ink is solid");
        assert!(from_strokes(&[], [0, 0, 0], 2.0).is_none());
    }

    #[test]
    fn a_scan_loses_its_paper_and_is_cut_down_to_the_ink() {
        let mut scan = image::RgbaImage::from_pixel(200, 100, image::Rgba([245, 243, 238, 255]));
        for x in 50..150 {
            for y in 48..52 {
                scan.put_pixel(x, y, image::Rgba([20, 20, 60, 255]));
            }
        }
        let kept = from_paper(&scan, 200).unwrap();
        assert!(kept.width() < 120 && kept.height() < 20, "{:?}", kept.dimensions());
        assert_eq!(kept.get_pixel(0, 0)[3], 0);
        assert!(kept.pixels().any(|p| p[3] == 255));
        let blank = image::RgbaImage::from_pixel(50, 50, image::Rgba([250, 250, 250, 255]));
        assert!(from_paper(&blank, 200).is_none());
    }

    #[test]
    fn kept_signatures_come_back_by_name_and_never_overwrite_each_other() {
        let dir = scratch("keep");
        let pic = from_strokes(&[vec![[0.0, 0.0], [10.0, 5.0]]], [0, 0, 0], 2.0).unwrap();
        let a = save_in(&dir, "Signature", &pic).unwrap();
        let b = save_in(&dir, "Signature", &pic).unwrap();
        let c = save_in(&dir, "In/itials", &pic).unwrap();
        assert_eq!(a.name, "Signature");
        assert_eq!(b.name, "Signature 2");
        assert_eq!(c.name, "In itials");
        let names: Vec<String> = saved_in(&dir).into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["In itials", "Signature", "Signature 2"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_signature_goes_down_two_inches_wide_and_its_own_shape() {
        let area = placed_at([300.0, 400.0], (600, 200));
        assert_eq!(area, [228.0, 376.0, 372.0, 424.0]);
    }
}
