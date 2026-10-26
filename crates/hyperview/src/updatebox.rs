//! The box that says a new version is here, and what is new in it.
//!
//! It used to be a bar across the top of the window: the headline, every word
//! of the release notes run together on one line, and the buttons squeezed in
//! beside them. It read as clutter and nobody read the notes. This is a box in
//! the middle of the window, the way Revu says it has an update: which
//! version, what is new in it laid out as it was written, and two buttons.
//!
//! It never restarts anything on its own. It waits for somebody to choose, and
//! "Later" leaves the new version to take over the next time the program
//! opens. It does not open under a mouse button that is held down, so a
//! markup being dragged is finished before it appears.

use egui::{Color32, Key, Modifiers, RichText, Stroke};
use ui::chrome::Theme;

/// A paragraph of the release notes, as it is laid out in the box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// The first line: what the release is, in a sentence.
    Lead(String),
    /// A section: `STUDIO`, or `## Also in this release`.
    Heading(String),
    /// A paragraph, its lines joined back together.
    Para(String),
    /// A point in a list: its mark (`•`, or `2.`) and its words.
    Point { mark: String, text: String },
}

/// The release notes as they were written, read back into paragraphs.
///
/// Notes are typed wrapped at about seventy-five characters, with a blank line
/// between paragraphs, a section's name on a line of its own in capitals (or
/// after `##`), and a list as lines that start with `-`. Each paragraph's
/// lines are joined again so the box can wrap them to its own width.
pub fn read(notes: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut open: Option<Block> = None;
    let close = |open: &mut Option<Block>, blocks: &mut Vec<Block>| {
        if let Some(block) = open.take() {
            let block = match block {
                Block::Para(text) if blocks.is_empty() => Block::Lead(text),
                other => other,
            };
            blocks.push(block);
        }
    };
    for line in notes.lines() {
        let words = line.trim();
        if words.is_empty() {
            close(&mut open, &mut blocks);
            continue;
        }
        if let Some((mark, text)) = point(words) {
            close(&mut open, &mut blocks);
            open = Some(Block::Point { mark, text: text.to_string() });
            continue;
        }
        match &mut open {
            Some(Block::Para(text) | Block::Lead(text) | Block::Point { text, .. }) => {
                text.push(' ');
                text.push_str(words);
            }
            Some(Block::Heading(_)) | None => {
                close(&mut open, &mut blocks);
                if let Some(heading) = heading(words) {
                    blocks.push(Block::Heading(heading));
                } else {
                    open = Some(Block::Para(words.to_string()));
                }
            }
        }
    }
    close(&mut open, &mut blocks);
    blocks
}

/// A line that names a section: `## Like this`, or LIKE THIS on its own.
fn heading(line: &str) -> Option<String> {
    // `#4 rebar` starts a sentence; `## Also in this release` is a heading.
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes > 0 && line[hashes..].starts_with(' ') {
        let name = line[hashes..].trim();
        return (!name.is_empty()).then(|| name.to_string());
    }
    let letters = line.chars().filter(|c| c.is_alphabetic()).count();
    let capitals = letters >= 2
        && !line.chars().any(char::is_lowercase)
        && line.chars().count() <= 60
        && !line.ends_with('.');
    capitals.then(|| line.to_string())
}

/// A line that starts a point in a list: `- `, `* `, `• `, or `3. `.
fn point(line: &str) -> Option<(String, &str)> {
    for mark in ["- ", "* ", "• "] {
        if let Some(rest) = line.strip_prefix(mark) {
            return Some(("•".to_string(), rest.trim()));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if (1..=2).contains(&digits) {
        if let Some(rest) = line[digits..].strip_prefix(". ") {
            return Some((line[..=digits].to_string(), rest.trim()));
        }
    }
    None
}

/// What the new version is waiting on.
pub enum Offer<'a> {
    /// Downloaded, checked and in place: a restart puts it in use.
    Ready,
    /// Out, but this copy cannot put it in place itself; `why` when it tried.
    Download { why: Option<&'a str> },
}

/// What the box says to a copy that is not the installed one: a download
/// folder's copy, or one started from a menu entry somebody made by hand.
/// Such a copy never updates, so saying only "Download" left people on it.
pub const NOT_INSTALLED: &str = "This copy is not the installed one, so it does not update itself. \
     Download the new version, run the downloaded program once so it installs itself, \
     and from then on open Excalibur View from the Start menu or your applications menu."; // WHAT: the three steps, in order. WHY: running the download once is what installs it, and opening the installed copy from the menu is what keeps it up to date.

/// The page Download opens for `version`: that version's own release, or the
/// latest release's when there is no version to go by.
pub fn download_page(version: Option<&str>) -> Option<String> {
    hub::web::release_page(crate::trust::HOME_FEED, version) // WHAT: the offered version's release page, from the program's own feed. WHY: "latest" skips early releases, so an early-channel office sent there got the old version.
}

/// What somebody chose in the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Nothing yet: the box is still up.
    Open,
    /// Later, or Escape.
    Later,
    Restart,
    Download,
}

/// The box, in the middle of the window. Returns what was chosen this frame
/// and where the box is.
pub fn show(
    ctx: &egui::Context,
    theme: Theme,
    version: &str,
    running: &str,
    notes: &str,
    offer: Offer<'_>,
) -> (Choice, egui::Rect) {
    let screen = ctx.screen_rect();
    let width = (screen.width() - 48.0).clamp(280.0, 540.0);
    // Room for the heading and the buttons is kept, and the notes scroll in
    // what is left, so the box never runs off a small window.
    let notes_height = (screen.height() - 320.0).clamp(90.0, 380.0);
    let blocks = read(notes);
    let mut choice = Choice::Open;

    let modal = egui::Modal::new(egui::Id::new("update-box"))
        .frame(card(theme))
        .backdrop_color(Color32::from_black_alpha(110))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 4.0;

            // Which version, beside the mark.
            ui.horizontal(|ui| {
                let (mark, _) = ui.allocate_exact_size(egui::vec2(46.0, 46.0), egui::Sense::hover());
                ui::mark::draw(ui.painter(), mark, theme);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.add_space(1.0);
                    ui.label(RichText::new(format!("Excalibur View {version}")).size(19.0).strong().color(theme.text));
                    let state = match offer {
                        Offer::Ready => "Ready to install",
                        Offer::Download { .. } => "Ready to download",
                    };
                    ui.label(RichText::new(state).size(12.5).strong().color(theme.accent_text));
                    ui.label(RichText::new(format!("You have {running}.")).size(11.5).color(theme.faint));
                });
            });

            if !blocks.is_empty() {
                ui.add_space(14.0);
                ui.label(RichText::new("What's new").size(12.0).strong().color(theme.faint));
                ui.add_space(2.0);
                egui::Frame::new()
                    .fill(theme.sunken)
                    .stroke(Stroke::new(1.0_f32, theme.line))
                    .corner_radius(if theme.classic { 0 } else { 6 })
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        // A bar that is always there when there is more to
                        // read, not one that only shows under the mouse: on
                        // the light palette that one cannot be seen at all.
                        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
                        egui::ScrollArea::vertical()
                            .max_height(notes_height)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                draw(ui, &blocks, theme);
                            });
                    });
            }

            ui.add_space(12.0);
            let (said, colour) = match offer {
                Offer::Ready => (
                    "Restart now saves what is open and opens it all again in the new version. \
                     Later leaves it to take over the next time Excalibur View opens."
                        .to_string(),
                    theme.faint,
                ),
                Offer::Download { why: Some(why) } => (format!("It could not be installed here: {why}"), theme.warn),
                Offer::Download { why: None } => (NOT_INSTALLED.to_string(), theme.faint), // WHAT: what to do with a copy that is not the installed one. WHY: said step by step, so the next version is one that updates itself.
            };
            ui.add(egui::Label::new(RichText::new(said).size(11.5).color(colour)).wrap());

            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if secondary(ui, theme, "Later").clicked() {
                    choice = Choice::Later;
                }
                match offer {
                    Offer::Ready => {
                        if primary(ui, theme, "Restart now")
                            .on_hover_text("Saves what is open, restarts, and opens it all again.")
                            .clicked()
                        {
                            choice = Choice::Restart;
                        }
                    }
                    Offer::Download { .. } => {
                        if primary(ui, theme, "Download").clicked() {
                            choice = Choice::Download;
                        }
                    }
                }
            });
        });

    // Escape is Later. A click outside the box is not: somebody clicking on
    // the drawing as it appears has not read it yet.
    if choice == Choice::Open
        && modal.is_top_modal
        && !modal.any_popup_open
        && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
    {
        choice = Choice::Later;
    }
    (choice, modal.response.rect)
}

/// Whether the box may open this frame: not while a mouse button is held, so
/// a markup half dragged is finished first. Once open it stays open.
pub fn may_open(ctx: &egui::Context, was_open: bool) -> bool {
    was_open || !ctx.input(|i| i.pointer.any_down())
}

/// A new version being fetched: a small card in the corner, out of the way,
/// until it is ready to install. Returns true when it is hidden.
pub fn getting(ctx: &egui::Context, theme: Theme, version: &str) -> bool {
    let mut hide = false;
    egui::Area::new(egui::Id::new("update-getting"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -40.0))
        .show(ctx, |ui| {
            card(theme).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(format!("Getting Excalibur View {version}")).size(12.5).strong());
                        ui.label(
                            RichText::new("It says when it is ready to install.").size(11.0).color(theme.faint),
                        );
                    });
                    ui.add_space(6.0);
                    if ui
                        .add(egui::Button::new(RichText::new("×").size(17.0).color(theme.faint)).frame(false))
                        .on_hover_text("Hide")
                        .clicked()
                    {
                        hide = true;
                    }
                });
            });
        });
    hide
}

/// "Check for Updates" being asked, or its answer: a small box in the middle
/// of the window. Returns true when it is closed.
pub fn answer(ctx: &egui::Context, theme: Theme, note: Option<&str>) -> bool {
    let width = (ctx.screen_rect().width() - 48.0).clamp(260.0, 440.0);
    let mut done = false;
    let modal = egui::Modal::new(egui::Id::new("update-answer"))
        .frame(card(theme))
        .backdrop_color(Color32::from_black_alpha(80))
        .show(ctx, |ui| {
            ui.set_width(width);
            match note {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.add_space(4.0);
                        ui.label(RichText::new("Checking for updates…").size(13.5));
                    });
                }
                Some(note) => {
                    ui.horizontal_top(|ui| {
                        let (mark, _) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
                        ui::mark::draw(ui.painter(), mark, theme);
                        ui.add_space(6.0);
                        ui.vertical(|ui| {
                            ui.add(egui::Label::new(RichText::new(note).size(13.0)).wrap());
                        });
                    });
                }
            }
            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let clicked = match note {
                    None => secondary(ui, theme, "Close").clicked(),
                    Some(_) => primary(ui, theme, "OK").clicked(),
                };
                if clicked {
                    done = true;
                }
            });
        });
    if modal.is_top_modal
        && !modal.any_popup_open
        && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
    {
        done = true;
    }
    done
}

/// The notes, laid out: the first line larger, each section's name in the
/// accent, paragraphs wrapped to the box, and lists with their marks hung.
fn draw(ui: &mut egui::Ui, blocks: &[Block], theme: Theme) {
    for (n, block) in blocks.iter().enumerate() {
        match block {
            Block::Lead(text) => {
                ui.add(egui::Label::new(RichText::new(text).size(14.5).strong().color(theme.text)).wrap());
                ui.add_space(6.0);
            }
            Block::Heading(text) => {
                if n > 0 {
                    ui.add_space(8.0);
                }
                ui.label(RichText::new(text).size(11.5).strong().color(theme.accent_text));
                ui.add_space(1.0);
            }
            Block::Para(text) => {
                ui.add(egui::Label::new(RichText::new(text).size(13.0).color(theme.text)).wrap());
                ui.add_space(6.0);
            }
            Block::Point { mark, text } => {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.add_sized(
                        egui::vec2(14.0, 16.0),
                        egui::Label::new(RichText::new(mark).size(13.0).color(theme.accent_text)),
                    );
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(RichText::new(text).size(13.0).color(theme.text)).wrap());
                    });
                });
                ui.add_space(3.0);
            }
        }
    }
}

/// The card the boxes are drawn on: the window's own colour, a hairline, round
/// corners (square in Classic) and a soft shadow so it stands off the drawing.
fn card(theme: Theme) -> egui::Frame {
    egui::Frame::new()
        .fill(theme.chrome)
        .stroke(Stroke::new(1.0_f32, theme.line))
        .corner_radius(if theme.classic { 0 } else { 10 })
        .inner_margin(egui::Margin::same(20))
        .shadow(egui::epaint::Shadow {
            offset: [0, 10],
            blur: 28,
            spread: 0,
            color: Color32::from_black_alpha(if theme.classic { 40 } else { 90 }),
        })
}

fn primary(ui: &mut egui::Ui, theme: Theme, text: &str) -> egui::Response {
    // Classic draws every button raised and grey whatever it is filled with,
    // so there the one to press is told by its type, as Windows did.
    if theme.classic {
        return ui.add(
            egui::Button::new(RichText::new(text).size(13.0).strong().color(theme.text))
                .stroke(Stroke::new(1.0_f32, theme.text))
                .min_size(egui::vec2(112.0, 32.0)),
        );
    }
    ui.add(
        egui::Button::new(RichText::new(text).size(13.0).strong().color(Color32::WHITE))
            .fill(theme.accent)
            .min_size(egui::vec2(112.0, 32.0)),
    )
}

fn secondary(ui: &mut egui::Ui, theme: Theme, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(13.0).color(theme.text))
            .fill(theme.bar)
            .stroke(Stroke::new(1.0_f32, theme.line))
            .min_size(egui::vec2(88.0, 32.0)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(s: &str) -> Block {
        Block::Para(s.into())
    }

    #[test]
    fn notes_written_with_capital_headings_come_back_as_paragraphs() {
        let notes = "The Studio shows the office's jobs as the office keeps its files.\n\
                     \n\
                     STUDIO\n\
                     \n\
                     Projects another program sends sit on shelves: Active Jobs,\n\
                     Bids by year, and Lost Bids by year.\n\
                     \n\
                     A drawing issued again replaces only the one of the same name.\n\
                     \n\
                     FOR OTHER PROGRAMS\n\
                     \n\
                     The API only grows.\n";
        assert_eq!(
            read(notes),
            vec![
                Block::Lead("The Studio shows the office's jobs as the office keeps its files.".into()),
                Block::Heading("STUDIO".into()),
                para("Projects another program sends sit on shelves: Active Jobs, Bids by year, and Lost Bids by year."),
                para("A drawing issued again replaces only the one of the same name."),
                Block::Heading("FOR OTHER PROGRAMS".into()),
                para("The API only grows."),
            ]
        );
    }

    #[test]
    fn notes_written_as_markdown_keep_their_sections_and_lists() {
        // WHAT: a sample of release notes written as Markdown. WHY: the same words as notes-0.6.4.txt, so the test reads what a real release says.
        let notes = "The Mac version — and the first public release since 0.6.1, so this carries\r\n\
                     0.6.2 and 0.6.3 with it.\r\n\
                     \r\n\
                     ## Also in this release\r\n\
                     \r\n\
                     - The assistant works in the free app (0.6.2).\r\n\
                     - Clicking a button no longer switched the keyboard off. Any button that had\r\n  \
                       been clicked kept the focus.\r\n\
                     1. First\r\n";
        assert_eq!(
            read(notes),
            vec![
                Block::Lead(
                    "The Mac version — and the first public release since 0.6.1, so this carries 0.6.2 and 0.6.3 with it."
                        .into()
                ),
                Block::Heading("Also in this release".into()),
                Block::Point { mark: "•".into(), text: "The assistant works in the free app (0.6.2).".into() }, // WHAT: the list point the sample's first line becomes. WHY: proves a Markdown list line is read as a point.
                Block::Point {
                    mark: "•".into(),
                    text: "Clicking a button no longer switched the keyboard off. Any button that had been clicked kept the focus."
                        .into()
                },
                Block::Point { mark: "1.".into(), text: "First".into() },
            ]
        );
    }

    #[test]
    fn capitals_only_name_a_section_at_the_start_of_one() {
        // Inside a paragraph, a line in capitals is part of it; a sentence in
        // capitals with a full stop is a sentence; a heading right above its
        // paragraph, with no blank line between, still heads it.
        let notes = "Lead.\n\nSome words\nPDF AND API\n\nTHIS IS SHOUTED.\n\n#4 rebar is counted.\n\nHELP MENU\nThe two commands.";
        assert_eq!(
            read(notes),
            vec![
                Block::Lead("Lead.".into()),
                para("Some words PDF AND API"),
                para("THIS IS SHOUTED."),
                para("#4 rebar is counted."),
                Block::Heading("HELP MENU".into()),
                para("The two commands."),
            ]
        );
    }

    #[test]
    fn no_notes_is_nothing_to_show() {
        assert!(read("").is_empty());
        assert!(read("  \n\n \r\n").is_empty());
        // A release whose notes start with a section has no lead.
        assert_eq!(read("STUDIO\nShelves."), vec![Block::Heading("STUDIO".into()), para("Shelves.")]);
    }

    fn a_pass(ctx: &egui::Context, size: egui::Vec2, events: Vec<egui::Event>, mut build: impl FnMut(&egui::Context)) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| ui.label("a sheet"));
            build(ctx);
        });
    }

    const LONG: &str = include_str!("../../../notes-0.6.4.txt");

    #[test]
    fn the_box_is_in_the_middle_of_the_window_and_never_bigger_than_it() {
        for (size, notes) in [
            (egui::vec2(1600.0, 900.0), "Short.\n\nSTUDIO\n\nShelves."),
            (egui::vec2(1600.0, 900.0), LONG),
            (egui::vec2(640.0, 480.0), LONG),
        ] {
            let ctx = egui::Context::default();
            let mut at = egui::Rect::NOTHING;
            // Drawn the way the program draws when nothing is happening: a
            // frame only when one is asked for. The notes are measured over
            // the first few, and the box is placed again as they are; it must
            // end up in the middle without the mouse moving, and then stop
            // asking.
            let mut passes = 0;
            loop {
                a_pass(&ctx, size, vec![], |ctx| {
                    at = show(ctx, Theme::dark(), "0.6.15", "0.6.14", notes, Offer::Ready).1;
                });
                passes += 1;
                if !ctx.has_requested_repaint() || passes == 20 {
                    break;
                }
            }
            assert!(passes < 20, "the box never stopped asking to be drawn");
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            assert!((at.center() - screen.center()).length() < 2.0, "{at:?} is not in the middle of {screen:?}");
            assert!(screen.contains_rect(at), "{at:?} runs off {screen:?}");
            assert!(at.width() > 250.0 && at.height() > 150.0, "{at:?} is too small to read");
        }
    }

    #[test]
    fn escape_is_later_and_until_somebody_chooses_the_box_stays() {
        let ctx = egui::Context::default();
        let size = egui::vec2(1280.0, 800.0);
        let mut chose = Choice::Later;
        for _ in 0..2 {
            a_pass(&ctx, size, vec![], |ctx| {
                chose = show(ctx, Theme::dark(), "0.6.15", "0.6.14", LONG, Offer::Download { why: None }).0;
            });
            assert_eq!(chose, Choice::Open);
        }
        let escape = egui::Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        a_pass(&ctx, size, vec![escape], |ctx| {
            chose = show(ctx, Theme::dark(), "0.6.15", "0.6.14", LONG, Offer::Download { why: None }).0;
        });
        assert_eq!(chose, Choice::Later);
    }

    #[test]
    fn a_click_outside_the_box_does_not_put_it_off() {
        let ctx = egui::Context::default();
        let size = egui::vec2(1280.0, 800.0);
        let corner = egui::pos2(20.0, 20.0);
        let press = |pressed| egui::Event::PointerButton {
            pos: corner,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let mut chose = Choice::Open;
        for events in [vec![], vec![egui::Event::PointerMoved(corner), press(true)], vec![press(false)], vec![]] {
            a_pass(&ctx, size, events, |ctx| {
                chose = show(ctx, Theme::dark(), "0.6.15", "0.6.14", "Short.", Offer::Ready).0;
            });
            assert_eq!(chose, Choice::Open);
        }
    }

    #[test]
    fn it_waits_for_a_mouse_button_to_be_let_go_before_it_opens() {
        let ctx = egui::Context::default();
        let size = egui::vec2(1280.0, 800.0);
        let at = egui::pos2(300.0, 300.0);
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let mut may = (true, true);
        a_pass(&ctx, size, vec![egui::Event::PointerMoved(at), press(true)], |ctx| {
            may = (may_open(ctx, false), may_open(ctx, true));
        });
        assert_eq!(may, (false, true), "held down: not opened, but not closed if it was already open");
        a_pass(&ctx, size, vec![press(false)], |ctx| may = (may_open(ctx, false), may_open(ctx, true)));
        a_pass(&ctx, size, vec![], |ctx| may = (may_open(ctx, false), may_open(ctx, true)));
        assert_eq!(may, (true, true));
    }

    // WHAT: Download opens the offered version's release, not "latest". WHY: an early-channel office was sent to the full release it already had.
    #[test]
    fn download_opens_the_offered_version_s_own_release() {
        let page = download_page(Some("0.6.23")).expect("the program's feed is GitHub's"); // WHAT: the page for a known version. WHY: it is what the button opens.
        assert!(page.ends_with("/releases/tag/v0.6.23"), "{page}"); // WHAT: that version's tag. WHY: the early release is there, and "latest" skips it.
        let fallback = download_page(None).expect("the program's feed is GitHub's"); // WHAT: the page with no version known. WHY: the button must still go somewhere useful.
        assert!(fallback.ends_with("/releases/latest"), "{fallback}"); // WHAT: the latest release. WHY: the fallback is the old behaviour.
    }

    // WHAT: the not-installed message says what to do. WHY: a person running a copy from a hand-made menu entry never got updates and was not told why or how.
    #[test]
    fn a_copy_that_is_not_installed_is_told_how_to_get_one_that_is() {
        assert!(NOT_INSTALLED.contains("run the downloaded program once"), "{NOT_INSTALLED}"); // WHAT: run it once. WHY: that is what installs it.
        assert!(NOT_INSTALLED.contains("installs itself"), "{NOT_INSTALLED}"); // WHAT: says it installs itself. WHY: so nobody looks for an installer step that is not there.
        assert!(NOT_INSTALLED.contains("Start menu") && NOT_INSTALLED.contains("applications menu"), "{NOT_INSTALLED}"); // WHAT: where to open it from on each system. WHY: opening the installed copy is what keeps it up to date.
    }
}
