//! The words of a text box, typed on the sheet: what the box being typed in
//! holds, and how it lays them out.
//!
//! The box being typed in and the saved drawing break their lines in the same
//! places because they are broken by the same wrap, [`annot::text::breaks`].
//! egui is not left to wrap the words by its own measure: the spaces the
//! wrap breaks at are shown to it as line breaks instead. That keeps every
//! character where it is — a space shown as a line break is still one
//! character — so the cursor, the selection and the words typed all land on
//! the character they were meant for.

use egui::Color32;

/// The words as the box being typed in holds them: every line break as `\n`.
///
/// Revu ends a text box's lines with `\r`, and text pasted from Windows
/// brings `\r\n`. egui breaks a line only at `\n`, so a box Revu wrote opened
/// as one long line with invisible characters in it that the cursor stepped
/// over one at a time.
pub fn editable(contents: &str) -> String {
    contents.replace("\r\n", "\n").replace('\r', "\n") // WHAT: every kind of line break made a \n. WHY: the one egui breaks a line at.
}

/// Everything about how the words being typed look on the screen, worked out
/// once a frame from the box's own setting.
#[derive(Clone)]
pub struct Look {
    pub setting: annot::text::Setting,
    /// The letters' size and the gap between lines, in screen points.
    pub size: f32,
    pub leading: f32,
    pub ink: Color32,
    pub family: egui::FontFamily,
    pub halign: egui::Align,
    /// How wide the words may run before they wrap, in the sheet's points.
    /// Infinite for a box that grows with its words.
    pub wrap_points: f64,
    /// The same, on the screen.
    pub wrap_screen: f32,
}

impl Look {
    /// The look of a box's words at a zoom, wrapping at `inner` screen points,
    /// or not at all for a box that grows with them.
    pub fn of(setting: annot::text::Setting, zoom: f32, inner: f32, grows: bool) -> Look {
        let size = (setting.size as f32 * zoom).max(2.0); // WHAT: the letters' size on screen. WHY: the words are typed at the size they will be drawn.
        let leading = setting.size as f32 * 1.18 * zoom; // WHAT: the gap from one line to the next. WHY: the same as the saved drawing's.
        let ink = Color32::from_rgb(
            (setting.colour[0] * 255.0) as u8,
            (setting.colour[1] * 255.0) as u8,
            (setting.colour[2] * 255.0) as u8,
        ); // WHAT: the words' colour. WHY: as they will be drawn.
        let family = match (setting.family, setting.bold) {
            (annot::text::Family::Courier, _) => egui::FontFamily::Monospace,
            (_, true) => egui::FontFamily::Name(ui::chrome::SHEET_TEXT_BOLD.into()),
            (_, false) => egui::FontFamily::Name(ui::chrome::SHEET_TEXT.into()),
        }; // WHAT: the type the words are shown in. WHY: Courier as fixed-width, the rest in a type with Helvetica's widths.
        let halign = match setting.across {
            annot::text::Across::Left => egui::Align::LEFT,
            annot::text::Across::Middle => egui::Align::Center,
            annot::text::Across::Right => egui::Align::RIGHT,
        }; // WHAT: how the lines line up. WHY: as the box says.
        let wrap_points = if grows || zoom <= 0.0 {
            f64::INFINITY
        } else {
            (inner / zoom) as f64
        }; // WHAT: the wrap width in the sheet's points. WHY: the wrap measures in the drawing's own units, as the saved drawing does.
        let wrap_screen = if grows { f32::INFINITY } else { inner };
        Look {
            setting,
            size,
            leading,
            ink,
            family,
            halign,
            wrap_points,
            wrap_screen,
        }
    }
}

/// The words as egui is shown them: the same characters, with each space a
/// line is wrapped at shown as a line break.
pub fn shown(text: &str, look: &Look) -> String {
    let mut chars: Vec<char> = text.chars().collect(); // WHAT: the words as characters. WHY: the wrap's places are characters.
    for line in annot::text::breaks(text, look.wrap_points, &look.setting) {
        if !line.typed && line.next > line.end {
            chars[line.next - 1] = '\n'; // WHAT: the last space before the next line shown as a line break. WHY: one character for one, so every place in the words stays the same place; egui ends the line exactly where the saved drawing will.
        }
    }
    chars.into_iter().collect()
}

/// The layout of the words being typed, for a `TextEdit`'s layouter.
pub fn job(text: &str, look: &Look) -> egui::text::LayoutJob {
    let line = egui::Stroke::new((look.size * 0.06).max(0.6), look.ink); // WHAT: the weight of an underline or a strike. WHY: in proportion to the letters, as drawn.
    let mut job = egui::text::LayoutJob::single_section(
        shown(text, look),
        egui::TextFormat {
            font_id: egui::FontId::new(look.size, look.family.clone()),
            color: look.ink,
            line_height: Some(look.leading),
            italics: look.setting.italic,
            underline: if look.setting.underline {
                line
            } else {
                egui::Stroke::NONE
            },
            strikethrough: if look.setting.strike {
                line
            } else {
                egui::Stroke::NONE
            },
            ..Default::default()
        },
    );
    // WHAT: egui wraps only what the wrap could not: a word too long for the box, broken in the middle. WHY: a little slack, so a line the wrap says fits is never broken again by egui's own measure.
    job.wrap.max_width = look.wrap_screen + look.size;
    job.halign = look.halign;
    job
}

#[cfg(test)]
mod tests {
    //! The box being typed in, driven the way the program drives it: a real
    //! egui context, a real `TextEdit` with this module's layout, and the
    //! words read from the markup and written back into it every frame, as
    //! `type_in_place` does.

    use super::*;
    use egui::{Event, Key, Modifiers};

    /// A text box on a sheet and the box it is being typed in.
    struct Typing {
        ctx: egui::Context,
        markup: annot::Markup,
        id: egui::Id,
        grows: bool,
        rows: usize,
    }

    impl Typing {
        fn new(words: &str, width: f64) -> Typing {
            let ctx = egui::Context::default();
            ui::chrome::fonts(&ctx); // WHAT: the program's own type. WHY: the sheet's text family has to exist.
            let mut markup = annot::Markup::new(annot::Subtype::FreeText);
            markup.set_box([100.0, 100.0, 100.0 + width, 400.0]);
            markup.set_contents(words);
            let mut typing = Typing {
                ctx,
                markup,
                id: egui::Id::new("typing"),
                grows: false,
                rows: 0,
            };
            typing.frame(Vec::new());
            typing.frame(Vec::new());
            typing
        }

        /// One frame of the program: the words read from the markup, typed
        /// into, and written back if they changed.
        fn frame(&mut self, events: Vec<Event>) {
            let modifiers = events
                .iter()
                .find_map(|e| match e {
                    Event::Key { modifiers, .. } => Some(*modifiers),
                    _ => None,
                })
                .unwrap_or_default();
            let raw = egui::RawInput {
                events,
                modifiers,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 900.0),
                )),
                ..Default::default()
            };
            let markup = &mut self.markup;
            let (id, grows) = (self.id, self.grows);
            let mut rows = 0;
            let _ = self.ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let b = markup.drawn_box().unwrap();
                    let setting = annot::text::Setting::of(markup);
                    let inner = (b[2] - b[0] - 4.0 - 2.0 * markup.width().max(0.0)) as f32;
                    let look = Look::of(setting, 1.0, inner, grows);
                    let mut text = editable(&markup.contents());
                    let before = text.clone();
                    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _w: f32| {
                        ui.fonts(|f| f.layout_job(job(buffer.as_str(), &look)))
                    };
                    let edit = egui::TextEdit::multiline(&mut text)
                        .id(id)
                        .desired_width(inner)
                        .layouter(&mut layouter)
                        .show(ui);
                    rows = edit.galley.rows.len();
                    if !edit.response.has_focus() {
                        edit.response.request_focus();
                    }
                    if text != before {
                        markup.set_contents(&text);
                    }
                });
            });
            self.rows = rows;
        }

        fn words(&self) -> String {
            self.markup.contents()
        }

        fn key(&mut self, key: Key, modifiers: Modifiers) {
            self.frame(vec![
                Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
                Event::Key {
                    key,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers,
                },
            ]);
        }

        fn press(&mut self, key: Key, times: usize) {
            for _ in 0..times {
                self.key(key, Modifiers::NONE);
            }
        }

        fn type_in(&mut self, words: &str) {
            self.frame(vec![Event::Text(words.into())]);
        }

        /// The cursor to the very start of the words.
        fn home(&mut self) {
            self.key(Key::Home, Modifiers::COMMAND);
        }

        fn end(&mut self) {
            self.key(Key::End, Modifiers::COMMAND);
        }
    }

    #[test]
    fn words_typed_in_the_middle_go_in_the_middle() {
        let mut t = Typing::new("BEAM ABOVE", 400.0);
        t.home();
        t.press(Key::ArrowRight, 4);
        t.type_in(" W12x26");
        assert_eq!(t.words(), "BEAM W12x26 ABOVE");
    }

    #[test]
    fn a_selection_typed_over_is_replaced() {
        let mut t = Typing::new("SEE DETAIL 3", 400.0);
        t.end();
        t.key(Key::ArrowLeft, Modifiers::SHIFT);
        t.type_in("7");
        assert_eq!(t.words(), "SEE DETAIL 7");
        t.home();
        for _ in 0..3 {
            t.key(Key::ArrowRight, Modifiers::SHIFT);
        }
        t.type_in("REFER TO");
        assert_eq!(t.words(), "REFER TO DETAIL 7");
    }

    #[test]
    fn backspace_and_delete_join_lines() {
        let mut t = Typing::new("FIRST\nSECOND", 400.0);
        t.home();
        t.press(Key::ArrowDown, 1);
        t.key(Key::Home, Modifiers::NONE);
        t.press(Key::Backspace, 1);
        assert_eq!(t.words(), "FIRSTSECOND");
        t.press(Key::Enter, 1);
        assert_eq!(t.words(), "FIRST\nSECOND");
        t.press(Key::ArrowLeft, 1);
        t.press(Key::Delete, 1);
        assert_eq!(t.words(), "FIRSTSECOND");
    }

    #[test]
    fn a_box_revu_wrote_opens_with_its_lines_and_edits_cleanly() {
        // WHAT: Revu's \r line breaks. WHY: they opened as one long line, and the cursor stepped over invisible characters, so Backspace at the start of a line deleted nothing that could be seen.
        let mut t = Typing::new("FIRST\rSECOND", 400.0);
        assert_eq!(t.rows, 2, "two lines, as Revu shows them");
        t.home();
        t.press(Key::ArrowDown, 1);
        t.key(Key::Home, Modifiers::NONE);
        t.press(Key::Backspace, 1);
        assert_eq!(t.words(), "FIRSTSECOND");
    }

    #[test]
    fn accents_degrees_fractions_and_emoji_are_one_character_each() {
        let mut t = Typing::new("", 400.0);
        t.type_in("Café 90° ±½\" 😀");
        assert_eq!(t.words(), "Café 90° ±½\" 😀");
        t.press(Key::Backspace, 2);
        assert_eq!(t.words(), "Café 90° ±½\"");
        t.home();
        t.press(Key::ArrowRight, 3);
        t.press(Key::Delete, 1);
        assert_eq!(t.words(), "Caf 90° ±½\"");
        t.press(Key::ArrowRight, 4);
        t.type_in("¼");
        assert_eq!(t.words(), "Caf 90°¼ ±½\"");
    }

    #[test]
    fn undo_and_redo_take_the_typing_back_and_forward() {
        let mut t = Typing::new("BEAM", 400.0);
        t.end();
        t.type_in(" ABOVE");
        assert_eq!(t.words(), "BEAM ABOVE");
        t.key(Key::Z, Modifiers::COMMAND);
        assert_eq!(t.words(), "BEAM");
        t.key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
        assert_eq!(t.words(), "BEAM ABOVE");
    }

    #[test]
    fn words_composed_with_an_input_method_arrive_once() {
        let mut t = Typing::new("A", 400.0);
        t.end();
        t.frame(vec![Event::Ime(egui::ImeEvent::Enabled)]);
        t.frame(vec![Event::Ime(egui::ImeEvent::Preedit("e".into()))]);
        t.frame(vec![Event::Ime(egui::ImeEvent::Preedit("é".into()))]);
        t.frame(vec![Event::Ime(egui::ImeEvent::Commit("é".into()))]);
        t.frame(vec![Event::Ime(egui::ImeEvent::Disabled)]);
        assert_eq!(t.words(), "Aé");
    }

    #[test]
    fn the_box_being_typed_in_breaks_its_lines_where_the_saved_drawing_does() {
        // WHAT: the box shows exactly the lines the saved drawing will draw. WHY: they were broken by two different measures, and the words jumped about when the box was finished.
        let words = "FIELD VERIFY ALL DIMENSIONS BEFORE FABRICATION.  SEE DETAIL 3 ON S-501 FOR THE CONNECTION";
        for width in [90.0, 140.0, 220.0] {
            let t = Typing::new(words, width);
            let setting = annot::text::Setting::of(&t.markup);
            let saved = annot::text::wrap(words, width - 4.0, &setting);
            assert_eq!(t.rows, saved.len(), "at {width}: {saved:?}");
        }
    }

    #[test]
    fn typing_at_a_wrap_puts_the_letter_where_the_cursor_is() {
        // WHAT: a space shown as a line break is still the space. WHY: if the shown words had a character more or less than the words, every letter after the first wrap went in one place off.
        let mut t = Typing::new("ONE TWO THREE FOUR FIVE SIX SEVEN EIGHT", 80.0);
        assert!(t.rows > 2);
        t.end();
        t.type_in("!");
        assert_eq!(t.words(), "ONE TWO THREE FOUR FIVE SIX SEVEN EIGHT!");
        t.home();
        t.press(Key::ArrowDown, 1);
        t.key(Key::Home, Modifiers::NONE);
        t.type_in("*");
        let words = t.words();
        assert_eq!(words.chars().filter(|c| *c == '*').count(), 1);
        assert_eq!(
            words.replace('*', ""),
            "ONE TWO THREE FOUR FIVE SIX SEVEN EIGHT!"
        );
        let at = words.find('*').unwrap();
        assert!(
            words[..at].ends_with(' '),
            "at the start of a line, after a space: {words:?}"
        );
    }

    #[test]
    fn a_new_size_or_font_keeps_every_character() {
        let mut t = Typing::new("W12x26 ±½\" 😀\nSECOND", 200.0);
        for (family, size) in [
            (annot::text::Family::Courier, 24.0),
            (annot::text::Family::Times, 7.0),
            (annot::text::Family::Helvetica, 12.0),
        ] {
            let mut setting = annot::text::Setting::of(&t.markup);
            setting.family = family;
            setting.size = size;
            setting.onto(&mut t.markup);
            t.frame(Vec::new());
            assert_eq!(t.words(), "W12x26 ±½\" 😀\nSECOND");
        }
    }

    #[test]
    fn shown_words_are_the_words_character_for_character() {
        let setting = annot::text::Setting::default();
        let look = Look::of(setting, 1.0, 60.0, false);
        for words in [
            "ONE TWO THREE FOUR FIVE",
            "A  B   C    D     E      F",
            "WWWWWWWWWWWWWWWWWWWW",
            "Ä😀 ½ ° ± é ".repeat(6).as_str(),
        ] {
            let shown = shown(words, &look);
            assert_eq!(shown.chars().count(), words.chars().count(), "{words:?}");
            for (a, b) in shown.chars().zip(words.chars()) {
                assert!(
                    a == b || (a == '\n' && b.is_whitespace()),
                    "{words:?} became {shown:?}"
                );
            }
        }
    }
}
