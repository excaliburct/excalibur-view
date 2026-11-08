//! Sign-offs: the signatures an office program (Fabwire) asked for on the
//! drawing that's open, signed here with a finger, a pen or the mouse.
//!
//! When the open drawing came from the office server and has lines waiting,
//! a small button over the drawing says how many. It opens the list down the
//! right, grouped as the program grouped them ("Stair 1"); each open line has
//! a Sign button, which opens the pad: draw the signature, check the name,
//! Sign. A signature is final once the server has it, so the pad says so.

use crate::app::App; // WHAT: the window's state. WHY: the list and the pad live on it.
use crate::server::Ask; // WHAT: asks of the office server. WHY: the lines and the signing go through the link.
use hub::{SignOff, SIGN_PAD}; // WHAT: a line, and the pad's size. WHY: the server's own shapes.

/// What the window knows about sign-offs on the open drawing.
#[derive(Default)]
pub struct SignOffs {
    pub set: Option<String>, // WHAT: the office set whose lines are held. WHY: asked again when another drawing comes up.
    pub lines: Vec<SignOff>, // WHAT: its lines, in the order asked for. WHY: the list.
    pub shown: bool,         // WHAT: whether the list is out. WHY: it takes room from the drawing.
    pub pad: Option<Pad>,    // WHAT: the pad, when somebody is signing. WHY: one at a time.
    pub said: Option<String>, // WHAT: the last thing to tell them (signed, or why not). WHY: shown at the list's top.
}

/// The signing pad, open on one line.
pub struct Pad {
    pub id: String,    // WHAT: the line being signed. WHY: what the signature goes to.
    pub label: String, // WHAT: what it says. WHY: shown on the pad, so they know what they sign for.
    pub name: String, // WHAT: the name written under it. WHY: printed on the form; filled in with theirs.
    pub strokes: Vec<Vec<[f64; 2]>>, // WHAT: the strokes so far, in the pad's units. WHY: what is sent.
    pub drawing: Vec<[f64; 2]>, // WHAT: the stroke under the finger now. WHY: added when the finger lifts.
    pub sending: bool,          // WHAT: sent, waiting for the server. WHY: one signature, once.
}

/// The lines grouped by heading, in the order the headings first come.
pub fn grouped(lines: &[SignOff]) -> Vec<(&str, Vec<&SignOff>)> {
    let mut out: Vec<(&str, Vec<&SignOff>)> = Vec::new(); // WHAT: headings with their lines. WHY: a traveler reads by page.
    for line in lines {
        match out.iter_mut().find(|(g, _)| *g == line.group.as_str()) {
            Some((_, list)) => list.push(line), // WHAT: under its heading. WHY: kept together.
            None => out.push((line.group.as_str(), vec![line])), // WHAT: a new heading. WHY: in first-seen order.
        }
    }
    out
}

/// A point on the screen as a point on the pad, kept inside it.
pub fn on_pad(at: egui::Pos2, pad: egui::Rect) -> [f64; 2] {
    let x = ((at.x - pad.min.x) / pad.width()) as f64 * SIGN_PAD.0; // WHAT: across, in pad units. WHY: the server's units, whatever the screen's size.
    let y = ((at.y - pad.min.y) / pad.height()) as f64 * SIGN_PAD.1; // WHAT: down. WHY: likewise.
    [x.clamp(0.0, SIGN_PAD.0), y.clamp(0.0, SIGN_PAD.1)] // WHAT: kept on the pad. WHY: the server refuses points off it.
}

impl App {
    /// Keeps the lines for the open drawing: asked for when another office drawing comes up.
    pub fn signoffs_follow(&mut self) {
        let set = self
            .doc()
            .and_then(|d| d.attached.as_ref().map(|a| a.set.clone())); // WHAT: the open drawing's office set, if it came from one. WHY: only those have lines.
        if set == self.signoffs.set {
            return; // WHAT: the same drawing. WHY: nothing to ask.
        }
        self.signoffs = crate::signoffs::SignOffs {
            set: set.clone(),
            ..Default::default()
        }; // WHAT: start clean. WHY: another drawing's lines never show on this one.
        if let Some(set) = set {
            self.ask(Ask::SignOffs(set)); // WHAT: its lines. WHY: shown if there are any.
        }
    }

    /// The small button over the drawing, when the open drawing has lines.
    pub fn signoffs_button(&mut self, ctx: &egui::Context) {
        if self.signoffs.lines.is_empty() || self.doc().is_none() {
            return; // WHAT: no button. WHY: nothing asked for on this drawing.
        }
        let open = self.signoffs.lines.iter().filter(|l| l.open()).count(); // WHAT: lines still to sign. WHY: the button says how many.
        let words = if open == 0 {
            "Sign-offs: all signed".to_string()
        } else {
            format!("Sign-offs: {open} to sign")
        }; // WHAT: what it says. WHY: at a glance from across the shop.
        egui::Area::new(egui::Id::new("signoffs-button"))
            .anchor(egui::Align2::RIGHT_TOP, [-70.0, 70.0]) // WHAT: top right, clear of the toolbars. WHY: over the drawing, where a finger finds it.
            .show(ctx, |ui| {
                let button = egui::Button::new(egui::RichText::new(words).size(16.0))
                    .min_size(egui::vec2(170.0, 40.0)); // WHAT: a big button. WHY: an iPad, a gloved finger.
                if ui.add(button).clicked() {
                    self.signoffs.shown = !self.signoffs.shown; // WHAT: the list out or away. WHY: one button for both.
                }
            });
    }

    /// The list down the right, when it's out.
    pub fn signoffs_panel(&mut self, ctx: &egui::Context) {
        if !self.signoffs.shown || self.signoffs.lines.is_empty() {
            return; // WHAT: nothing shown. WHY: put away, or nothing to sign.
        }
        let mut sign: Option<(String, String)> = None; // WHAT: a line someone tapped Sign on. WHY: the pad opens after drawing.
        let mut close = false; // WHAT: whether Close was tapped. WHY: likewise.
        egui::SidePanel::right("signoffs")
            .default_width(320.0) // WHAT: room for a line and its button. WHY: readable beside the drawing.
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Sign-offs"); // WHAT: what this is. WHY: plain.
                    if ui.button("Close").clicked() {
                        close = true; // WHAT: put away. WHY: the drawing gets its room back.
                    }
                });
                if let Some(said) = &self.signoffs.said {
                    ui.label(said); // WHAT: what just happened. WHY: signed, or why not.
                }
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (group, lines) in grouped(&self.signoffs.lines) {
                        if !group.is_empty() {
                            ui.label(egui::RichText::new(group).strong()); // WHAT: the heading. WHY: as the form groups them.
                        }
                        for line in lines {
                            ui.horizontal(|ui| {
                                match line.status.as_str() {
                                    "open" => {
                                        let tap = egui::Button::new("Sign")
                                            .min_size(egui::vec2(64.0, 34.0)); // WHAT: a finger-sized button. WHY: an iPad.
                                        if ui.add(tap).clicked() {
                                            sign = Some((line.id.clone(), line.label.clone()));
                                            // WHAT: this line. WHY: the pad opens on it.
                                        }
                                        ui.label(&line.label);
                                    }
                                    "signed" => {
                                        ui.label(egui::RichText::new("✔").strong()); // WHAT: a tick. WHY: done at a glance.
                                        ui.label(format!(
                                            "{} · {} {}",
                                            line.label,
                                            line.signer.as_deref().unwrap_or(""),
                                            line.signed
                                                .as_deref()
                                                .map(|s| s.get(..10).unwrap_or(s))
                                                .unwrap_or("")
                                        )); // WHAT: the line, who and the day. WHY: what the form will say.
                                    }
                                    _ => {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{} (no longer asked for)",
                                                line.label
                                            ))
                                            .weak(),
                                        ); // WHAT: a withdrawn line, greyed. WHY: shown so nobody hunts for it.
                                    }
                                }
                            });
                        }
                        ui.add_space(6.0); // WHAT: room between headings. WHY: easier to read.
                    }
                });
            });
        if close {
            self.signoffs.shown = false; // WHAT: put away. WHY: Close.
        }
        if let Some((id, label)) = sign {
            let name = self
                .standing
                .who
                .as_ref()
                .map(|u| u.name.clone())
                .unwrap_or_default(); // WHAT: their name, from their account. WHY: they only check it.
            self.signoffs.pad = Some(Pad {
                id,
                label,
                name,
                strokes: Vec::new(),
                drawing: Vec::new(),
                sending: false,
            }); // WHAT: the pad, clean. WHY: one signature.
            self.signoffs.said = None; // WHAT: the last message cleared. WHY: about something else.
        }
    }

    /// The pad, when someone is signing.
    pub fn signoffs_pad(&mut self, ctx: &egui::Context) {
        let Some(mut pad) = self.signoffs.pad.take() else {
            return; // WHAT: no pad. WHY: nobody is signing.
        };
        let set = self.signoffs.set.clone().unwrap_or_default(); // WHAT: the set. WHY: its lines are read again after signing.
        let (mut keep, mut send) = (true, false); // WHAT: whether the pad stays, and whether to send. WHY: acted on after drawing.
        egui::Window::new("Sign")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]) // WHAT: in the middle. WHY: where the hand is.
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(&pad.label).size(18.0).strong()); // WHAT: what's being signed. WHY: nobody signs blind.
                let size = egui::vec2(560.0, 560.0 * (SIGN_PAD.1 / SIGN_PAD.0) as f32); // WHAT: the pad on screen, the server's shape. WHY: big enough for a finger.
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::drag()); // WHAT: the pad's area, following a drag. WHY: a finger or a pen draws by dragging.
                let painter = ui.painter_at(rect); // WHAT: drawing kept inside the pad. WHY: ink never spills.
                painter.rect_filled(rect, 6.0, egui::Color32::WHITE); // WHAT: white paper. WHY: as on paper.
                painter.line_segment(
                    [
                        egui::pos2(rect.min.x + 20.0, rect.max.y - 30.0),
                        egui::pos2(rect.max.x - 20.0, rect.max.y - 30.0),
                    ],
                    egui::Stroke::new(1.0_f32, egui::Color32::GRAY),
                ); // WHAT: the line to sign on. WHY: people sign on a line.
                if !pad.sending {
                    if let Some(at) = response.interact_pointer_pos() {
                        if response.dragged() || response.drag_started() {
                            let point = on_pad(at, rect); // WHAT: where the finger is, on the pad. WHY: the server's units.
                            if pad.drawing.last() != Some(&point) {
                                pad.drawing.push(point); // WHAT: added to the stroke. WHY: the line follows the finger.
                            }
                        }
                    }
                    if response.drag_stopped() && !pad.drawing.is_empty() {
                        pad.strokes.push(std::mem::take(&mut pad.drawing)); // WHAT: the stroke finished. WHY: the next touch starts another.
                    }
                }
                let to_screen = |p: &[f64; 2]| {
                    egui::pos2(
                        rect.min.x + (p[0] / SIGN_PAD.0) as f32 * rect.width(),
                        rect.min.y + (p[1] / SIGN_PAD.1) as f32 * rect.height(),
                    )
                }; // WHAT: a pad point on the screen. WHY: drawn where it was made.
                for stroke in pad.strokes.iter().chain(std::iter::once(&pad.drawing)) {
                    let points: Vec<egui::Pos2> = stroke.iter().map(to_screen).collect(); // WHAT: the stroke on screen. WHY: drawn.
                    painter.add(egui::Shape::line(
                        points,
                        egui::Stroke::new(3.0_f32, egui::Color32::BLACK),
                    )); // WHAT: black ink. WHY: what the printed picture shows.
                }
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut pad.name).desired_width(300.0));
                    // WHAT: the name under it. WHY: printed on the form; theirs to correct.
                });
                ui.label(egui::RichText::new("Once signed, it can't be changed.").weak()); // WHAT: said before, not after. WHY: a signature is final.
                ui.horizontal(|ui| {
                    let ready =
                        !pad.strokes.is_empty() && !pad.name.trim().is_empty() && !pad.sending; // WHAT: drawn and named, not already sent. WHY: nothing half-done goes.
                    if ui
                        .add_enabled(
                            ready,
                            egui::Button::new("Sign").min_size(egui::vec2(100.0, 40.0)),
                        )
                        .clicked()
                    {
                        send = true; // WHAT: send it. WHY: Sign.
                    }
                    if ui
                        .add_enabled(
                            !pad.sending,
                            egui::Button::new("Clear").min_size(egui::vec2(80.0, 40.0)),
                        )
                        .clicked()
                    {
                        pad.strokes.clear(); // WHAT: start again. WHY: a smudge.
                        pad.drawing.clear();
                    }
                    if ui
                        .add_enabled(
                            !pad.sending,
                            egui::Button::new("Cancel").min_size(egui::vec2(80.0, 40.0)),
                        )
                        .clicked()
                    {
                        keep = false; // WHAT: put away, unsigned. WHY: Cancel.
                    }
                });
                if pad.sending {
                    ui.label("Signing…"); // WHAT: waiting on the server. WHY: so nobody taps twice.
                }
            });
        if send {
            pad.sending = true; // WHAT: sent once. WHY: the buttons wait for the answer.
            self.ask(Ask::Sign {
                set,
                id: pad.id.clone(),
                name: pad.name.trim().to_string(),
                strokes: pad.strokes.clone(),
            }); // WHAT: the signature to the server. WHY: kept there, final.
        }
        if keep {
            self.signoffs.pad = Some(pad); // WHAT: still open. WHY: drawing, or waiting.
        }
    }

    /// The server's answer about a set's lines.
    pub fn take_signoffs(&mut self, set: String, lines: Vec<SignOff>) {
        if self.signoffs.set.as_deref() != Some(set.as_str()) {
            return; // WHAT: for a drawing no longer open. WHY: never shown on another.
        }
        if let Some(pad) = &self.signoffs.pad {
            if pad.sending && lines.iter().any(|l| l.id == pad.id && !l.open()) {
                let line = lines.iter().find(|l| l.id == pad.id); // WHAT: the line just signed. WHY: said in plain words.
                self.signoffs.said = line.map(|l| {
                    format!(
                        "Signed: {} — {}.",
                        l.label,
                        l.signer.as_deref().unwrap_or("")
                    )
                }); // WHAT: what happened. WHY: plain.
                self.signoffs.pad = None; // WHAT: the pad put away. WHY: done.
            }
        }
        self.signoffs.lines = lines; // WHAT: the lines as the server has them. WHY: what's shown.
    }

    /// Signing didn't go through: said, and the pad left open to try again.
    pub fn signoff_failed(&mut self, why: String) {
        if let Some(pad) = self.signoffs.pad.as_mut() {
            pad.sending = false; // WHAT: ready again. WHY: they can try again or cancel.
        }
        self.signoffs.said = Some(why); // WHAT: the reason. WHY: in their words (already signed, no network).
        self.signoffs.shown = true; // WHAT: the list out, where the reason shows. WHY: they see it.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, group: &str, status: &str) -> SignOff {
        SignOff {
            id: id.into(),
            set: "set".into(),
            label: id.into(),
            group: group.into(),
            reference: id.into(),
            page: None,
            status: status.into(),
            signer: None,
            signed_by: None,
            signed: None,
            seq: 0,
        }
    }

    #[test]
    fn lines_are_grouped_under_their_headings_in_the_order_they_came() {
        let lines = [
            line("a", "Stair 1", "open"),
            line("b", "Stair 2", "open"),
            line("c", "Stair 1", "signed"),
        ];
        let groups = grouped(&lines);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "Stair 1");
        assert_eq!(
            groups[0]
                .1
                .iter()
                .map(|l| l.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "c"]
        );
        assert_eq!(groups[1].0, "Stair 2");
    }

    #[test]
    fn a_touch_is_kept_on_the_pad_in_its_units() {
        let pad = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(560.0, 196.0));
        assert_eq!(on_pad(egui::pos2(100.0, 50.0), pad), [0.0, 0.0]);
        assert_eq!(on_pad(egui::pos2(660.0, 246.0), pad), [400.0, 140.0]);
        assert_eq!(on_pad(egui::pos2(380.0, 148.0), pad), [200.0, 70.0]);
        assert_eq!(
            on_pad(egui::pos2(9999.0, -50.0), pad),
            [400.0, 0.0],
            "off the pad: kept on it"
        );
    }
}
