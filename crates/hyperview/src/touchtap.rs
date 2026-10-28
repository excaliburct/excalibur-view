//! Taps and strokes from a finger or a pen, read so that the tools work the
//! same under a finger as they do under a mouse.
//!
//! egui turns a finger into a pretend mouse, and on the frame the finger
//! lifts it says two things at once: the button came up, and the pointer has
//! gone (a finger cannot hover). The tools used to ask "where is the pointer
//! hovering" before doing anything, got no answer on exactly that frame, and
//! so never saw the tap that a click is made of. Every tool that puts points
//! down one at a time (setting the scale, length, area, angle, count) lost
//! its taps that way, and the pen never saw its stroke end, so the next
//! stroke was joined on to the last one.
//!
//! What is here decides, from what egui saw this frame, where a tool should
//! act and whether a tap puts a point down or finishes the shape. It is kept
//! apart from the canvas so it can be tested without a window.

/// What a tap, a click or a lift means to a tool that collects points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tap {
    /// Nothing to do this frame.
    Nothing,
    /// Put a point down where the pointer is.
    Place,
    /// The shape is finished.
    Finish,
}

/// What egui saw happen on the canvas this frame, as far as placing points
/// goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// A press and release that stayed put: a click, or a quick tap.
    pub clicked: bool,
    /// A second click soon after the first.
    pub double_clicked: bool,
    /// A right-click, or a long press on a touch screen.
    pub secondary_clicked: bool,
    /// A press that moved or was held, let go of this frame.
    pub drag_stopped: bool,
}

/// How near, in screen points, a second click has to be to the last point
/// for a double click to finish the shape. A finger is wider than a mouse
/// pointer, so it is given more room.
pub fn finish_reach(finger: bool) -> f32 {
    if finger { 24.0 } else { 10.0 } // WHAT: the room a double click is allowed. WHY: a fingertip lands less exactly than a mouse.
}

/// Reads one frame of input for a tool that puts points down.
///
/// With a mouse, a click puts a point down and a double click, or a right
/// click, finishes the shape, as before.
///
/// With a finger or a pen there is no hovering, so a press that slides
/// before it lifts is somebody aiming: the point goes where it lifts. A
/// finger held still puts its point down where it is held (egui calls that a
/// right click, and finishing the shape because somebody held their finger
/// still a moment too long lost what they had drawn). A double tap finishes
/// only when the second tap is on the last point; two quick taps in
/// different places are two points.
pub fn read(seen: Seen, finger: bool, near_last: bool) -> Tap {
    if seen.double_clicked && near_last {
        return Tap::Finish; // WHAT: a double click on the last point ends the shape. WHY: the way it has always ended, and a finger's double tap the same.
    }
    if finger {
        if seen.clicked || seen.drag_stopped || seen.secondary_clicked {
            return Tap::Place; // WHAT: a tap, a lift after aiming, or a finger held still puts a point down. WHY: a finger cannot hover, and egui reports a held finger as a right click and its lift as nothing at all.
        }
        return Tap::Nothing; // WHAT: still moving, or nothing happened. WHY: no point until the finger settles or lifts.
    }
    if seen.secondary_clicked {
        return Tap::Finish; // WHAT: a right click ends the shape. WHY: Revu's way, kept for the mouse.
    }
    if seen.clicked {
        return Tap::Place; // WHAT: an ordinary click puts a point down. WHY: unchanged for the mouse.
    }
    Tap::Nothing // WHAT: nothing a tool acts on. WHY: hovering and dragging alone place nothing.
}

/// Where a tool should act this frame: where the pointer hovers, or, on the
/// frame a finger lifts, where it lifted.
pub fn pointer(input: &egui::InputState) -> Option<egui::Pos2> {
    if let Some(at) = input.pointer.hover_pos() {
        return Some(at); // WHAT: a mouse, or a finger still down. WHY: the ordinary case.
    }
    let lifted = input.pointer.any_released() || input.pointer.any_click(); // WHAT: did a button come up this frame. WHY: only then is the last place still the place that matters.
    if lifted {
        return input.pointer.interact_pos(); // WHAT: where the finger was as it lifted. WHY: egui keeps it for one frame for exactly this.
    }
    None // WHAT: no pointer at all. WHY: nothing for a tool to act on.
}

/// Whether the person is working with a finger or a pen rather than a mouse,
/// from this frame's events and what was true before. A touch says finger; a
/// mouse moving or clicking with no touch says mouse; a frame with neither
/// leaves it as it was.
pub fn by_finger(events: &[egui::Event], was: bool) -> bool {
    if events.iter().any(|e| matches!(e, egui::Event::Touch { .. })) {
        return true; // WHAT: a touch this frame. WHY: a finger or a pen is on the screen.
    }
    let mouse = events.iter().any(|e| {
        matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. }) // WHAT: the pointer moved or clicked. WHY: with no touch alongside, that is a mouse or a trackpad.
    });
    if mouse {
        return false; // WHAT: a mouse was used. WHY: a tablet with a mouse or trackpad plugged in works as a desktop does.
    }
    was // WHAT: nothing either way. WHY: a frame woken by a timer says nothing about the hand.
}

/// The pressure a pen or finger pressed with this frame, nought to one, when
/// the screen reports it.
pub fn pressure(events: &[egui::Event]) -> Option<f32> {
    events.iter().rev().find_map(|e| match e {
        egui::Event::Touch { force, .. } => *force, // WHAT: the touch's force, when it has one. WHY: an Apple Pencil reports it; most fingers do not.
        _ => None, // WHAT: anything else. WHY: carries no pressure.
    })
}

/// The width a stroke is drawn at, from the pen's width and the pressure it
/// was drawn with. A PDF ink markup has one width for the whole stroke, so
/// this is decided once, from the stroke's average pressure: pressing lightly
/// draws a little finer, pressing hard a little bolder, and a screen that
/// reports no pressure draws the pen's own width.
pub fn stroke_width(pen: f64, pressures: &[f32]) -> f64 {
    if pressures.is_empty() || pen <= 0.0 {
        return pen; // WHAT: the pen's own width. WHY: no pressure was reported, or there is no width to vary.
    }
    let average = pressures.iter().map(|p| p.clamp(0.0, 1.0) as f64).sum::<f64>() / pressures.len() as f64; // WHAT: how hard the stroke was pressed, on average. WHY: one width for the whole stroke.
    pen * (0.6 + 0.8 * average) // WHAT: 0.6 times the width at the lightest touch, 1.4 times at the hardest. WHY: noticeably different, never vanishing or blotting.
}

/// A freehand stroke made smaller without changing what it looks like: points
/// closer to the line through their neighbours than `tolerance` (in the
/// sheet's own points) are dropped. A finger drawing for two seconds sends a
/// hundred and more points; most of them say nothing a line between the
/// others does not.
pub fn simplify_stroke(points: &[[f64; 2]], tolerance: f64) -> Vec<[f64; 2]> {
    if points.len() < 3 || tolerance <= 0.0 {
        return points.to_vec(); // WHAT: nothing to take out. WHY: two points are already a line.
    }
    takeoff::fill::simplify(points, tolerance) // WHAT: Ramer-Douglas-Peucker, the same one Dynamic Fill uses. WHY: one tested simplifier rather than two.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(clicked: bool, double: bool, secondary: bool, stopped: bool) -> Seen {
        Seen { clicked, double_clicked: double, secondary_clicked: secondary, drag_stopped: stopped } // WHAT: one frame's events. WHY: short to write in each test.
    }

    #[test]
    fn a_mouse_click_places_and_a_double_click_or_right_click_finishes() {
        assert_eq!(read(seen(true, false, false, false), false, false), Tap::Place); // WHAT: a click. WHY: puts a point down, as always.
        assert_eq!(read(seen(true, true, false, false), false, true), Tap::Finish); // WHAT: a double click on the last point. WHY: ends the shape.
        assert_eq!(read(seen(false, false, true, false), false, false), Tap::Finish); // WHAT: a right click. WHY: ends the shape, as always.
        assert_eq!(read(seen(false, false, false, true), false, false), Tap::Nothing); // WHAT: a mouse drag let go. WHY: dragging with a mouse places nothing, as before.
    }

    #[test]
    fn a_finger_lifting_after_aiming_or_a_long_press_puts_a_point_down() {
        assert_eq!(read(seen(true, false, false, false), true, false), Tap::Place); // WHAT: a quick tap. WHY: a point.
        assert_eq!(read(seen(false, false, false, true), true, false), Tap::Place); // WHAT: a slide then a lift. WHY: the point goes where it lifted.
        assert_eq!(read(seen(false, false, true, false), true, false), Tap::Place); // WHAT: a finger held still. WHY: a slow tap is a point, and must not finish the shape and lose the points.
        assert_eq!(read(seen(false, false, false, false), true, false), Tap::Nothing); // WHAT: a finger still moving. WHY: nothing until it settles or lifts.
    }

    #[test]
    fn two_quick_taps_in_different_places_are_two_points_not_a_finish() {
        assert_eq!(read(seen(true, true, false, false), true, false), Tap::Place); // WHAT: a fast second tap somewhere else. WHY: setting the scale is two quick taps, and must not end after one.
        assert_eq!(read(seen(true, true, false, false), true, true), Tap::Finish); // WHAT: a double tap on the last point. WHY: ends a run of points.
    }

    /// One frame of input through a real egui context, with a canvas that
    /// senses clicks and drags the way the sheet does. Returns what the
    /// canvas saw, where the pointer hovered, and where a tool should act.
    fn frame(ctx: &egui::Context, events: Vec<egui::Event>, time: f64) -> (Seen, Option<egui::Pos2>, Option<egui::Pos2>) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), // WHAT: a window-sized screen. WHY: the canvas needs room to be under the finger.
            time: Some(time), // WHAT: the frame's time. WHY: egui decides clicks, double clicks and long presses by it.
            events, // WHAT: what happened. WHY: the touch being tested.
            ..Default::default()
        };
        let mut out = (Seen::default(), None, None); // WHAT: filled in from inside the frame. WHY: the input is only readable there.
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = ui.max_rect(); // WHAT: the whole panel. WHY: it stands for the sheet.
                let r = ui.allocate_rect(rect, egui::Sense::click_and_drag()); // WHAT: sensed the way the sheet is. WHY: the same clicks and drags reach it.
                out.0 = Seen { clicked: r.clicked(), double_clicked: r.double_clicked(), secondary_clicked: r.secondary_clicked(), drag_stopped: r.drag_stopped() }; // WHAT: what the canvas saw. WHY: what the tools read.
                out.1 = ui.input(|i| i.pointer.hover_pos()); // WHAT: where egui says the pointer hovers. WHY: what the tools used to read.
                out.2 = ui.input(pointer); // WHAT: where a tool should act. WHY: what they read now.
            });
        });
        out
    }

    /// An egui with the canvas laid out once already, as it is after the
    /// first frame of a real window: egui finds what is under a finger from
    /// the frame before.
    fn sheet() -> egui::Context {
        let ctx = egui::Context::default(); // WHAT: a fresh egui. WHY: each test its own.
        let _ = frame(&ctx, Vec::new(), 0.5); // WHAT: one frame with nothing happening. WHY: so the canvas is there to be touched.
        ctx
    }

    fn touch(phase: egui::TouchPhase, at: egui::Pos2) -> egui::Event {
        egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos: at, force: Some(0.5) } // WHAT: one finger's event. WHY: what a tablet sends alongside the pretend mouse.
    }

    /// The events egui-winit sends for a finger coming down at `at`.
    fn finger_down(at: egui::Pos2) -> Vec<egui::Event> {
        vec![
            touch(egui::TouchPhase::Start, at), // WHAT: the touch. WHY: as a tablet sends it.
            egui::Event::PointerMoved(at), // WHAT: the pretend mouse moves there. WHY: egui-winit does this first.
            egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }, // WHAT: and presses. WHY: a finger down is a button down.
        ]
    }

    /// And for it lifting: the button comes up and the pointer is gone, in
    /// the same frame.
    fn finger_up(at: egui::Pos2) -> Vec<egui::Event> {
        vec![
            touch(egui::TouchPhase::End, at), // WHAT: the touch ends. WHY: as a tablet sends it.
            egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }, // WHAT: the button comes up. WHY: a lift is a release.
            egui::Event::PointerGone, // WHAT: the pointer goes. WHY: egui-winit sends this with every lift, so nothing hovers.
        ]
    }

    #[test]
    fn a_tap_is_seen_where_it_lifted_although_nothing_hovers_then() {
        let ctx = sheet(); // WHAT: a real egui. WHY: the bug was in how egui reports a lift.
        let at = egui::pos2(300.0, 200.0); // WHAT: where the finger taps. WHY: somewhere on the canvas.
        let _ = frame(&ctx, finger_down(at), 1.0); // WHAT: the finger comes down. WHY: the first half of a tap.
        let (seen, hover, tool) = frame(&ctx, finger_up(at), 1.05); // WHAT: and lifts. WHY: the frame a tap is decided on.
        assert!(seen.clicked, "a quick tap is a click"); // WHAT: egui does see the tap. WHY: the lost taps were never egui's.
        assert_eq!(hover, None, "nothing hovers on the frame a finger lifts"); // WHAT: the cause of the lost taps. WHY: this is what the tools used to ask.
        assert_eq!(tool, Some(at), "the tool is told where the finger lifted"); // WHAT: the fix. WHY: the tap lands where it was made.
        assert_eq!(read(seen, by_finger(&finger_up(at), false), false), Tap::Place); // WHAT: and it places a point. WHY: tap, tap, type is what setting the scale is.
    }

    #[test]
    fn a_second_quick_tap_elsewhere_is_a_second_point() {
        let ctx = sheet(); // WHAT: a real egui. WHY: double clicks are decided by its clock.
        let (a, b) = (egui::pos2(100.0, 100.0), egui::pos2(500.0, 300.0)); // WHAT: two ends of a dimension. WHY: far apart, as a scale is set.
        let _ = frame(&ctx, finger_down(a), 1.0); // WHAT: first tap down. WHY: the first end.
        let (first, _, _) = frame(&ctx, finger_up(a), 1.05); // WHAT: and up. WHY: the first point.
        assert_eq!(read(first, true, false), Tap::Place); // WHAT: the first end is a point. WHY: it was always so.
        let _ = frame(&ctx, finger_down(b), 1.15); // WHAT: second tap down, quickly. WHY: within egui's double-click time.
        let (second, _, tool) = frame(&ctx, finger_up(b), 1.2); // WHAT: and up. WHY: the second end.
        assert!(second.double_clicked, "egui calls two quick taps a double click wherever they are"); // WHAT: the trap. WHY: that used to finish the shape with one point.
        assert_eq!(tool, Some(b)); // WHAT: it is at the second end. WHY: where the finger lifted.
        assert_eq!(read(second, true, false), Tap::Place, "the second end is a point"); // WHAT: far from the first, so not a finish. WHY: the second tap must register.
    }

    #[test]
    fn a_finger_held_still_puts_one_point_down_and_finishes_nothing() {
        let ctx = sheet(); // WHAT: a real egui. WHY: a long press is decided by its clock.
        let at = egui::pos2(250.0, 250.0); // WHAT: where the finger rests. WHY: anywhere on the canvas.
        let _ = frame(&ctx, finger_down(at), 1.0); // WHAT: down. WHY: the start of a slow tap.
        let (held, hover, _) = frame(&ctx, vec![touch(egui::TouchPhase::Move, at)], 2.0); // WHAT: still down a second later. WHY: longer than egui's click time.
        assert!(held.secondary_clicked && !held.clicked, "egui calls a held finger a right click"); // WHAT: what egui reports. WHY: with a mouse a right click finishes the shape.
        assert_eq!(hover, Some(at)); // WHAT: the finger is still there. WHY: the point goes where it is held.
        assert_eq!(read(held, true, false), Tap::Place, "a slow tap is a point"); // WHAT: a point, not a finish. WHY: it used to end the shape and drop its points.
        let (lifted, _, _) = frame(&ctx, finger_up(at), 2.05); // WHAT: lifted. WHY: the end of the slow tap.
        assert_eq!(read(lifted, true, false), Tap::Nothing, "the lift after it adds nothing more"); // WHAT: one point for one touch. WHY: egui reports nothing on that lift, so no second point lands.
    }

    #[test]
    fn a_finger_or_a_mouse_is_told_from_the_events() {
        let at = egui::pos2(1.0, 1.0); // WHAT: anywhere. WHY: only the kind of event matters.
        assert!(by_finger(&finger_down(at), false)); // WHAT: a touch. WHY: a finger.
        assert!(!by_finger(&[egui::Event::PointerMoved(at)], true)); // WHAT: a mouse moving with no touch. WHY: a mouse.
        assert!(by_finger(&[], true)); // WHAT: nothing happened. WHY: stays as it was.
        assert!(!by_finger(&[], false)); // WHAT: nothing happened. WHY: stays as it was.
    }

    #[test]
    fn pressure_varies_the_width_and_no_pressure_keeps_the_pen() {
        assert_eq!(stroke_width(2.0, &[]), 2.0); // WHAT: no pressure reported. WHY: the pen's own width.
        assert!(stroke_width(2.0, &[0.1, 0.1]) < 2.0); // WHAT: a light stroke. WHY: comes out finer.
        assert!(stroke_width(2.0, &[1.0]) > 2.0); // WHAT: a hard one. WHY: comes out bolder.
        assert_eq!(pressure(&finger_down(egui::pos2(0.0, 0.0))), Some(0.5)); // WHAT: the touch's force is read. WHY: a pencil's pressure reaches the stroke.
    }

    #[test]
    fn a_long_wobbly_stroke_is_made_smaller_and_keeps_its_ends() {
        let wobbly: Vec<[f64; 2]> = (0..200).map(|i| [i as f64, if i % 2 == 0 { 0.0 } else { 0.05 }]).collect(); // WHAT: two hundred points along a line, wobbling a twentieth of a point. WHY: what a finger drawing a straight line sends.
        let small = simplify_stroke(&wobbly, 0.5); // WHAT: simplified at half a point. WHY: finer than anybody can see.
        assert!(small.len() < 10, "{} points kept", small.len()); // WHAT: nearly all gone. WHY: they said nothing the line does not.
        assert_eq!(small.first(), wobbly.first()); // WHAT: the start kept. WHY: the stroke starts where it did.
        assert_eq!(small.last(), wobbly.last()); // WHAT: the end kept. WHY: and ends where it did.
        let corner = vec![[0.0, 0.0], [50.0, 0.0], [50.0, 50.0]]; // WHAT: a stroke that turns a corner. WHY: the corner is the drawing.
        assert_eq!(simplify_stroke(&corner, 0.5), corner); // WHAT: nothing taken. WHY: every point there matters.
    }
}
