//! Printing: Hyperview's own Print dialog, straight to the printer.
//!
//! Ctrl+P opens it over the drawing. The printers, their paper sizes and their
//! own Properties dialog are Windows'; the sheets are drawn onto the printer by
//! the PDF engine, markups and all, as vectors. Nothing else is opened — not a
//! second window, not somebody else's PDF program.

use std::path::PathBuf;

use crossbeam_channel::Receiver;

use crate::app::App;
use crate::winprint::{self, Fit, Orientation, PaperSize};

#[derive(Debug)]
pub struct Printing {
    pub which: Which,
    pub range: String,
    /// The sheets picked in the Thumbnails panel when the window opened, and
    /// their numbers, for saying which they are.
    pub picked: Vec<u32>,
    pub picked_names: String,
    pub error: Option<String>,
    /// The sheets are being got ready (markups and all) before they go.
    pub waiting: bool,
    /// Sheets sent so far, of how many, once the printer has them.
    pub sending: Option<(usize, usize)>,
    pub printers: Vec<String>,
    pub printer: String,
    pub papers: Vec<PaperSize>,
    pub paper: Option<i16>,
    /// The printer's settings as its driver keeps them — colour, quality and
    /// everything its own Properties dialog sets — with the choices here
    /// written in on the way out.
    pub settings: Vec<u8>,
    pub fit: Fit,
    pub orientation: Orientation,
    pub copies: u16,
    pub collate: bool,
    /// Printers are asked about off the window's thread: a network printer
    /// that is switched off can take a while to say so.
    pub asking: Option<Receiver<Found>>,
}

/// What asking Windows about printers came to.
#[derive(Debug)]
pub struct Found {
    pub printers: Option<(Vec<String>, Option<String>)>,
    pub printer: String,
    pub papers: Vec<PaperSize>,
    pub settings: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Which {
    ThisSheet,
    Everything,
    Some,
    /// The sheets picked in the Thumbnails panel.
    Picked,
}

/// What was chosen last time, so the dialog opens on it.
#[derive(Clone, Debug, Default)]
pub struct LastChoice {
    pub printer: String,
    pub settings: Vec<u8>,
    pub paper: Option<i16>,
    pub fit: Fit,
    pub orientation: Orientation,
}

impl Printing {
    pub fn new(last: &LastChoice) -> Printing {
        let mut printing = Printing {
            which: Which::ThisSheet,
            range: String::new(),
            picked: Vec::new(),
            picked_names: String::new(),
            error: None,
            waiting: false,
            sending: None,
            printers: Vec::new(),
            printer: last.printer.clone(),
            papers: Vec::new(),
            paper: last.paper,
            settings: last.settings.clone(),
            fit: last.fit,
            orientation: last.orientation,
            copies: 1,
            collate: true,
            asking: None,
        };
        printing.ask(true);
        printing
    }

    /// Asks Windows for the printers (the first time) and for the chosen
    /// printer's papers and settings.
    fn ask(&mut self, and_the_list: bool) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let wanted = self.printer.clone();
        std::thread::Builder::new()
            .name("hyperview-printers".into())
            .spawn(move || {
                let list = and_the_list.then(winprint::printers);
                let printer = if wanted.is_empty() {
                    list.as_ref()
                        .and_then(|(all, default)| default.clone().or_else(|| all.first().cloned()))
                        .unwrap_or_default()
                } else {
                    wanted
                };
                let (papers, settings) = if printer.is_empty() {
                    (Vec::new(), Vec::new())
                } else {
                    (
                        winprint::paper_sizes(&printer),
                        winprint::settings(&printer).unwrap_or_default(),
                    )
                };
                let _ = tx.send(Found { printers: list, printer, papers, settings });
            })
            .ok();
        self.asking = Some(rx);
    }

    fn take_answer(&mut self) {
        let Some(found) = self.asking.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return;
        };
        self.asking = None;
        if let Some((all, _)) = found.printers {
            self.printers = all;
        }
        if self.printer != found.printer || self.settings.is_empty() {
            self.settings = found.settings;
        }
        self.printer = found.printer;
        self.papers = found.papers;
        let offered = |id: i16| self.papers.iter().any(|p| p.id == id);
        if !self.paper.is_some_and(offered) {
            self.paper = winprint::paper_of(&self.settings).filter(|id| offered(*id));
        }
    }

    fn paper_size(&self) -> Option<&PaperSize> {
        self.paper.and_then(|id| self.papers.iter().find(|p| p.id == id))
    }

    /// The settings with this dialog's choices in them.
    fn outgoing(&self) -> Vec<u8> {
        winprint::with_choices(&self.settings, self.paper, self.orientation, self.copies, self.collate)
    }
}

pub fn read_range(text: &str, sheets: usize) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for part in text.split([',', ';']) {
        let part = part.trim().replace(['\u{2013}', '\u{2014}'], "-");
        if part.is_empty() {
            continue;
        }
        match part.split_once('-') {
            Some((from, to)) => {
                let (Ok(from), Ok(to)) = (from.trim().parse::<usize>(), to.trim().parse::<usize>())
                else {
                    continue;
                };
                let (low, high) = if from <= to { (from, to) } else { (to, from) };
                for n in low..=high {
                    if n >= 1 && n <= sheets {
                        out.push(n as u32 - 1);
                    }
                }
            }
            None => {
                if let Ok(n) = part.parse::<usize>() {
                    if n >= 1 && n <= sheets {
                        out.push(n as u32 - 1);
                    }
                }
            }
        }
    }
    out.dedup();
    out
}

/// Hands a file to whatever the person prints with.
///
/// Windows is asked for the "print" verb, which is the same thing that happens
/// when somebody right-clicks a PDF in Explorer and chooses Print: their reader
/// opens with their print dialog, with their printers in it. If no reader is
/// registered for printing, the file is opened instead and they can press
/// Ctrl-P — better than a dead button and a shrug.
pub fn send_to_printer(path: &std::path::Path) -> Result<bool, String> {
    // A tablet prints the way everything on it prints: the system's own
    // printing, with its own list of printers.
    let name = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if crate::platform::print(path, &name) {
        return Ok(true);
    }
    #[cfg(windows)]
    {
        let quoted = path.display().to_string().replace('\'', "''");
        let printed = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &format!("Start-Process -FilePath '{quoted}' -Verb Print"),
            ])
            .status();
        if matches!(&printed, Ok(s) if s.success()) {
            return Ok(true);
        }
        let opened = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &format!("Start-Process -FilePath '{quoted}'"),
            ])
            .status()
            .map_err(|e| e.to_string())?;
        if opened.success() {
            return Ok(false);
        }
        Err("Windows would not open the sheets to print.".into())
    }
    #[cfg(not(windows))]
    {
        // lp where there is a print system, and failing that whatever opens a
        // PDF, so this is useful on a Linux box too rather than a stub.
        if std::process::Command::new("lp")
            .arg(path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Ok(true);
        }
        let opened = std::process::Command::new("xdg-open")
            .arg(path)
            .status()
            .map_err(|e| e.to_string())?;
        if opened.success() {
            Ok(false)
        } else {
            Err("Nothing on this system would open the sheets to print.".into())
        }
    }
}

impl App {
    pub fn begin_print(&mut self) {
        if self.doc().is_none() {
            self.status = "Open a drawing first.".into();
            return;
        }
        let mut printing = Printing::new(&self.last_print);
        // Sheets picked in the Thumbnails panel are what somebody means to
        // print, so the window opens on them.
        if let Some(doc) = self.doc() {
            if let Some(picked) = doc.picks.offered(doc.page) {
                let numbers: Vec<String> = picked
                    .iter()
                    .map(|p| doc.labels.get(*p as usize).map(|l| l.number.clone()).unwrap_or_default())
                    .collect();
                printing.picked_names = crate::picks::in_words(&numbers);
                printing.picked = picked;
                printing.which = Which::Picked;
            }
        }
        self.printing = Some(printing);
    }

    pub fn print_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut printing) = self.printing.take() else {
            return;
        };
        printing.take_answer();
        if printing.asking.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let theme = self.chrome.theme;
        let Some(doc) = self.doc() else { return };
        let sheets = doc.pages.len();
        let here = doc.page as usize + 1;
        let sheet_size = doc
            .pages
            .get(doc.page as usize)
            .map(|p| (p.width, p.height))
            .unwrap_or((612.0, 792.0));
        let preview = doc.previews.get(&doc.page).map(|(_, t)| t.id());
        let title = doc
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Drawing".into());

        let mut keep = true;
        let mut go = false;
        let mut cancel_sending = false;
        let mut properties = false;
        let mut switch_to: Option<String> = None;
        let busy = printing.waiting || printing.sending.is_some();

        egui::Window::new("Print")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .max_width(860.0)
            .show(ctx, |ui| {
                ui.set_min_width(620.0);
                ui.horizontal_top(|ui| {
                    // ---- the choices
                    ui.vertical(|ui| {
                        ui.set_width(330.0);
                        ui.label(egui::RichText::new(&title).strong());
                        ui.add_space(8.0);

                        egui::Grid::new("print-choices")
                            .num_columns(2)
                            .spacing([10.0, 8.0])
                            .show(ui, |ui| {
                                ui.label("Printer");
                                ui.horizontal(|ui| {
                                    let shown = if printing.printer.is_empty() {
                                        "Looking for printers…".to_string()
                                    } else {
                                        printing.printer.clone()
                                    };
                                    egui::ComboBox::from_id_salt("printer")
                                        .width(190.0)
                                        .selected_text(shown)
                                        .show_ui(ui, |ui| {
                                            for name in &printing.printers {
                                                if ui
                                                    .selectable_label(*name == printing.printer, name)
                                                    .clicked()
                                                {
                                                    switch_to = Some(name.clone());
                                                }
                                            }
                                        });
                                    if ui
                                        .add_enabled(
                                            !printing.printer.is_empty() && cfg!(windows),
                                            egui::Button::new("Properties…").small(),
                                        )
                                        .on_hover_text("The printer's own settings: colour, quality, tray.")
                                        .clicked()
                                    {
                                        properties = true;
                                    }
                                });
                                ui.end_row();

                                ui.label("Paper");
                                let shown = printing
                                    .paper_size()
                                    .map(|p| p.name.clone())
                                    .unwrap_or_else(|| "The printer's own".into());
                                egui::ComboBox::from_id_salt("paper")
                                    .width(250.0)
                                    .selected_text(shown)
                                    .show_ui(ui, |ui| {
                                        for paper in &printing.papers {
                                            let (w, h) = paper.inches();
                                            let label = format!("{}  ({w:.1} × {h:.1} in)", paper.name);
                                            if ui
                                                .selectable_label(printing.paper == Some(paper.id), label)
                                                .clicked()
                                            {
                                                printing.paper = Some(paper.id);
                                            }
                                        }
                                    });
                                ui.end_row();

                                ui.label("Orientation");
                                ui.horizontal(|ui| {
                                    ui.radio_value(&mut printing.orientation, Orientation::Auto, "Auto");
                                    ui.radio_value(&mut printing.orientation, Orientation::Portrait, "Portrait");
                                    ui.radio_value(&mut printing.orientation, Orientation::Landscape, "Landscape");
                                });
                                ui.end_row();

                                ui.label("Size");
                                ui.horizontal(|ui| {
                                    ui.radio_value(&mut printing.fit, Fit::ToPaper, "Fit to paper");
                                    ui.radio_value(&mut printing.fit, Fit::Actual, "Actual size");
                                });
                                ui.end_row();

                                ui.label("Copies");
                                ui.horizontal(|ui| {
                                    ui.add(egui::DragValue::new(&mut printing.copies).range(1..=999));
                                    ui.checkbox(&mut printing.collate, "Collate");
                                });
                                ui.end_row();
                            });

                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("Sheets").color(theme.faint).size(11.0));
                        if !printing.picked.is_empty() {
                            let n = printing.picked.len();
                            ui.radio_value(
                                &mut printing.which,
                                Which::Picked,
                                format!("The {n} sheet{} picked", if n == 1 { "" } else { "s" }),
                            );
                            let names = if printing.picked_names.is_empty() {
                                format!("Sheets {}", crate::picks::as_range(&printing.picked))
                            } else {
                                printing.picked_names.clone()
                            };
                            ui.horizontal(|ui| {
                                ui.add_space(24.0);
                                ui.label(egui::RichText::new(names).color(theme.faint).size(10.0));
                            });
                        }
                        ui.radio_value(
                            &mut printing.which,
                            Which::ThisSheet,
                            format!("This sheet ({here} of {sheets})"),
                        );
                        ui.radio_value(&mut printing.which, Which::Everything, format!("All {sheets} sheets"));
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut printing.which, Which::Some, "Sheets");
                            let box_ = ui.add(
                                egui::TextEdit::singleline(&mut printing.range)
                                    .hint_text("3, 7, 12-18")
                                    .desired_width(150.0),
                            );
                            if box_.changed() {
                                printing.which = Which::Some;
                                printing.error = None;
                            }
                        });
                        if printing.which == Which::Some && !printing.range.trim().is_empty() {
                            let picked = read_range(&printing.range, sheets);
                            ui.label(
                                egui::RichText::new(if picked.is_empty() {
                                    format!("Nothing in that range is in this drawing (1 to {sheets}).")
                                } else {
                                    format!("{} sheet{}.", picked.len(), if picked.len() == 1 { "" } else { "s" })
                                })
                                .color(theme.faint)
                                .size(10.0),
                            );
                        }
                    });

                    ui.add_space(14.0);

                    // ---- what it will look like
                    ui.vertical(|ui| {
                        let paper_in = printing
                            .paper_size()
                            .map(|p| p.inches())
                            .unwrap_or((8.5, 11.0));
                        let landscape = match printing.orientation {
                            Orientation::Landscape => true,
                            Orientation::Portrait => false,
                            Orientation::Auto => paper_in.0 > paper_in.1,
                        };
                        let (pw, ph) = if landscape == (paper_in.0 > paper_in.1) {
                            paper_in
                        } else {
                            (paper_in.1, paper_in.0)
                        };
                        paper_preview(ui, theme, (pw, ph), sheet_size, printing.fit, printing.orientation, preview);
                        ui.label(
                            egui::RichText::new(format!("{pw:.1} × {ph:.1} in paper"))
                                .color(theme.faint)
                                .size(10.0),
                        );
                    });
                });

                if let Some(error) = &printing.error {
                    ui.add_space(8.0);
                    ui.colored_label(egui::Color32::from_rgb(235, 120, 120), error);
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let ready = !busy
                        && (!printing.printer.is_empty() || !cfg!(windows))
                        && match printing.which {
                            Which::Some => !read_range(&printing.range, sheets).is_empty(),
                            Which::Picked => !printing.picked.is_empty(),
                            _ => true,
                        };
                    let print = ui.add_enabled(ready, egui::Button::new("Print"));
                    if print.clicked()
                        || (ready && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        go = true;
                    }
                    if printing.sending.is_some() {
                        if ui.button("Stop").clicked() {
                            cancel_sending = true;
                        }
                    } else if ui.button("Cancel").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        keep = false;
                    }
                    if printing.waiting {
                        ui.add_space(8.0);
                        ui.spinner();
                        ui.label(egui::RichText::new("Getting the sheets ready…").color(theme.faint).size(11.0));
                    }
                    if let Some((done, of)) = printing.sending {
                        ui.add_space(8.0);
                        ui.spinner();
                        ui.label(
                            egui::RichText::new(format!("Sending sheet {} of {of} to {}…", (done + 1).min(of), printing.printer))
                                .color(theme.faint)
                                .size(11.0),
                        );
                    }
                });
            });

        if let Some(name) = switch_to {
            if name != printing.printer {
                printing.printer = name;
                printing.settings.clear();
                printing.papers.clear();
                printing.paper = None;
                printing.ask(false);
            }
        }
        if properties {
            if let Some(changed) = winprint::properties(&printing.printer, &printing.outgoing()) {
                printing.paper = winprint::paper_of(&changed).or(printing.paper);
                printing.settings = changed;
            }
        }
        if cancel_sending {
            self.svc.send(crate::render::ToWorker::CancelPrint(self.print_job));
            printing.sending = None;
            printing.waiting = false;
            self.status = "Printing stopped.".into();
        }
        if go {
            let pages = match printing.which {
                Which::ThisSheet => vec![here as u32 - 1],
                Which::Everything => (0..sheets as u32).collect(),
                Which::Some => read_range(&printing.range, sheets),
                Which::Picked => printing
                    .picked
                    .iter()
                    .copied()
                    .filter(|p| (*p as usize) < sheets)
                    .collect(),
            };
            printing.waiting = true;
            printing.error = None;
            self.last_print = LastChoice {
                printer: printing.printer.clone(),
                settings: printing.settings.clone(),
                paper: printing.paper,
                fit: printing.fit,
                orientation: printing.orientation,
            };
            winprint::remember(&printing.printer, &printing.outgoing());
            self.start_printing(pages);
        }
        if keep {
            self.printing = Some(printing);
        }
    }

    fn start_printing(&mut self, pages: Vec<u32>) {
        self.save_now();
        let Some(doc) = self.doc() else { return };
        let id = doc.id;
        let stem = doc
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "drawing".into());
        self.print_job += 1;
        let job = self.print_job;
        let to: PathBuf = std::env::temp_dir().join(format!("{stem} (to print) {job}.pdf"));
        self.svc.send(crate::render::ToWorker::Extract { doc: id, job, pages, to });
    }

    /// The sheets are ready, markups and all. Straight to the printer.
    pub fn sheets_ready_to_print(&mut self, job: u64, to: PathBuf, pages: usize) {
        if job != self.print_job {
            // Somebody pressed Print twice and changed their mind in between.
            let _ = std::fs::remove_file(&to);
            return;
        }
        let Some(printing) = self.printing.as_mut() else {
            let _ = std::fs::remove_file(&to);
            return;
        };
        if !cfg!(windows) {
            self.printing = None;
            match send_to_printer(&to) {
                Ok(true) => self.status = format!("{pages} sheet(s) sent to print."),
                Ok(false) => self.status = "Nothing on this system would print them.".into(),
                Err(why) => self.error = Some(why),
            }
            return;
        }
        printing.waiting = false;
        printing.sending = Some((0, pages));
        let title = to
            .file_stem()
            .map(|s| s.to_string_lossy().replace(" (to print)", "").to_string())
            .unwrap_or_else(|| "Excalibur View".into());
        self.svc.send(crate::render::ToWorker::PrintFile {
            job,
            path: to,
            printer: printing.printer.clone(),
            settings: printing.outgoing(),
            fit: printing.fit,
            orientation: printing.orientation,
            title,
        });
    }

    pub fn print_progress(&mut self, job: u64, done: usize, of: usize) {
        if job != self.print_job {
            return;
        }
        if let Some(printing) = self.printing.as_mut() {
            printing.sending = Some((done, of));
        }
    }

    pub fn printed(&mut self, job: u64, sheets: usize, printer: String) {
        if job != self.print_job {
            return;
        }
        self.printing = None;
        self.status = format!(
            "{sheets} sheet{} sent to {printer}.",
            if sheets == 1 { "" } else { "s" }
        );
    }

    pub fn printing_failed(&mut self, job: u64, why: String) {
        if job != self.print_job {
            return;
        }
        if let Some(printing) = self.printing.as_mut() {
            printing.waiting = false;
            printing.sending = None;
            printing.error = Some(format!("That did not print: {why}"));
        } else {
            self.error = Some(why);
        }
    }
}

/// The paper, with the sheet on it where it will land.
fn paper_preview(
    ui: &mut egui::Ui,
    theme: ui::chrome::Theme,
    paper_in: (f32, f32),
    sheet: (f32, f32),
    fit: Fit,
    orientation: Orientation,
    picture: Option<egui::TextureId>,
) {
    let room = egui::vec2(250.0, 250.0);
    let scale = (room.x / paper_in.0).min(room.y / paper_in.1);
    let size = egui::vec2(paper_in.0 * scale, paper_in.1 * scale);
    let (outer, _) = ui.allocate_exact_size(room, egui::Sense::hover());
    let paper_rect = egui::Rect::from_center_size(outer.center(), size);
    let painter = ui.painter_at(outer);
    painter.rect_filled(paper_rect.translate(egui::vec2(2.0, 2.0)), 0.0, egui::Color32::from_black_alpha(90));
    painter.rect_filled(paper_rect, 0.0, egui::Color32::WHITE);

    // The same placement the printer will use, at a preview's resolution.
    let dpi = 100;
    let px = |inches: f32| (inches * dpi as f32).round() as i32;
    let paper = winprint::Paper {
        printable: (px(paper_in.0), px(paper_in.1)),
        physical: (px(paper_in.0), px(paper_in.1)),
        offset: (0, 0),
        dpi: (dpi, dpi),
    };
    let at = winprint::place(sheet, paper, fit, orientation);
    let k = size.x / paper.physical.0 as f32;
    let sheet_rect = egui::Rect::from_min_size(
        paper_rect.min + egui::vec2(at.x as f32 * k, at.y as f32 * k),
        egui::vec2(at.w as f32 * k, at.h as f32 * k),
    );
    let clip = painter.with_clip_rect(paper_rect);
    match picture {
        Some(texture) => {
            let mut mesh = egui::Mesh::with_texture(texture);
            let corners = [
                sheet_rect.left_top(),
                sheet_rect.right_top(),
                sheet_rect.right_bottom(),
                sheet_rect.left_bottom(),
            ];
            // Turned a quarter clockwise, the sheet's left edge runs along
            // the top of the paper.
            let uvs = if at.rotate == 1 {
                [egui::pos2(0.0, 1.0), egui::pos2(0.0, 0.0), egui::pos2(1.0, 0.0), egui::pos2(1.0, 1.0)]
            } else {
                [egui::pos2(0.0, 0.0), egui::pos2(1.0, 0.0), egui::pos2(1.0, 1.0), egui::pos2(0.0, 1.0)]
            };
            for (p, uv) in corners.iter().zip(uvs) {
                mesh.vertices.push(egui::epaint::Vertex { pos: *p, uv, color: egui::Color32::WHITE });
            }
            mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
            clip.add(egui::Shape::mesh(mesh));
        }
        None => {
            clip.rect_filled(sheet_rect, 0.0, egui::Color32::from_gray(235));
        }
    }
    clip.rect_stroke(sheet_rect, 0.0, egui::Stroke::new(1.0_f32, theme.accent), egui::StrokeKind::Inside); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
    painter.rect_stroke(paper_rect, 0.0, egui::Stroke::new(1.0_f32, egui::Color32::from_gray(90)), egui::StrokeKind::Outside); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn people_write_ranges_the_way_people_write_ranges() {
        assert_eq!(read_range("3", 20), vec![2]);
        assert_eq!(read_range("1,2,3", 20), vec![0, 1, 2]);
        assert_eq!(read_range(" 3 , 7 ", 20), vec![2, 6]);
        assert_eq!(read_range("2-5", 20), vec![1, 2, 3, 4]);
        // An en dash, because that is what a pasted email contains.
        assert_eq!(read_range("2\u{2013}4", 20), vec![1, 2, 3]);
        // Backwards, because somebody typed it that way.
        assert_eq!(read_range("5-2", 20), vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_range_past_the_end_prints_what_is_there_and_not_what_is_not() {
        // Not an error, and not a hundred blank pages either.
        assert_eq!(read_range("18-25", 20), vec![17, 18, 19]);
        assert_eq!(read_range("50", 20), Vec::<u32>::new());
        assert_eq!(read_range("0", 20), Vec::<u32>::new());
    }

    #[test]
    fn nonsense_is_dropped_rather_than_printed() {
        assert_eq!(read_range("", 20), Vec::<u32>::new());
        assert_eq!(read_range("all of them please", 20), Vec::<u32>::new());
        assert_eq!(read_range("-", 20), Vec::<u32>::new());
        // And the good parts of a half-typed range still work.
        assert_eq!(read_range("3, banana, 5", 20), vec![2, 4]);
    }

    #[test]
    fn sheet_numbers_are_one_based_on_the_screen_and_zero_based_underneath() {
        // The estimator types 1 for the first sheet. Everything inside counts
        // from nought, and exactly one place converts between them: here.
        assert_eq!(read_range("1", 5), vec![0]);
        assert_eq!(read_range("5", 5), vec![4]);
    }
}
