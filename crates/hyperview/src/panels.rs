//! The window, laid out the way his Revu profile lays it out.

use egui::{Color32, RichText};

use crate::app::{App, Tool};
use annot::measure::Measure;

impl App {
    // ---- menus and toolbars ---------------------------------------------

    pub fn menus(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menus")
            .frame(bar_frame(self.chrome.theme.chrome))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    self.chrome.menu_bar(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.account(ui);
                        if let Some(doc) = self.doc() {
                            ui.label(
                                RichText::new(crate::app::name_of(&doc.path))
                                    .color(self.chrome.theme.faint),
                            );
                        }
                    });
                });
            });
    }

    /// Who is signed in, in the corner of the menu row - the one place a
    /// person looks to answer "am I on the office server, and as whom".
    ///
    /// It is quiet when there is nothing to say. The free program, used alone
    /// with no server ever set up, shows nothing at all rather than an empty
    /// account waiting to be filled in.
    fn account(&mut self, ui: &mut egui::Ui) {
        let theme = self.chrome.theme;
        match self.standing.who.clone() {
            Some(who) => {
                let office = if self.standing.name.is_empty() {
                    String::new()
                } else {
                    format!("  ·  {}", self.standing.name)
                };
                let label = ui.add(
                    egui::Label::new(
                        RichText::new(format!("{}{office}", who.name)).color(theme.text),
                    )
                    .sense(egui::Sense::click()),
                );
                let role = match who.role {
                    hub::Role::Admin => "administrator",
                    hub::Role::Estimator => "estimator",
                    hub::Role::Viewer => "viewer, reading only",
                };
                let where_from = self.standing.base.trim_end_matches('/');
                if label
                    .on_hover_text(format!("{}\n{role}\n{where_from}", who.email))
                    .clicked()
                {
                    self.chrome.fire("Server.Connect");
                }
            }
            // Set up on a server before, but not signed in to it now - the
            // week-old session, the laptop that woke up somewhere else.
            None if !self.prefs.server.base.is_empty() => {
                if ui
                    .add(
                        egui::Label::new(RichText::new("Sign in").color(theme.accent_text))
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_text(format!(
                        "Not signed in to {}",
                        self.prefs.server.base.trim_end_matches('/')
                    ))
                    .clicked()
                {
                    self.chrome.fire("Server.Connect");
                }
            }
            None => {}
        }
        ui.add_space(6.0);
    }

    /// Whether simple mode is on for this computer.
    pub fn simple_mode(&self) -> bool {
        self.prefs.simple_on(crate::platform::tablet()) // WHAT: as chosen, or on for a tablet's first run. WHY: the toolbars took five rows of an iPad's screen.
    }

    /// Simple mode's one bar in place of the toolbars: open, select and pan,
    /// the measuring and markup tools behind two buttons, the scale, undo and
    /// redo, the pages, and the way back to the full toolbars.
    fn simple_bar(&mut self, ctx: &egui::Context) {
        let theme = self.chrome.theme; // WHAT: the colours in use. WHY: matches the toolbars it stands in for.
        egui::TopBottomPanel::top("simple-bar")
            .frame(bar_frame(theme.bar)) // WHAT: the toolbars' own frame. WHY: one look.
            .show(ctx, |ui| {
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0; // WHAT: a little more room between buttons than the toolbars have. WHY: easier for a finger.
                    self.chrome.button(ui, "File.Open"); // WHAT: open a drawing. WHY: the first thing anybody does.
                    ui.separator();
                    for id in ["Select", "Pan"] {
                        self.chrome.button(ui, id); // WHAT: pick things up, or move round. WHY: the two ways of getting about.
                    }
                    ui.separator();
                    let measuring = ui::menu::MEASURE.iter().any(|e| matches!(e, ui::menu::Entry::Item(id) | ui::menu::Entry::Named(id, _) if *id == self.chrome.tool)); // WHAT: a measuring tool is in hand. WHY: the Measure button says which kind of tool is out.
                    ui.menu_button(if measuring { RichText::new("Measure ▾").strong() } else { RichText::new("Measure ▾") }, |ui| {
                        ui.set_min_width(220.0); // WHAT: wide enough for the names. WHY: a menu that wraps is hard to read.
                        self.chrome.menu_entries(ui, ui::menu::MEASURE); // WHAT: every measuring tool, Set Scale first. WHY: the same list as the Measure menu.
                    });
                    self.scale_box(ui); // WHAT: the sheet's scale, and setting it. WHY: no measuring without it.
                    let marking = ui::menu::MARKUP.iter().any(|e| matches!(e, ui::menu::Entry::Item(id) if *id == self.chrome.tool)); // WHAT: a markup tool is in hand. WHY: the same for the Markup button.
                    ui.menu_button(if marking { RichText::new("Markup ▾").strong() } else { RichText::new("Markup ▾") }, |ui| {
                        ui.set_min_width(220.0); // WHAT: as above. WHY: as above.
                        self.chrome.menu_entries(ui, ui::menu::MARKUP); // WHAT: every markup tool. WHY: the same list as the Markup menu.
                    });
                    ui.separator();
                    for id in ["Edit.Undo", "Edit.Redo"] {
                        self.chrome.button(ui, id); // WHAT: undo and redo. WHY: a tablet has no Ctrl+Z.
                    }
                    ui.separator();
                    for id in ["View.PageFirst", "View.PagePrevious"] {
                        self.chrome.button(ui, id); // WHAT: back through the sheets. WHY: the bottom bar is put away in simple mode.
                    }
                    self.page_box(ui); // WHAT: which sheet, and going to one by number. WHY: the same.
                    for id in ["View.PageNext", "View.PageLast"] {
                        self.chrome.button(ui, id); // WHAT: on through the sheets. WHY: the same.
                    }
                    ui.separator();
                    self.chrome.button(ui, "View.FitPage"); // WHAT: the whole sheet on the screen. WHY: the way back after zooming in too far.
                    ui.separator();
                    if ui
                        .button("Full toolbars")
                        .on_hover_text("Leave simple mode and show every toolbar (View, Simple Mode)")
                        .clicked()
                    {
                        self.chrome.set("View.SimpleMode", false); // WHAT: the switch shows off. WHY: the View menu's tick follows.
                        self.chrome.fire("View.SimpleMode"); // WHAT: handled where the View menu's switch is. WHY: one place keeps the setting.
                    }
                });
                ui.add_space(2.0);
            });
    }

    pub fn tool_bars(&mut self, ctx: &egui::Context) {
        let theme = self.chrome.theme;
        if self.simple_mode() {
            self.simple_bar(ctx); // WHAT: the short bar instead. WHY: simple mode.
            return;
        }
        egui::TopBottomPanel::top("toolbars")
            .frame(bar_frame(theme.bar))
            .show(ctx, |ui| {
                ui.add_space(2.0);
                // The profile's own rows when it has them. A seat with no
                // profile, or with a tool set on its own (a .btx carries tools
                // and no toolbars), gets Hyperview's standard rows — every
                // tool, never an empty bar or a cut-down one.
                match self.profile.take() {
                    Some(profile) if ui::chrome::has_toolbars(&profile.toolbars) => {
                        let bars = ui::chrome::completed(&profile.toolbars);
                        self.chrome.toolbars(ui, &bars);
                        self.profile = Some(profile);
                    }
                    other => {
                        self.profile = other;
                        let standard = self.standard_bars.get_or_insert_with(ui::chrome::standard_toolbars).clone();
                        self.chrome.toolbars(ui, &standard);
                    }
                }
                ui.add_space(2.0);
            });
    }

    // ---- the bottom bar --------------------------------------------------

    pub fn navigation(&mut self, ctx: &egui::Context) {
        if self.simple_mode() {
            return; // WHAT: no bottom bar in simple mode. WHY: its pages and scale are on the simple bar, and the drawing gets the room.
        }
        let theme = self.chrome.theme;
        egui::TopBottomPanel::bottom("navigation")
            .frame(bar_frame(theme.bar))
            .show(ctx, |ui| {
                ui.add_space(1.0);
                ui.horizontal(|ui| {
                    let split = self.doc().map(|d| d.split).unwrap_or_default();
                    self.chrome.set("View.Split", split == crate::sheet::Split::Vertical);
                    self.chrome
                        .set("View.SplitHorizontal", split == crate::sheet::Split::Horizontal);
                    self.chrome.set("View.UnSplit", !split.on());
                    for id in ["View.UnSplit", "View.Split", "View.SplitHorizontal"] {
                        self.chrome.button(ui, id);
                    }
                    ui.separator();
                    for id in ["Select", "Pan", "Zoom", "Measure.Tool"] {
                        self.chrome.button(ui, id);
                    }
                    ui.separator();
                    self.scale_box(ui);
                    ui.separator();
                    for id in ["View.PageFirst", "View.PagePrevious"] {
                        self.chrome.button(ui, id);
                    }
                    self.page_box(ui);
                    for id in ["View.PageNext", "View.PageLast"] {
                        self.chrome.button(ui, id);
                    }
                    ui.separator();
                    for id in ["View.FitPage", "View.FitWidth", "View.ActualSize"] {
                        self.chrome.button(ui, id);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Which version this is, where anybody asked can read it
                        // off without opening a menu.
                        ui.label(
                            RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                                .color(theme.faint)
                                .size(10.0),
                        )
                        .on_hover_text("Excalibur View's version. Help → About says more.");
                        self.chrome.button(ui, "View.InvertColors");
                        ui.separator();
                        if let Some(doc) = self.doc() {
                            let size = doc.size();
                            ui.label(
                                RichText::new(format!(
                                    "{:.0} × {:.0} in",
                                    size.width / 72.0,
                                    size.height / 72.0
                                ))
                                .color(theme.faint),
                            );
                            ui.separator();
                            ui.label(
                                RichText::new(format!("{:.0}%", doc.view.zoom * 100.0))
                                    .color(theme.faint),
                            );
                        }
                    });
                });
                ui.add_space(1.0);
            });
    }

    fn page_box(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = self.doc() else {
            ui.add_enabled(false, egui::Button::new("—"));
            return;
        };
        let total = doc.pages.len();
        let current = doc.page + 1;
        let mut text = format!("{current}");
        let response = ui.add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(44.0)
                .horizontal_align(egui::Align::Center),
        );
        if response.changed() {
            if let Ok(wanted) = text.trim().parse::<usize>() {
                if wanted >= 1 && wanted <= total {
                    self.go_to(wanted as u32 - 1);
                }
            }
        }
        ui.label(RichText::new(format!("of {total}")).color(self.chrome.theme.faint));
    }

    fn scale_box(&mut self, ui: &mut egui::Ui) {
        let theme = self.chrome.theme;
        let units = self.units;
        let denominator = self.denominator;
        let Some(doc) = self.doc_mut() else {
            ui.label(RichText::new("Scale").color(theme.faint));
            return;
        };
        let page = doc.page;
        let in_force = doc.scale_of(page);
        let current = in_force
            .as_ref()
            .map(|s| s.ratio.clone())
            .unwrap_or_else(|| "Scale Not Set".to_string());
        let text = if in_force.is_none() {
            RichText::new(current).color(theme.warn)
        } else {
            RichText::new(current).color(theme.text)
        };
        let mut chosen: Option<Measure> = None;
        let mut calibrate = false;
        let mut everywhere = false;
        let mut clear = false;
        egui::ComboBox::from_id_salt("scale")
            .selected_text(text)
            .width(180.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(false, "Set scale on the drawing…").clicked() {
                    calibrate = true;
                }
                ui.separator();
                for (name, ratio) in crate::scale::presets(units) {
                    if ui.selectable_label(false, *name).clicked() {
                        chosen = Some(crate::scale::named(name, *ratio, units, denominator));
                    }
                }
                ui.separator();
                if ui
                    .selectable_label(false, "Use this scale on every sheet")
                    .clicked()
                {
                    everywhere = true;
                }
                if ui
                    .add_enabled(
                        in_force.is_some(),
                        egui::Button::selectable(false, "Clear the scale on this sheet"),
                    )
                    .clicked()
                {
                    clear = true;
                }
            });
        if let Some(measure) = chosen {
            doc.set_scale(page, Some(measure));
        }
        if clear {
            // Clearing does not zero the measurements on the sheet; it takes
            // them out of the totals and says so, which is the honest answer.
            doc.set_scale(page, None);
        }
        if everywhere {
            if let Some(measure) = doc.scale_of(page) {
                let count = doc.pages.len() as u32;
                for p in 0..count {
                    doc.set_scale(p, Some(measure.clone()));
                }
            }
        }
        if calibrate {
            self.chrome.tool = "Measure.Calibrate".into();
            self.tool = Tool::Calibrate;
        }
    }

    // ---- the left rail and its panels ------------------------------------

    pub fn side_panels(&mut self, ctx: &egui::Context) {
        let theme = self.chrome.theme;
        let mut tabs: Vec<String> = match self.profile.as_ref() {
            Some(p) if !p.panels.left.tabs.is_empty() => p.panels.left.tabs.clone(),
            _ => ui::chrome::STANDARD_PANELS.iter().map(|s| s.to_string()).collect(),
        };
        // Studio is the office's shared drawings — Hyperview's server panel —
        // so a profile that lists Studio gets it there, where Revu people look
        // for it. One that does not still gets it: it is not optional.
        if !tabs.iter().any(|t| t == "Server" || t == "Studio") {
            tabs.push("Studio".into());
        }

        egui::SidePanel::left("rail")
            .frame(bar_frame(theme.chrome))
            .exact_width(34.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    for name in &tabs {
                        let ready = ui::chrome::panel_ready(name);
                        let open = self.panel == *name;
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
                        let painter = ui.painter();
                        let mut middle = rect.center();
                        if theme.classic {
                            if rail_button(painter, rect, open, response.hovered() && ready.is_ok()) {
                                middle += egui::vec2(1.0, 1.0);
                            }
                        } else if open {
                            ui::icon::tile(painter, rect.shrink(1.0), ui::icon::Tile::Chosen, theme.chrome);
                        } else if response.hovered() && ready.is_ok() {
                            painter.rect_filled(rect, egui::CornerRadius::same(3), theme.hover);
                        }
                        let colour = match (ready, open) {
                            (Err(_), _) => theme.unavailable(),
                            (Ok(()), true) if !theme.classic => ui::icon::ON_TILE,
                            (Ok(()), _) => theme.glyph,
                        };
                        ui::icon::draw(
                            painter,
                            ui::chrome::panel_glyph(name),
                            egui::Rect::from_center_size(middle, egui::vec2(17.0, 17.0)),
                            colour,
                            1.0,
                        );
                        let response = match ready {
                            Ok(()) => response.on_hover_text(name),
                            Err(why) => response.on_hover_text(format!("{name}\n{why}")),
                        };
                        if response.clicked() && ready.is_ok() {
                            self.panel = if open { String::new() } else { name.clone() };
                        }
                    }
                    ui.add_space(6.0);
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
                    let mut middle = rect.center();
                    if theme.classic {
                        if rail_button(ui.painter(), rect, self.show_markups, response.hovered()) {
                            middle += egui::vec2(1.0, 1.0);
                        }
                    } else if self.show_markups {
                        ui::icon::tile(ui.painter(), rect.shrink(1.0), ui::icon::Tile::Chosen, theme.chrome);
                    } else if response.hovered() {
                        ui.painter()
                            .rect_filled(rect, egui::CornerRadius::same(3), theme.hover);
                    }
                    ui::icon::draw(
                        ui.painter(),
                        ui::icon::GRID,
                        egui::Rect::from_center_size(middle, egui::vec2(17.0, 17.0)),
                        if self.show_markups && !theme.classic { ui::icon::ON_TILE } else { theme.glyph },
                        1.0,
                    );
                    if response.on_hover_text("Markups list\nEverything picked up, and what it totals.").clicked() {
                        self.show_markups = !self.show_markups;
                    }
                });
            });

        if self.panel.is_empty() {
            return;
        }
        let name = self.panel.clone();
        egui::SidePanel::left("panel")
            .frame(bar_frame(theme.chrome))
            .default_width(
                self.profile
                    .as_ref()
                    .map(|p| p.panels.left.size.clamp(240.0, 520.0))
                    .unwrap_or(320.0),
            )
            .width_range(220.0..=560.0)
            .show(ctx, |ui| {
                ui.add_space(if theme.classic { 3.0 } else { 6.0 });
                let studio = name == "Server" || name == "Studio";
                ui::chrome::panel_title(ui, theme, &name, |ui| {
                    if studio {
                        let (mark, _) =
                            ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                        ui::mark::draw(ui.painter(), mark, theme);
                        ui.add_space(2.0);
                    }
                });
                if theme.classic {
                    ui.add_space(2.0);
                } else {
                    ui.separator();
                }
                match name.as_str() {
                    "Thumbnails" => self.thumbnails(ui),
                    "Tool Chest" => self.tool_chest(ui),
                    "Measurements" => self.measurements(ui),
                    "Search" => self.search_panel(ui),
                    "Properties" => self.properties(ui),
                    "Server" | "Studio" => self.server_panel(ui),
                    "Spaces" => self.spaces_panel(ui),
                    "Layers" => self.layers_panel(ui),
                    "Bookmarks" => self.bookmarks_panel(ui),
                    "Flags" => self.flags_panel(ui),
                    "Hyperlinks" => self.hyperlinks_panel(ui),
                    "Acroforms" | "Forms" => self.forms_panel(ui),
                    "Signatures" => self.signatures_panel(ui),
                    _ => {
                        ui.add_space(8.0);
                        ui.weak("This panel is not built yet.");
                    }
                }
            });
    }
}

/// A button on the rail in the Classic look: pushed in while its panel is
/// open, standing up under the pointer. True when it is pushed in, so the
/// picture on it can move down and right the way a pressed button's did.
fn rail_button(painter: &egui::Painter, rect: egui::Rect, open: bool, hovered: bool) -> bool {
    use ui::classic::{HIGHLIGHT, PUSHED_IN, SHADOW};
    let ppp = painter.ctx().pixels_per_point();
    if open {
        painter.add(ui::classic::thin(rect, SHADOW, HIGHLIGHT, PUSHED_IN, ppp));
    } else if hovered {
        painter.add(ui::classic::thin(rect, HIGHLIGHT, SHADOW, Color32::TRANSPARENT, ppp));
    }
    open
}

pub fn bar_frame(fill: Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(6, 2))
}

impl App {
    /// The row of open drawing sets.
    ///
    /// Shown only when there is more than one: a single drawing does not need a
    /// tab above it saying which drawing it is, and the window has better uses
    /// for those twenty-six pixels.
    pub fn tabs(&mut self, ctx: &egui::Context) {
        if self.docs.len() < 2 {
            return;
        }
        let theme = self.chrome.theme;
        let mut go: Option<usize> = None;
        let mut close: Option<usize> = None;

        egui::TopBottomPanel::top("tabs")
            .frame(
                egui::Frame::new()
                    .fill(theme.chrome)
                    .inner_margin(egui::Margin {
                        left: 6,
                        right: 6,
                        top: 4,
                        bottom: 0,
                    }),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for (at, doc) in self.docs.iter().enumerate() {
                                let name = crate::app::name_of(&doc.path);
                                let open = at == self.current;
                                let label = ui.fonts(|f| {
                                    f.layout_no_wrap(
                                        name.clone(),
                                        egui::FontId::proportional(12.0),
                                        if open { theme.text } else { theme.faint },
                                    )
                                });
                                // Room for the name, the unsaved dot and the
                                // close cross, without the tab jumping about as
                                // the dot comes and goes.
                                let width = (label.size().x + 46.0).min(240.0);
                                let (rect, response) = ui.allocate_exact_size(
                                    egui::vec2(width, 26.0),
                                    egui::Sense::click(),
                                );
                                let painter = ui.painter();
                                let corner = egui::CornerRadius {
                                    nw: 4,
                                    ne: 4,
                                    sw: 0,
                                    se: 0,
                                };
                                if theme.classic {
                                    classic_tab(painter, rect, open);
                                } else if open {
                                    painter.rect_filled(rect, corner, theme.bar);
                                    painter.rect_filled(
                                        egui::Rect::from_min_max(
                                            egui::pos2(rect.left(), rect.top()),
                                            egui::pos2(rect.right(), rect.top() + 2.0),
                                        ),
                                        corner,
                                        theme.accent_text,
                                    );
                                } else if response.hovered() {
                                    painter.rect_filled(rect, corner, theme.hover);
                                }

                                let mut left = rect.left() + 10.0;
                                // A dot rather than an asterisk: it reads at a
                                // glance and is in every font, which an
                                // asterisk in the wrong place is not.
                                if doc.dirty && !doc.read_only {
                                    painter.circle_filled(
                                        egui::pos2(left + 3.0, rect.center().y),
                                        3.0,
                                        theme.warn,
                                    );
                                    left += 12.0;
                                } else if doc.read_only {
                                    painter.circle_stroke(
                                        egui::pos2(left + 3.0, rect.center().y),
                                        3.0,
                                        egui::Stroke::new(1.0_f32, theme.faint), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
                                    );
                                    left += 12.0;
                                }
                                painter.galley(
                                    egui::pos2(left, rect.center().y - label.size().y * 0.5),
                                    label,
                                    theme.text,
                                );

                                // The close cross, painted rather than typed.
                                let cross = egui::Rect::from_center_size(
                                    egui::pos2(rect.right() - 13.0, rect.center().y),
                                    egui::vec2(14.0, 14.0),
                                );
                                let over_cross = ui
                                    .input(|i| i.pointer.hover_pos())
                                    .map(|p| cross.contains(p))
                                    .unwrap_or(false);
                                if over_cross {
                                    painter.circle_filled(cross.center(), 8.0, theme.hover);
                                }
                                draw_cross(
                                    painter,
                                    cross,
                                    if over_cross { theme.text } else { theme.faint },
                                );

                                if response.clicked() {
                                    if over_cross {
                                        close = Some(at);
                                    } else {
                                        go = Some(at);
                                    }
                                }
                                let _ = response.on_hover_text(doc.path.display().to_string());
                            }
                        });
                    });
            });

        if let Some(at) = close {
            self.close_tab(at);
            self.retitle(ctx);
        } else if let Some(at) = go {
            self.switch_to(at);
            self.retitle(ctx);
        }
    }

    /// Brings a tab forward.
    pub fn switch_to(&mut self, at: usize) {
        if at >= self.docs.len() || at == self.current {
            return;
        }
        self.current = at;
        // A search belongs to the drawing it was run on. Carrying its answers
        // across to another set would point at sheets that are not there.
        self.search.clear();
        let Some(doc) = self.docs.get(at) else { return };
        let (id, page) = (doc.id, doc.page);
        let wanted = !doc.previews.contains_key(&page);
        let unlabelled = (doc.labelled as usize) < doc.pages.len();
        if wanted {
            self.svc.send(crate::render::ToWorker::Preview { doc: id, page });
        }
        if unlabelled {
            self.svc.send(crate::render::ToWorker::ScanLabels(id));
        }
    }
}

/// One tab in the Classic look: lit along the top and left, shaded down the
/// right, and the one in front a little taller and joined to what is below.
fn classic_tab(painter: &egui::Painter, rect: egui::Rect, open: bool) {
    use ui::classic::{DARK, FACE, HIGHLIGHT, SHADOW};
    let ppp = painter.ctx().pixels_per_point();
    let w = ui::classic::line_width(ppp);
    let r = if open { rect } else { egui::Rect::from_min_max(rect.min + egui::vec2(0.0, 2.0), rect.max) };
    let mut mesh = egui::Mesh::default();
    mesh.add_colored_rect(r, FACE);
    mesh.add_colored_rect(egui::Rect::from_min_max(r.min + egui::vec2(w, 0.0), egui::pos2(r.max.x - w, r.min.y + w)), HIGHLIGHT);
    mesh.add_colored_rect(egui::Rect::from_min_max(r.min + egui::vec2(0.0, w), egui::pos2(r.min.x + w, r.max.y)), HIGHLIGHT);
    mesh.add_colored_rect(egui::Rect::from_min_max(egui::pos2(r.max.x - w, r.min.y + w), r.max), DARK);
    mesh.add_colored_rect(egui::Rect::from_min_max(egui::pos2(r.max.x - 2.0 * w, r.min.y + w), egui::pos2(r.max.x - w, r.max.y)), SHADOW);
    if !open {
        // Behind the one in front: the line along the bottom shows.
        mesh.add_colored_rect(egui::Rect::from_min_max(egui::pos2(r.min.x, r.max.y - w), r.max), HIGHLIGHT);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// A close cross, drawn. Two strokes, in any font and at any size.
fn draw_cross(painter: &egui::Painter, rect: egui::Rect, colour: Color32) {
    let c = rect.center();
    let s = rect.width() * 0.26;
    let stroke = egui::Stroke::new(1.3_f32, colour); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
    painter.line_segment(
        [egui::pos2(c.x - s, c.y - s), egui::pos2(c.x + s, c.y + s)],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(c.x + s, c.y - s), egui::pos2(c.x - s, c.y + s)],
        stroke,
    );
}
