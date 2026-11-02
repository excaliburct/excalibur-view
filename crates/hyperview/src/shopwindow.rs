//! The Shop List: the takeoff as something the shop floor can work from.
//!
//! Revu stops at the markups list. It will tell you that you picked up
//! thirty-one lengths of W12x26 and what they add up to, and then you build
//! the cut list in a spreadsheet by hand — which is the step where a job
//! usually acquires its first wrong number.
//!
//! This is that step, done from the same rows the grid is showing, so the cut
//! list and the screen cannot disagree. It rounds every length **up** to a
//! cutting increment, gathers the identical ones into one line with a count,
//! weighs each shape from the pounds-per-foot that came off his own tool
//! chest, and — once somebody has said what stock length they buy — nests the
//! pieces into sticks and says how many to order and what falls on the floor.
//!
//! Every warning the takeoff carries comes with it: a sheet with no scale, a
//! shape the chest has no weight for, a piece longer than the stock. None of
//! them is quietly dropped and none of them is quietly rounded to zero.

use egui::RichText;
use takeoff::shoplist::{self, Buying, Nest, ShopList};

use crate::app::App;

/// The window's own state. It holds what has been typed, which is not the
/// same as what has been accepted: a half-typed stock length is not a stock
/// length, and nothing is nested off one.
pub struct Shop {
    pub open: bool,
    pub stock: String,
    pub kerf: String,
    pub step: String,
    pub keep: String,
    /// Show the nesting rather than the cut list.
    pub nesting: bool,
    /// Which shape's sticks are being looked at, by name.
    pub looking_at: Option<String>,
}

impl Default for Shop {
    fn default() -> Shop {
        Shop {
            open: false,
            stock: String::new(),
            kerf: String::new(),
            step: String::new(),
            keep: String::new(),
            nesting: false,
            looking_at: None,
        }
    }
}

/// How lengths get typed in and written back out for one drawing.
///
/// A takeoff's lengths are kept in whatever the sheet's scale calls its
/// primary unit — feet on an imperial job, metres or millimetres on a metric
/// one. Somebody typing `40'` into the stock box means forty feet of real
/// steel, so the box has to know which unit it is landing in. When the scale
/// names a unit this does not recognise, nothing is converted and the box is
/// read as a plain number in that unit — which is the honest answer, and
/// better than a conversion nobody asked for.
#[derive(Clone, Copy, Debug)]
pub struct Rule {
    /// How many inches one primary unit is. Zero means "no idea".
    pub inches_per_unit: f64,
    pub units: crate::units::Units,
    pub denominator: u32,
}

impl Rule {
    /// Works the rule out from the sheet's own scale.
    pub fn of(
        scale: Option<&annot::measure::Measure>,
        units: crate::units::Units,
        denominator: u32,
    ) -> Rule {
        let named = scale
            .and_then(|s| s.x.first())
            .map(|f| f.unit.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let mm = crate::units::MM_PER_INCH;
        let inches_per_unit = match named.as_str() {
            "'" | "ft" | "feet" | "foot" => 12.0,
            "\"" | "in" | "inch" | "inches" => 1.0,
            "m" | "meter" | "metre" | "meters" | "metres" => 1000.0 / mm,
            "cm" => 10.0 / mm,
            "mm" => 1.0 / mm,
            _ => 0.0,
        };
        Rule {
            inches_per_unit,
            units,
            denominator,
        }
    }

    /// Reads what somebody typed, in the drawing's primary unit.
    ///
    /// An empty or unreadable box is `None` — "nobody has said" — and never
    /// zero, because the two mean opposite things everywhere in this program.
    pub fn read(&self, typed: &str) -> Option<f64> {
        let typed = typed.trim();
        if typed.is_empty() {
            return None;
        }
        let value = if self.inches_per_unit > 0.0 {
            self.units.parse(typed)? / self.inches_per_unit
        } else {
            typed.parse::<f64>().ok()?
        };
        (value.is_finite() && value > 0.0).then_some(value)
    }

    /// Writes a length in the primary unit back into a box.
    pub fn write(&self, value: f64) -> String {
        if self.inches_per_unit > 0.0 {
            crate::units::format(value * self.inches_per_unit, self.units, self.denominator)
        } else {
            format!("{value:.4}")
        }
    }
}

impl App {
    /// How lengths are typed and written for the drawing in front.
    pub fn shop_rule(&self) -> Rule {
        let scale = self
            .doc()
            .and_then(|doc| doc.rows().into_iter().find_map(|r| r.scale));
        Rule::of(scale.as_ref(), self.prefs.units, self.prefs.denominator)
    }

    /// The command behind **Document ▸ Shop List**.
    pub fn shop_list(&mut self) {
        let Some(doc) = self.doc() else {
            self.status = "Open a drawing first.".into();
            return;
        };
        if doc.rows().is_empty() {
            self.status = "There is nothing on this drawing to make a cut list from.".into();
            return;
        }
        let rule = self.shop_rule();
        let buying = self.prefs.buying();
        self.shop.open = true;
        if self.shop.stock.is_empty() && buying.stock > 0.0 {
            self.shop.stock = rule.write(buying.stock);
        }
        if self.shop.kerf.is_empty() {
            self.shop.kerf = rule.write(buying.kerf);
        }
        if self.shop.step.is_empty() {
            self.shop.step = rule.write(buying.step);
        }
        if self.shop.keep.is_empty() {
            self.shop.keep = rule.write(buying.worth_keeping);
        }
    }

    /// What is being bought and cut, as the boxes currently read.
    fn buying_now(&self) -> Buying {
        let rule = self.shop_rule();
        let was = self.prefs.buying();
        Buying {
            step: rule.read(&self.shop.step).unwrap_or(was.step),
            // No fallback for the stock. An unreadable stock length means
            // nobody has said one, and nothing is nested off a guess.
            stock: rule.read(&self.shop.stock).unwrap_or(0.0),
            kerf: rule.read(&self.shop.kerf).unwrap_or(0.0),
            worth_keeping: rule.read(&self.shop.keep).unwrap_or(0.0),
        }
    }

    /// The cut list as the drawing stands right now.
    pub fn shop_list_now(&self) -> Option<(ShopList, Buying)> {
        let doc = self.doc()?;
        let buying = self.buying_now();
        let rows = doc.rows();
        Some((
            shoplist::build(&rows, crate::panelbody::weights(), buying.step),
            buying,
        ))
    }

    pub fn shop_window(&mut self, ctx: &egui::Context) {
        if !self.shop.open {
            return;
        }
        let theme = self.chrome.theme;
        let Some((list, buying)) = self.shop_list_now() else {
            self.shop.open = false;
            return;
        };
        let scale = self
            .doc()
            .and_then(|doc| doc.rows().into_iter().find_map(|r| r.scale));
        let unit = self
            .doc()
            .and_then(|doc| doc.rows().into_iter().find(|r| r.scaled).map(|r| r.unit))
            .unwrap_or_else(|| "'".into());
        let nests = if buying.can_nest() {
            shoplist::nest_all(&list, buying.stock, buying.kerf)
        } else {
            Vec::new()
        };

        let say = |value: f64| match &scale {
            Some(scale) => {
                let per = scale.per_point();
                if per > 0.0 {
                    scale.length(value / per)
                } else {
                    format!("{value:.3}")
                }
            }
            None => format!("{value:.3}"),
        };

        let mut open = true;
        let mut save_cuts = false;
        let mut save_nest = false;
        let mut save_pdf = false;
        let mut remember = false;

        egui::Window::new("Shop List")
            .collapsible(false)
            .resizable(true)
            .default_width(760.0)
            .default_height(600.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "Built from the same markups the list is showing. Every cutting \
                         length is rounded up, never down — a member that arrives short \
                         is a crane that waits.",
                    )
                    .color(theme.faint)
                    .size(11.0),
                );
                ui.add_space(8.0);

                // ---- what the shop buys and cuts with ----------------------
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Cut to the nearest").size(11.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.shop.step)
                            .desired_width(70.0)
                            .hint_text("1\""),
                    );
                    ui.add_space(12.0);
                    ui.label(RichText::new("Stock length").size(11.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.shop.stock)
                            .desired_width(80.0)
                            .hint_text("none set"),
                    );
                    ui.add_space(12.0);
                    ui.label(RichText::new("Saw kerf").size(11.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.shop.kerf)
                            .desired_width(70.0)
                            .hint_text("1/8\""),
                    );
                    ui.add_space(12.0);
                    ui.label(RichText::new("Keep drops over").size(11.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.shop.keep)
                            .desired_width(70.0)
                            .hint_text("2'"),
                    );
                });
                ui.add_space(4.0);
                if !buying.can_nest() {
                    ui.label(
                        RichText::new(
                            "No stock length set, so nothing is nested. This program has \
                             no idea what length you buy and will not pick one for you — \
                             twenty, forty and sixty feet are all ordinary, and a cut \
                             list that guessed would be one that ordered the wrong steel.",
                        )
                        .color(theme.faint)
                        .size(10.0),
                    );
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.shop.nesting, false, "Cut list");
                    ui.add_enabled_ui(buying.can_nest(), |ui| {
                        ui.selectable_value(&mut self.shop.nesting, true, "What to buy");
                    });
                });
                ui.separator();

                if self.shop.nesting && buying.can_nest() {
                    nest_table(ui, theme, &nests, buying, &say, &mut self.shop.looking_at);
                } else {
                    cut_table(ui, theme, &list, &say, &unit);
                }

                // ---- the warnings, never in a footnote ---------------------
                let missing = list.what_is_missing();
                if !missing.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new(missing).color(theme.warn).size(11.0));
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Save cut list as CSV").clicked() {
                        save_cuts = true;
                    }
                    if ui
                        .add_enabled(buying.can_nest(), egui::Button::new("Save nesting as CSV"))
                        .clicked()
                    {
                        save_nest = true;
                    }
                    if ui.button("Save as PDF").clicked() {
                        save_pdf = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Remember these settings").clicked() {
                            remember = true;
                        }
                    });
                });
            });

        if !open {
            self.shop.open = false;
        }
        if remember {
            self.prefs.cut_step = buying.step;
            self.prefs.stock_length = buying.stock;
            self.prefs.kerf = buying.kerf;
            self.prefs.worth_keeping = buying.worth_keeping;
            match self.prefs.save() {
                Ok(()) => self.status = "These are the settings from now on.".into(),
                Err(why) => self.error = Some(why),
            }
        }
        if save_cuts {
            let sheet = |page: usize| {
                self.doc()
                    .map(|doc| doc.sheet_name(page as u32))
                    .unwrap_or_else(|| format!("{}", page + 1))
            };
            let csv = shoplist::to_csv(&list, &unit, &sheet);
            self.save_beside("cut list", "CSV", "csv", csv.into_bytes());
        }
        if save_nest {
            let csv = shoplist::nest_csv(&nests, &unit);
            self.save_beside("nesting", "CSV", "csv", csv.into_bytes());
        }
        if save_pdf {
            self.write_shop_pdf(&list, &nests, buying, &unit);
        }
    }
}

// ---- the two tables ---------------------------------------------------------

fn cut_table(
    ui: &mut egui::Ui,
    theme: ui::chrome::Theme,
    list: &ShopList,
    say: &dyn Fn(f64) -> String,
    unit: &str,
) {
    if list.shapes.is_empty() {
        ui.add_space(12.0);
        ui.label(
            RichText::new(
                "No length measurements on this drawing yet. A cut list is made of \
                 lengths; areas and counts are on the Summary instead.",
            )
            .color(theme.faint)
            .size(11.0),
        );
        return;
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height(320.0)
        .show(ui, |ui| {
            egui::Grid::new("cut list")
                .num_columns(5)
                .striped(true)
                .spacing(egui::vec2(18.0, 4.0))
                .show(ui, |ui| {
                    for head in ["Shape", "Cut", "Pieces", "Total", "Weight"] {
                        ui.label(RichText::new(head).color(theme.faint).size(10.0));
                    }
                    ui.end_row();
                    for shape in &list.shapes {
                        ui.label(RichText::new(&shape.name).strong().size(11.0));
                        ui.label("");
                        ui.label(RichText::new(format!("{}", shape.pieces)).strong().size(11.0));
                        ui.label(
                            RichText::new(say(shape.total_length))
                                .strong()
                                .size(11.0),
                        );
                        // `None` is not zero, and the cell says which it is.
                        ui.label(match shape.pounds {
                            Some(pounds) => RichText::new(format!("{pounds:.0} lb"))
                                .strong()
                                .size(11.0),
                            None => RichText::new("no unit weight")
                                .color(theme.warn)
                                .size(10.0),
                        });
                        ui.end_row();
                        for cut in &shape.cuts {
                            ui.label("");
                            ui.label(RichText::new(say(cut.length)).size(11.0));
                            ui.label(RichText::new(format!("× {}", cut.count)).size(11.0));
                            ui.label(
                                RichText::new(say(cut.length * cut.count as f64))
                                    .color(theme.faint)
                                    .size(11.0),
                            );
                            ui.label(
                                RichText::new(match shape.per_foot {
                                    Some(per) => format!(
                                        "{:.0} lb",
                                        per * cut.length * cut.count as f64
                                    ),
                                    None => String::new(),
                                })
                                .color(theme.faint)
                                .size(11.0),
                            );
                            ui.end_row();
                            // Where they are, when the sheet has a grid to
                            // say. A blank line is not drawn at all.
                            if !cut.places.is_empty() {
                                ui.label("");
                                ui.label("");
                                ui.label("");
                                ui.label(
                                    RichText::new(cut.places.join(", "))
                                        .color(theme.accent_text)
                                        .size(10.0),
                                );
                                ui.label("");
                                ui.end_row();
                            }
                        }
                    }
                });
        });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "{} piece{} · {}",
                list.pieces,
                if list.pieces == 1 { "" } else { "s" },
                list.weight_says()
            ))
            .strong()
            .size(12.0),
        );
        ui.label(
            RichText::new(format!("(lengths in {unit})"))
                .color(theme.faint)
                .size(10.0),
        );
    });
}

fn nest_table(
    ui: &mut egui::Ui,
    theme: ui::chrome::Theme,
    nests: &[Nest],
    buying: Buying,
    say: &dyn Fn(f64) -> String,
    looking_at: &mut Option<String>,
) {
    if nests.is_empty() {
        ui.label(RichText::new("Nothing to nest.").color(theme.faint).size(11.0));
        return;
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height(320.0)
        .show(ui, |ui| {
            for nest in nests {
                let chosen = looking_at.as_deref() == Some(nest.shape.as_str());
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(
                            chosen,
                            RichText::new(format!(
                                "{} — buy {} × {}",
                                nest.shape,
                                nest.sticks_to_buy(),
                                say(nest.stock)
                            ))
                            .strong()
                            .size(12.0),
                        )
                        .clicked()
                    {
                        *looking_at = if chosen { None } else { Some(nest.shape.clone()) };
                    }
                    ui.label(
                        RichText::new(format!(
                            "{:.0}% of it ends up in the building",
                            nest.yield_of() * 100.0
                        ))
                        .color(theme.faint)
                        .size(10.0),
                    );
                });
                if nest.unnested > 0 {
                    ui.label(
                        RichText::new(format!(
                            "{} pieces were not nested: a shape with more than {} pieces is not nested one by one. None of them is on a stick above.",
                            nest.unnested,
                            takeoff::shoplist::MOST_NESTED
                        ))
                        .color(theme.warn),
                    ); // WHAT: said plainly. WHY: a shape with no sticks must not read as nothing to buy.
                } else if !nest.is_whole() {
                    ui.label(
                        RichText::new(format!(
                            "{} piece{} will not come out of {} stock: {}. \
                             They are not on any stick above.",
                            nest.too_long.len(),
                            if nest.too_long.len() == 1 { "" } else { "s" },
                            say(nest.stock),
                            nest.too_long
                                .iter()
                                .map(|p| say(*p))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                        .color(theme.warn)
                        .size(10.0),
                    );
                }
                let keepers = nest.keepers(buying.worth_keeping);
                if !keepers.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "{} drop{} worth racking: {}",
                            keepers.len(),
                            if keepers.len() == 1 { "" } else { "s" },
                            keepers.iter().map(|d| say(*d)).collect::<Vec<_>>().join(", ")
                        ))
                        .color(theme.faint)
                        .size(10.0),
                    );
                }
                if chosen {
                    ui.add_space(2.0);
                    for (at, stick) in nest.sticks.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add_space(14.0);
                            ui.label(
                                RichText::new(format!("#{}", at + 1))
                                    .color(theme.faint)
                                    .size(10.0),
                            );
                            ui.label(
                                RichText::new(
                                    stick
                                        .pieces
                                        .iter()
                                        .map(|p| say(*p))
                                        .collect::<Vec<_>>()
                                        .join("  +  "),
                                )
                                .size(11.0),
                            );
                            ui.label(
                                RichText::new(format!("drop {}", say(stick.drop)))
                                    .color(if stick.drop >= buying.worth_keeping {
                                        theme.faint
                                    } else {
                                        theme.warn
                                    })
                                    .size(10.0),
                            );
                        });
                    }
                }
                ui.add_space(6.0);
            }
        });

    let sticks: usize = nests.iter().map(|n| n.sticks_to_buy()).sum();
    let bought: f64 = nests.iter().map(|n| n.bought).sum();
    let used: f64 = nests.iter().map(|n| n.used).sum();
    ui.add_space(6.0);
    ui.label(
        RichText::new(format!(
            "{sticks} stick{} · {} bought · {} cut · {} on the floor",
            if sticks == 1 { "" } else { "s" },
            say(bought),
            say(used),
            say(bought - used)
        ))
        .strong()
        .size(12.0),
    );
}

// ---- saving it -------------------------------------------------------------

impl App {
    /// Saves a file the drawing produced, suggested beside the drawing.
    pub fn save_beside(&mut self, what: &str, filter: &str, extension: &str, bytes: Vec<u8>) {
        let suggested = match self.doc() {
            Some(doc) => format!(
                "{} {what}.{extension}",
                doc.path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default()
            ),
            None => format!("{what}.{extension}"),
        };
        self.filing.one(
            crate::files::Choose::save(suggested).filter(filter, &[extension]),
            move |app, path| match std::fs::write(&path, bytes) {
                Ok(()) => app.status = format!("Written to {}", path.display()),
                Err(e) => app.error = Some(format!("Could not write that file: {e}")),
            },
        );
    }

    /// The cut list as a PDF, laid out with the same machinery as the takeoff
    /// summary so the two read like the same document.
    fn write_shop_pdf(&mut self, list: &ShopList, nests: &[Nest], buying: Buying, unit: &str) {
        let Some(doc) = self.doc() else { return };
        let drawing = doc
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let sheet = |page: usize| doc.sheet_name(page as u32);
        let rule = self.shop_rule();
        let report = report_of(list, nests, buying, unit, &drawing, &self.author, &sheet, rule);

        let suggested = crate::docops::beside(&doc.path, "shop list");
        let mut choose = crate::files::Choose::save(
            suggested.file_name().unwrap_or_default().to_string_lossy(),
        )
        .filter("PDF", &["pdf"]);
        if let Some(folder) = suggested.parent() {
            choose = choose.start_in(folder);
        }
        self.filing.one(choose, move |app, to| {
            let to = crate::docops::free_name(&to);
            match crate::render::write_report(None, &report, &to) {
                Ok(done) => {
                    app.status = done.said;
                    app.open(to);
                }
                Err(why) => app.error = Some(why),
            }
        });
    }
}

/// Lays the cut list out as a report.
///
/// The warning block at the top is the same one the takeoff summary carries,
/// for the same reason: a cut list that is quietly short is worse than no cut
/// list, because somebody orders steel off it.
#[allow(clippy::too_many_arguments)]
pub fn report_of(
    list: &ShopList,
    nests: &[Nest],
    buying: Buying,
    unit: &str,
    drawing: &str,
    who: &str,
    sheet: &dyn Fn(usize) -> String,
    rule: Rule,
) -> crate::report::Report {
    use crate::report::{paginate, widths_for, Line};

    let headings: Vec<String> = ["Shape", "Cut", "Pieces", "Total", "Weight"]
        .iter()
        .map(|h| h.to_string())
        .collect();
    let mut lines: Vec<Line> = Vec::new();
    let say = |value: f64| rule.write(value);

    for shape in &list.shapes {
        lines.push(Line {
            cells: vec![
                shape.name.clone(),
                String::new(),
                format!("{}", shape.pieces),
                say(shape.total_length),
                match shape.pounds {
                    Some(pounds) => format!("{pounds:.0} lb"),
                    // Never a zero. The cell says the chest had no weight.
                    None => "no unit weight".to_string(),
                },
            ],
            heading: true,
        });
        for cut in &shape.cuts {
            lines.push(Line {
                cells: vec![
                    String::new(),
                    say(cut.length),
                    format!("× {}", cut.count),
                    say(cut.length * cut.count as f64),
                    match shape.per_foot {
                        Some(per) => format!("{:.0} lb", per * cut.length * cut.count as f64),
                        None => String::new(),
                    },
                ],
                heading: false,
            });
            // Where they go, when the sheet's grid says. This is the line
            // that makes the difference between a purchase order and an
            // instruction somebody can carry out.
            if !cut.places.is_empty() {
                lines.push(Line {
                    cells: vec![
                        String::new(),
                        String::new(),
                        String::new(),
                        format!("at {}", cut.places.join(", ")),
                        String::new(),
                    ],
                    heading: false,
                });
            }
        }
        // Which sheets the shape came off, so somebody arguing about a
        // quantity six months from now can go and look at the drawing.
        let mut pages: Vec<usize> = shape
            .cuts
            .iter()
            .flat_map(|c| c.pages.iter().copied())
            .collect();
        pages.sort_unstable();
        pages.dedup();
        lines.push(Line {
            cells: vec![
                String::new(),
                format!(
                    "off {}",
                    pages.iter().map(|p| sheet(*p)).collect::<Vec<_>>().join(", ")
                ),
            ],
            heading: false,
        });
    }

    lines.push(Line {
        cells: vec![String::new()],
        heading: false,
    });
    lines.push(Line {
        cells: vec![
            "ALL".to_string(),
            String::new(),
            format!("{} pieces", list.pieces),
            String::new(),
            // Not always a figure: see `ShopList::weight_says`.
            list.weight_says(),
        ],
        heading: true,
    });

    if !nests.is_empty() {
        lines.push(Line {
            cells: vec![String::new()],
            heading: false,
        });
        lines.push(Line {
            cells: vec![format!("WHAT TO BUY — {} stock", say(buying.stock))],
            heading: true,
        });
        for nest in nests {
            lines.push(Line {
                cells: vec![
                    nest.shape.clone(),
                    format!("{} × {}", nest.sticks_to_buy(), say(nest.stock)),
                    String::new(),
                    say(nest.bought),
                    format!("{:.0}% used", nest.yield_of() * 100.0),
                ],
                heading: true,
            });
            for (at, stick) in nest.sticks.iter().enumerate() {
                lines.push(Line {
                    cells: vec![
                        String::new(),
                        format!("#{}", at + 1),
                        format!("{} pc", stick.pieces.len()),
                        stick
                            .pieces
                            .iter()
                            .map(|p| say(*p))
                            .collect::<Vec<_>>()
                            .join(" + "),
                        format!("drop {}", say(stick.drop)),
                    ],
                    heading: false,
                });
            }
            for long in &nest.too_long {
                lines.push(Line {
                    cells: vec![
                        String::new(),
                        say(*long),
                        String::new(),
                        "WILL NOT COME OUT OF THIS STOCK — not on any stick above"
                            .to_string(),
                        String::new(),
                    ],
                    heading: false,
                });
            }
        }
    }

    // The warning, carried at the top where somebody reads it rather than in
    // a footnote where they do not.
    let mut short_by = list.what_is_missing();
    if list.unscaled > 0 {
        let sheets: Vec<String> = list.unscaled_pages.iter().map(|p| sheet(*p)).collect();
        short_by.push_str(&format!(
            " Sheets with no scale: {}. Set a scale on them and take this list again.",
            sheets.join(", ")
        ));
    }
    let stuck: usize = nests.iter().map(|n| n.too_long.len()).sum();
    if stuck > 0 {
        short_by.push_str(&format!(
            " {stuck} piece(s) will not come out of {} stock and are on no stick.",
            say(buying.stock)
        ));
    }

    let widths = widths_for(&headings, &lines);
    crate::report::Report {
        title: format!("Shop list · lengths in {unit}"),
        drawing: drawing.to_string(),
        when: crate::stamps::today(),
        who: who.to_string(),
        short_by,
        headings,
        widths,
        pages: paginate(lines),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::Units;
    use takeoff::shoplist::INCH;
    use takeoff::{Row, WeightColumns};

    fn feet() -> Rule {
        Rule {
            inches_per_unit: 12.0,
            units: Units::FeetInches,
            denominator: 16,
        }
    }

    fn a_row(subject: &str, feet: f64, page: usize, per_foot: Option<f64>) -> Row {
        let mut row = Row::blank();
        row.page = page;
        row.subject = subject.into();
        row.kind = annot::Kind::Length;
        row.length = Some(feet);
        row.scaled = true;
        row.unit = "'".into();
        if let Some(per) = per_foot {
            row.columns[crate::panelbody::weights().per_length] = format!("{per}");
        }
        row
    }

    #[test]
    fn a_stock_length_typed_in_feet_lands_in_feet() {
        let rule = feet();
        assert!((rule.read("40'").unwrap() - 40.0).abs() < 1e-9);
        assert!((rule.read("40'-6\"").unwrap() - 40.5).abs() < 1e-9);
        assert!((rule.read("1/8\"").unwrap() - INCH / 8.0).abs() < 1e-9);
    }

    #[test]
    fn an_empty_box_is_nobody_has_said_and_never_zero() {
        // The two mean opposite things everywhere in this program: zero stock
        // is a nonsense, no stock is a question nobody has answered.
        let rule = feet();
        assert_eq!(rule.read(""), None);
        assert_eq!(rule.read("   "), None);
        assert_eq!(rule.read("nonsense"), None);
        assert_eq!(rule.read("0"), None, "zero is not a stock length");
        assert_eq!(rule.read("-20'"), None);
    }

    #[test]
    fn what_is_written_into_a_box_reads_back_as_the_same_length() {
        let rule = feet();
        for value in [40.0, 24.5, 2.0, INCH, INCH / 8.0] {
            let back = rule.read(&rule.write(value)).unwrap_or(-1.0);
            assert!((back - value).abs() < 1e-6, "{value} came back as {back}");
        }
    }

    #[test]
    fn a_scale_in_metres_puts_the_boxes_in_metres() {
        let scale = annot::measure::metric(50.0, "1:50");
        let rule = Rule::of(Some(&scale), Units::Meters, 16);
        assert!(rule.inches_per_unit > 0.0, "metres are a unit we know");
        let back = rule.read(&rule.write(12.0)).unwrap_or(-1.0);
        assert!((back - 12.0).abs() < 1e-6, "{back}");
    }

    #[test]
    fn a_unit_nobody_recognises_is_left_alone_rather_than_converted() {
        // Better a plain number in whatever the sheet calls its unit than a
        // conversion nobody asked for.
        let rule = Rule {
            inches_per_unit: 0.0,
            units: Units::FeetInches,
            denominator: 16,
        };
        assert!((rule.read("40").unwrap() - 40.0).abs() < 1e-9);
        assert_eq!(rule.write(40.0), "40.0000");
    }

    #[test]
    fn the_report_carries_the_warning_at_the_top() {
        let mut short = a_row("W12x26", 20.0, 4, Some(26.0));
        short.scaled = false;
        let rows = vec![
            a_row("W12x26", 20.0, 0, Some(26.0)),
            a_row("SOMETHING ODD", 8.0, 0, None),
            short,
        ];
        let list = shoplist::build(&rows, WeightColumns::default(), INCH);
        let report = report_of(
            &list,
            &[],
            Buying::default(),
            "'",
            "job.pdf",
            "Sam",
            &|p| format!("S-{}", p + 1),
            feet(),
        );
        assert!(report.short_by.contains("NOT IN THE WEIGHT"));
        assert!(report.short_by.contains("SOMETHING ODD"));
        assert!(report.short_by.contains("S-5"));
        assert!(!report.pages.is_empty());
    }

    #[test]
    fn a_shape_with_no_weight_says_so_on_the_page_rather_than_printing_a_zero() {
        let list = shoplist::build(
            &[a_row("SOMETHING ODD", 8.0, 0, None)],
            WeightColumns::default(),
            INCH,
        );
        let report = report_of(
            &list,
            &[],
            Buying::default(),
            "'",
            "job.pdf",
            "Sam",
            &|p| format!("{}", p + 1),
            feet(),
        );
        let words: Vec<String> = report
            .pages
            .iter()
            .flat_map(|page| page.lines.iter())
            .map(|line| line.cells.join(" "))
            .collect();
        assert!(
            words.iter().any(|w| w.contains("no unit weight")),
            "{words:?}"
        );
        assert!(
            !words.iter().any(|w| w.contains("0 lb")),
            "a zero weight was printed: {words:?}"
        );
    }

    #[test]
    fn a_piece_that_will_not_come_out_of_the_stock_is_on_the_page_and_in_the_warning() {
        let rows = vec![
            a_row("W12x26", 70.0, 0, Some(26.0)),
            a_row("W12x26", 20.0, 0, Some(26.0)),
        ];
        let list = shoplist::build(&rows, WeightColumns::default(), INCH);
        let buying = Buying {
            stock: 60.0,
            ..Default::default()
        };
        let nests = shoplist::nest_all(&list, buying.stock, buying.kerf);
        let report = report_of(
            &list,
            &nests,
            buying,
            "'",
            "job.pdf",
            "Sam",
            &|p| format!("{}", p + 1),
            feet(),
        );
        assert!(report.short_by.contains("will not come out"));
        let words: Vec<String> = report
            .pages
            .iter()
            .flat_map(|page| page.lines.iter())
            .map(|line| line.cells.join(" "))
            .collect();
        assert!(
            words.iter().any(|w| w.contains("WILL NOT COME OUT")),
            "{words:?}"
        );
    }
}

#[cfg(test)]
mod grid_tests {
    use super::*;
    use crate::units::Units;
    use takeoff::shoplist::INCH;
    use takeoff::{Row, WeightColumns};

    fn feet() -> Rule {
        Rule {
            inches_per_unit: 12.0,
            units: Units::FeetInches,
            denominator: 16,
        }
    }

    fn at(subject: &str, feet: f64, place: &str) -> Row {
        let mut row = Row::blank();
        row.subject = subject.into();
        row.kind = annot::Kind::Length;
        row.length = Some(feet);
        row.scaled = true;
        row.unit = "'".into();
        row.grid = place.into();
        row.columns[crate::panelbody::weights().per_length] = "26".into();
        row
    }

    #[test]
    fn the_cut_list_pdf_says_where_the_pieces_go() {
        let rows = vec![
            at("W12x26", 24.5, "B-4"),
            at("W12x26", 24.5, "C-4"),
            at("W12x26", 24.5, "D-4"),
        ];
        let list = shoplist::build(&rows, WeightColumns::default(), INCH);
        let report = report_of(
            &list,
            &[],
            Buying::default(),
            "'",
            "job.pdf",
            "Sam",
            &|p| format!("{}", p + 1),
            feet(),
        );
        let words: Vec<String> = report
            .pages
            .iter()
            .flat_map(|page| page.lines.iter())
            .map(|line| line.cells.join(" "))
            .collect();
        assert!(
            words.iter().any(|w| w.contains("at B-4, C-4, D-4")),
            "{words:?}"
        );
    }

    #[test]
    fn a_sheet_with_no_grid_adds_no_line_about_where() {
        // No grid is a real answer, and a blank "at" line would read as one
        // that could not be worked out.
        let mut plain = at("W12x26", 24.5, "");
        plain.grid = String::new();
        let list = shoplist::build(&[plain], WeightColumns::default(), INCH);
        let report = report_of(
            &list,
            &[],
            Buying::default(),
            "'",
            "job.pdf",
            "Sam",
            &|p| format!("{}", p + 1),
            feet(),
        );
        let words: Vec<String> = report
            .pages
            .iter()
            .flat_map(|page| page.lines.iter())
            .map(|line| line.cells.join(" ").trim().to_string())
            .collect();
        assert!(!words.iter().any(|w| w.starts_with("at ")), "{words:?}");
    }
}
