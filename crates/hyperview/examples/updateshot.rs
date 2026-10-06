//! Draws the update box over a stand-in window so it can be looked at before
//! it ships, and saves what it drew.
//!
//! `cargo run --example updateshot -- <dark|light|classic> <ready|download|refused|getting|checking|answer> <notes.txt> <out.png> [width height]`
use eframe::egui;
use hyperview::updatebox::{self, Offer};

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |n: usize, or: &str| args.get(n).cloned().unwrap_or_else(|| or.to_string());
    let theme = match arg(0, "dark").as_str() {
        "light" => ui::Theme::light(),
        "classic" => ui::Theme::classic(),
        _ => ui::Theme::dark(),
    };
    let notes = std::fs::read_to_string(arg(2, "notes-0.6.15.txt")).unwrap_or_default();
    let width: f32 = arg(4, "1280").parse().unwrap_or(1280.0);
    let height: f32 = arg(5, "800").parse().unwrap_or(800.0);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([width, height]),
        ..Default::default()
    };
    eframe::run_native(
        "update",
        options,
        Box::new(move |cc| {
            ui::chrome::fonts(&cc.egui_ctx);
            theme.apply(&cc.egui_ctx);
            Ok(Box::new(Shot { theme, what: arg(1, "ready"), notes, out: arg(3, "update.png"), frame: 0 }))
        }),
    )
}

struct Shot {
    theme: ui::Theme,
    what: String,
    notes: String,
    out: String,
    frame: u32,
}

impl eframe::App for Shot {
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        let theme = self.theme;
        egui::TopBottomPanel::top("bar").frame(egui::Frame::new().fill(theme.bar).inner_margin(8)).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for word in ["File", "Edit", "View", "Markup", "Measure", "Tools", "Window", "Help"] {
                    ui.label(word);
                    ui.add_space(6.0);
                }
            });
        });
        egui::TopBottomPanel::bottom("status").frame(egui::Frame::new().fill(theme.bar).inner_margin(6)).show(ctx, |ui| {
            ui.label(egui::RichText::new("S-101  ·  1/4\" = 1'-0\"  ·  12 markups").size(11.0).color(theme.faint));
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme.surround)).show(ctx, |ui| {
            // A sheet with some linework, so the dimming behind the box reads.
            let room = ui.max_rect().shrink(40.0);
            let sheet = egui::Rect::from_center_size(room.center(), egui::vec2(room.height() * 1.4, room.height()).min(room.size()));
            let p = ui.painter();
            p.rect_filled(sheet, 0.0, egui::Color32::WHITE);
            let ink = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(60));
            for n in 1..12 {
                let x = sheet.left() + sheet.width() * n as f32 / 12.0;
                p.line_segment([egui::pos2(x, sheet.top() + 30.0), egui::pos2(x, sheet.bottom() - 60.0)], ink);
            }
            for n in 1..7 {
                let y = sheet.top() + 30.0 + (sheet.height() - 90.0) * n as f32 / 7.0;
                p.line_segment([egui::pos2(sheet.left() + 20.0, y), egui::pos2(sheet.right() - 20.0, y)], ink);
            }
            p.rect_stroke(sheet.shrink(12.0), 0.0, ink, egui::StrokeKind::Inside);
            p.rect_stroke(
                egui::Rect::from_min_size(sheet.left_top() + egui::vec2(120.0, 90.0), egui::vec2(220.0, 140.0)),
                0.0,
                egui::Stroke::new(2.5_f32, egui::Color32::from_rgb(214, 40, 40)),
                egui::StrokeKind::Middle,
            );
        });

        match self.what.as_str() {
            "getting" => {
                updatebox::getting(ctx, theme, "0.6.15");
            }
            "checking" => {
                updatebox::answer(ctx, theme, None);
            }
            "answer" => {
                updatebox::answer(ctx, theme, Some("Excalibur View 0.6.15 is the newest version. Your server is offering it to everyone."));
            }
            what => {
                let offer = match what {
                    "download" => Offer::Download { why: None },
                    "refused" => Offer::Download { why: Some("the installer's signature did not match the key this copy trusts.") },
                    _ => Offer::Ready,
                };
                updatebox::show(ctx, theme, "0.6.15", "0.6.12", &self.notes, offer);
            }
        }

        self.frame += 1;
        if self.frame == 12 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = shot {
            let [w, h] = image.size;
            let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(png) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                let _ = png.save(&self.out);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }
}
