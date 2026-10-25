//! The markups list: a bar along the bottom of the window that drags up into
//! every markup, grouped, with what each group adds up to — and out to Excel,
//! CSV, PDF, XML, JSON or HTML.
//!
//! It is always there. Closed, it is one line that still says how many
//! markups there are, what they weigh, and whether anything was left out for
//! want of a scale; that line is the number an estimator is after, and it
//! should not take a click to see it. Drag it up, or click it, and it opens.
//!
//! Grouped the way Revu groups — by subject, sheet, label, type, author,
//! status or colour — plus by drawing, across every drawing that is open.
//! Each group says its own count, length, area and weight. Every export is
//! made from exactly what the list is showing, so a number in a spreadsheet
//! can always be found again on the drawing.

use std::collections::HashSet;

use egui::{Color32, RichText};
use takeoff::export::{GroupBy, Line, Listing};
use takeoff::Column;

use crate::app::App;

/// Which markups the list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Sheet,
    Drawing,
    Everything,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::Sheet => "This sheet",
            Scope::Drawing => "This drawing",
            Scope::Everything => "Every open drawing",
        }
    }
}

/// How the list is set up. Kept for the session.
pub struct State {
    pub group: GroupBy,
    pub scope: Scope,
    pub filter: String,
    /// Groups folded shut, by name.
    pub folded: HashSet<String>,
    /// The column it is sorted by, and whether up.
    pub sort: Option<(Column, bool)>,
}

impl Default for State {
    fn default() -> State {
        State {
            group: GroupBy::Subject,
            scope: Scope::Drawing,
            filter: String::new(),
            folded: HashSet::new(),
            sort: None,
        }
    }
}

/// The formats it writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Excel,
    Csv,
    Pdf,
    Xml,
    Json,
    Html,
}

impl Format {
    pub const ALL: &'static [Format] = &[
        Format::Excel,
        Format::Csv,
        Format::Pdf,
        Format::Xml,
        Format::Json,
        Format::Html,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Format::Excel => "Excel workbook (.xlsx)",
            Format::Csv => "CSV (.csv)",
            Format::Pdf => "PDF summary (.pdf)",
            Format::Xml => "XML (.xml)",
            Format::Json => "JSON, Bluebeam-style (.json)",
            Format::Html => "Web page (.html)",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Format::Excel => "xlsx",
            Format::Csv => "csv",
            Format::Pdf => "pdf",
            Format::Xml => "xml",
            Format::Json => "json",
            Format::Html => "html",
        }
    }
}

/// Every row the list shows, owned, with where it came from.
struct Gathered {
    /// (tab, markup index, row, sheet, drawing)
    rows: Vec<(usize, usize, takeoff::Row, String, String)>,
    title: String,
}

impl App {
    fn gather(&self) -> Gathered {
        let scope = self.list.scope;
        let needle = self.list.filter.trim().to_lowercase();
        let mut rows = Vec::new();
        let tabs: Vec<usize> = match scope {
            Scope::Everything => (0..self.docs.len()).collect(),
            _ if self.docs.is_empty() => Vec::new(),
            _ => vec![self.current.min(self.docs.len() - 1)],
        };
        for tab in tabs {
            let doc = &self.docs[tab];
            let drawing = crate::app::name_of(&doc.path);
            for (index, mark) in doc.live() {
                if scope == Scope::Sheet && mark.page != doc.page {
                    continue;
                }
                let Some(row) = doc.row(index) else { continue };
                let sheet = doc.sheet_name(row.page as u32);
                if !needle.is_empty() {
                    let hay = format!(
                        "{} {} {} {} {} {}",
                        row.subject, row.label, row.comments, row.caption, row.author, sheet
                    )
                    .to_lowercase();
                    if !hay.contains(&needle) {
                        continue;
                    }
                }
                rows.push((tab, index, row, sheet, drawing.clone()));
            }
        }
        let title = match scope {
            Scope::Everything if self.docs.len() > 1 => format!("{} drawings", self.docs.len()),
            _ => self.doc().map(|d| crate::app::name_of(&d.path)).unwrap_or_default(),
        };
        Gathered { rows, title }
    }

    fn listing<'a>(&self, gathered: &'a Gathered) -> Listing<'a> {
        let mut columns: Vec<Column> = self.grid_columns().into_iter().map(|(c, _)| c).collect();
        if self.list.scope == Scope::Everything && !columns.contains(&Column::Page) {
            columns.insert(0, Column::Page);
        }
        let mut lines: Vec<Line<'a>> = gathered
            .rows
            .iter()
            .map(|(_, _, row, sheet, drawing)| Line {
                row,
                sheet: sheet.clone(),
                drawing: drawing.clone(),
            })
            .collect();
        let mut listing = Listing {
            title: gathered.title.clone(),
            who: self.author.clone(),
            when: now_text(),
            columns,
            user: crate::panelbody::user_columns(),
            weights: crate::panelbody::weights(),
            lines: Vec::new(),
            group: self.list.group,
        };
        if let Some((column, up)) = self.list.sort {
            lines.sort_by(|a, b| {
                let order = match (listing.number(a, column), listing.number(b, column)) {
                    (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                    _ => listing.cell(a, column).to_lowercase().cmp(&listing.cell(b, column).to_lowercase()),
                };
                if up { order } else { order.reverse() }
            });
        }
        listing.lines = lines;
        listing
    }

    /// The bar, and the list it opens into.
    pub fn markups_bar(&mut self, ctx: &egui::Context) {
        let theme = self.chrome.theme;
        if self.docs.is_empty() {
            return;
        }
        let gathered = self.gather();
        let listing = self.listing(&gathered);
        let totals = listing.totals();
        let open = self.show_markups;

        let mut toggle = false;
        let mut export: Option<Format> = None;
        let mut jump: Option<(usize, u32, usize)> = None;
        let mut sort_by: Option<Column> = None;
        let mut fold: Option<String> = None;

        let panel = if open {
            egui::TopBottomPanel::bottom("markups-open")
                .resizable(true)
                .default_height(280.0)
                .height_range(140.0..=(ctx.screen_rect().height() * 0.8).max(160.0))
        } else {
            egui::TopBottomPanel::bottom("markups-closed")
                .resizable(false)
                .exact_height(30.0)
        };
        panel
            .frame(crate::panels::bar_frame(theme.chrome))
            .show(ctx, |ui| {
                // The line that is always there. Dragging it up opens the list.
                let header = ui.horizontal(|ui| {
                    let (mark, chevron) =
                        ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                    chevron_glyph(ui.painter(), mark, open, theme.glyph);
                    if chevron.on_hover_text(if open { "Close the list" } else { "Open the list" }).clicked() {
                        toggle = true;
                    }
                    let title = ui.add(
                        egui::Label::new(RichText::new("Markups").strong()).sense(egui::Sense::click_and_drag()),
                    );
                    if title.clicked() {
                        toggle = true;
                    }
                    if title.dragged() && (ui.input(|i| i.pointer.delta().y) < -3.0) != open {
                        toggle = true;
                    }
                    ui.label(
                        RichText::new(format!(
                            "{} markup{}",
                            totals.markups,
                            if totals.markups == 1 { "" } else { "s" }
                        ))
                        .color(theme.faint)
                        .size(11.0),
                    );
                    if totals.weighed {
                        ui.separator();
                        ui.label(
                            RichText::new(format!("{:.3} tons", totals.tons()))
                                .strong()
                                .color(theme.accent_text),
                        );
                    }
                    if totals.unscaled > 0 {
                        ui.separator();
                        ui.label(
                            RichText::new(format!("{} left out — no scale", totals.unscaled))
                                .color(theme.warn)
                                .size(11.0),
                        )
                        .on_hover_text(
                            "Measurements on a sheet with no scale are in the list but in no \
                             total. Set the sheet's scale and they count.",
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button("Export", |ui| {
                            ui.set_min_width(230.0);
                            ui.label(
                                RichText::new("What the list is showing, as:")
                                    .color(theme.faint)
                                    .size(11.0),
                            );
                            for format in Format::ALL {
                                if ui.button(format.name()).clicked() {
                                    export = Some(*format);
                                    ui.close();
                                }
                            }
                        });
                        if open {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.list.filter)
                                    .hint_text("Filter")
                                    .desired_width(140.0),
                            );
                            egui::ComboBox::from_id_salt("markups-scope")
                                .selected_text(self.list.scope.name())
                                .show_ui(ui, |ui| {
                                    for scope in [Scope::Sheet, Scope::Drawing, Scope::Everything] {
                                        ui.selectable_value(&mut self.list.scope, scope, scope.name());
                                    }
                                });
                            egui::ComboBox::from_id_salt("markups-group")
                                .selected_text(format!("Group: {}", self.list.group.name()))
                                .show_ui(ui, |ui| {
                                    for group in GroupBy::ALL {
                                        ui.selectable_value(&mut self.list.group, *group, group.name());
                                    }
                                });
                        }
                    });
                });
                if !open {
                    header.response.on_hover_text("Click or drag up to see every markup.");
                    return;
                }
                if let Some(warning) = listing.warning() {
                    ui.label(RichText::new(warning).color(theme.warn).size(11.0));
                }
                ui.separator();

                let widths: Vec<(Column, f32)> = listing
                    .columns
                    .iter()
                    .map(|c| {
                        let from_grid = self.grid_columns().into_iter().find(|(g, _)| g == c).map(|(_, w)| w);
                        (*c, from_grid.unwrap_or(90.0))
                    })
                    .collect();

                if theme.classic {
                    // A classic list sits in a sunken white well.
                    let well = ui.available_rect_before_wrap();
                    let ppp = ui.ctx().pixels_per_point();
                    ui.painter().add(ui::classic::bevel(
                        well,
                        ui::classic::SUNKEN_EDGES,
                        Color32::WHITE,
                        ppp,
                    ));
                }
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Headings: click to sort, again to turn it round.
                        ui.horizontal(|ui| {
                            ui.add_space(18.0);
                            for (column, width) in &widths {
                                fixed(ui, *width, |ui| {
                                    let arrow = match self.list.sort {
                                        Some((c, true)) if c == *column => " ▲",
                                        Some((c, false)) if c == *column => " ▼",
                                        _ => "",
                                    };
                                    let heading = ui.add(
                                        egui::Label::new(
                                            RichText::new(format!("{}{arrow}", listing.heading(*column)))
                                                .color(theme.faint)
                                                .size(11.0),
                                        )
                                        .truncate()
                                        .sense(egui::Sense::click()),
                                    );
                                    if heading.clicked() {
                                        sort_by = Some(*column);
                                    }
                                });
                            }
                        });
                        let selected = |tab: usize, index: usize| {
                            tab == self.current && self.doc().and_then(|d| d.selected) == Some(index)
                        };
                        for (group, lines) in listing.ordered() {
                            let grouped = listing.group != GroupBy::Nothing;
                            let folded = grouped && self.list.folded.contains(&group.key);
                            if grouped {
                                let cells = listing.total_cells(&group);
                                let summary = [
                                    (!cells[2].is_empty()).then(|| format!("{} counted", cells[2])),
                                    (!cells[3].is_empty()).then(|| cells[3].clone()),
                                    (!cells[4].is_empty()).then(|| cells[4].clone()),
                                    (!cells[7].is_empty()).then(|| format!("{} t", cells[7])),
                                    (group.unscaled > 0).then(|| format!("{} no scale", group.unscaled)),
                                ]
                                .into_iter()
                                .flatten()
                                .collect::<Vec<_>>()
                                .join("  ·  ");
                                let row = ui.horizontal(|ui| {
                                    let (mark, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                                    crate::panelbody::twist(ui.painter(), mark, !folded, theme.faint);
                                    ui.label(RichText::new(&group.key).strong());
                                    ui.label(
                                        RichText::new(format!("({})", group.markups))
                                            .color(theme.faint)
                                            .size(11.0),
                                    );
                                    ui.label(RichText::new(summary).color(theme.accent_text).size(11.0));
                                });
                                if row.response.interact(egui::Sense::click()).clicked() {
                                    fold = Some(group.key.clone());
                                }
                            }
                            if folded {
                                continue;
                            }
                            for line in lines {
                                let at = listing
                                    .lines
                                    .iter()
                                    .position(|l| std::ptr::eq(l.row, line.row))
                                    .unwrap_or(0);
                                let (tab, index, ..) = &gathered.rows
                                    [gathered.rows.iter().position(|r| std::ptr::eq(&r.2, line.row)).unwrap_or(at)];
                                let chosen = selected(*tab, *index);
                                let response = ui.horizontal(|ui| {
                                    ui.add_space(18.0);
                                    for (column, width) in &widths {
                                        fixed(ui, *width, |ui| {
                                            if *column == Column::Colour {
                                                let (rect, _) = ui.allocate_exact_size(
                                                    egui::vec2(24.0, 10.0),
                                                    egui::Sense::hover(),
                                                );
                                                let c = line.row.colour;
                                                ui.painter().rect_filled(
                                                    rect,
                                                    2.0,
                                                    Color32::from_rgb(
                                                        (c[0] * 255.0) as u8,
                                                        (c[1] * 255.0) as u8,
                                                        (c[2] * 255.0) as u8,
                                                    ),
                                                );
                                                return;
                                            }
                                            let text = listing.cell(line, *column);
                                            if text.is_empty() {
                                                return;
                                            }
                                            let rich = RichText::new(&text).size(12.0);
                                            let rich = if text == "no scale" {
                                                rich.color(theme.warn)
                                            } else if chosen {
                                                rich.strong().color(theme.accent_text)
                                            } else {
                                                rich
                                            };
                                            // Not selectable: a click anywhere on
                                            // the row picks the markup.
                                            ui.add(
                                                egui::Label::new(rich)
                                                    .truncate()
                                                    .selectable(false)
                                                    .sense(egui::Sense::hover()),
                                            )
                                            .on_hover_text(&text);
                                        });
                                    }
                                });
                                if response.response.interact(egui::Sense::click()).clicked() {
                                    jump = Some((*tab, line.row.page as u32, *index));
                                }
                            }
                        }
                        // The foot: what the whole list comes to.
                        ui.separator();
                        let cells = listing.total_cells(&totals);
                        ui.horizontal(|ui| {
                            ui.add_space(18.0);
                            ui.label(RichText::new("TOTAL").strong());
                            let markups = if totals.markups == 1 { "markup" } else { "markups" };
                            for (i, name) in ["", markups, "counted", "", "", "", "lb", "tons", "no scale"].iter().enumerate() {
                                if i == 0 || cells[i].is_empty() {
                                    continue;
                                }
                                ui.separator();
                                ui.label(RichText::new(format!("{} {name}", cells[i]).trim().to_string()).strong().size(12.0));
                            }
                        });
                    });
            });

        if toggle {
            self.show_markups = !self.show_markups;
        }
        if let Some(column) = sort_by {
            self.list.sort = match self.list.sort {
                Some((c, true)) if c == column => Some((c, false)),
                Some((c, false)) if c == column => None,
                _ => Some((column, true)),
            };
        }
        if let Some(key) = fold {
            if !self.list.folded.remove(&key) {
                self.list.folded.insert(key);
            }
        }
        if let Some((tab, page, index)) = jump {
            if tab != self.current && tab < self.docs.len() {
                self.current = tab;
                self.retitle(ctx);
            }
            self.go_to(page);
            if let Some(doc) = self.doc_mut() {
                doc.selected = Some(index);
            }
        }
        if let Some(format) = export {
            drop(listing);
            self.export_list(format, &gathered);
        }
    }

    fn export_list(&mut self, format: Format, gathered: &Gathered) {
        if format == Format::Pdf {
            // The printed summary has its own layout, made for paper.
            self.export_report();
            return;
        }
        let listing = self.listing(gathered);
        if listing.lines.is_empty() {
            self.status = "There is nothing in the list to export.".into();
            return;
        }
        let stem = listing.title.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let suggested = format!("{stem} takeoff.{}", format.extension());
        // Written now, while the list is to hand, and moved to wherever it is
        // wanted once that has been chosen.
        let staged = std::env::temp_dir().join(format!(
            "excalibur-view-export-{}-{}.{}",
            std::process::id(),
            listing.lines.len(),
            format.extension()
        ));
        let written = match format {
            Format::Csv => std::fs::write(&staged, listing.to_csv()).map_err(|e| e.to_string()),
            Format::Xml => std::fs::write(&staged, listing.to_xml()).map_err(|e| e.to_string()),
            Format::Html => std::fs::write(&staged, listing.to_html()).map_err(|e| e.to_string()),
            Format::Json => serde_json::to_string_pretty(&listing.to_json())
                .map_err(|e| e.to_string())
                .and_then(|text| std::fs::write(&staged, text).map_err(|e| e.to_string())),
            Format::Excel => crate::workbook::write(&listing, &staged),
            Format::Pdf => Ok(()),
        };
        if let Err(why) = written {
            self.error = Some(format!("Could not write the list: {why}"));
            return;
        }
        let count = listing.lines.len();
        self.filing.one(
            crate::files::Choose::save(suggested).filter(format.name(), &[format.extension()]),
            move |app, path| {
                let moved = std::fs::copy(&staged, &path);
                let _ = std::fs::remove_file(&staged);
                match moved {
                    Ok(_) => app.status = format!("{count} markups written to {}.", path.display()),
                    Err(why) => app.error = Some(format!("Could not write {}: {why}", path.display())),
                }
            },
        );
    }
}

/// A cell exactly `width` wide, whatever is in it, so the columns line up.
fn fixed(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 18.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(width);
            ui.set_max_width(width);
            add(ui);
        },
    );
}

fn chevron_glyph(painter: &egui::Painter, rect: egui::Rect, open: bool, colour: Color32) {
    let c = rect.center();
    let (a, b) = if open { (4.0, -4.0) } else { (-4.0, 4.0) };
    let points = vec![
        egui::pos2(c.x - 5.0, c.y + b / 2.0 - a / 4.0),
        egui::pos2(c.x, c.y + a / 2.0 - b / 4.0 - 1.0),
        egui::pos2(c.x + 5.0, c.y + b / 2.0 - a / 4.0),
    ];
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.6_f32, colour))); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
}

fn now_text() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Days from the epoch to a date, the civil-calendar way.
    let days = (now / 86_400) as i64;
    let (h, m) = ((now % 86_400) / 3600, (now % 3600) / 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02} UTC")
}
