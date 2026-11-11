//! Grips on a markup once it is placed.
//!
//! Select something and it shows where it can be taken hold of. A box, an
//! ellipse, a cloud, a text box or a stamp has one on each corner and each
//! side: drag one and that side moves, so it grows or shrinks from there. A
//! line has one on each end, and a shape drawn point by point has one on every
//! corner. A callout has its box's grips and one on the tip of its leader, so
//! the words and the thing they point at can each go somewhere else.
//!
//! Everything is worked out in sheet space, the way the sheet is on screen,
//! and turned back into the file's own coordinates at the end, so a grip does
//! what it looks like it does on a sheet that is turned round.
//!
//! A drag always starts again from the markup as it was when the button went
//! down, rather than adding up small changes, so letting go of a grip where it
//! started leaves the markup exactly as it was.

use annot::{Frame, Markup, Subtype};

/// Which sides of a box a grip moves. In sheet space, where `y` runs down the
/// sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sides {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Sides {
    const fn of(left: bool, right: bool, top: bool, bottom: bool) -> Sides {
        Sides { left, right, top, bottom }
    }

    pub fn is_corner(self) -> bool {
        (self.left || self.right) && (self.top || self.bottom)
    }
}

/// Something on a markup that can be taken hold of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    /// A corner or a side of its box.
    Side(Sides),
    /// One of its own points: the end of a line, the corner of an area.
    Point(usize),
    /// A point on a callout's leader. 0 is the tip.
    Leader(usize),
}

/// Where a grip is, in sheet space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Handle {
    pub grip: Grip,
    pub at: [f64; 2],
}

/// A grip being dragged.
#[derive(Clone, Debug)]
pub struct Gripping {
    /// Which markup, in the drawing's list.
    pub index: usize,
    pub grip: Grip,
    /// Where the button went down, in sheet space.
    pub from: [f64; 2],
    /// The markup as it was then. Every step of the drag starts from this.
    pub original: Markup,
    /// Whether anything has changed yet. The undo step is taken then, so a
    /// click on a grip that doesn't move is not an undo step.
    pub moved: bool,
    /// The rest of its group as it was, for the parts that follow: a Cloud+'s
    /// leader keeps its tip on the cloud as the cloud is resized.
    pub partners: Vec<(usize, Markup)>,
}

/// The smallest a box can be dragged down to, in points. Small enough for a
/// bolt tag, big enough to still be taken hold of.
pub const LEAST: f64 = 4.0;

/// Whether a markup is a picture: a signature, a stamp, a photograph. Its
/// shape is kept as it's resized unless Shift says otherwise, because a
/// stretched signature is not the signature any more.
pub fn is_picture(markup: &Markup) -> bool {
    markup.picture.is_some() || markup.subtype() == Subtype::Stamp
}

/// A box's corners and the middles of its sides.
fn box_handles(b: [f64; 4], corners_only: bool) -> Vec<Handle> {
    let (x0, y0, x1, y1) = (b[0], b[1], b[2], b[3]);
    let (mx, my) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
    let mut out = vec![
        Handle { grip: Grip::Side(Sides::of(true, false, true, false)), at: [x0, y0] },
        Handle { grip: Grip::Side(Sides::of(false, true, true, false)), at: [x1, y0] },
        Handle { grip: Grip::Side(Sides::of(false, true, false, true)), at: [x1, y1] },
        Handle { grip: Grip::Side(Sides::of(true, false, false, true)), at: [x0, y1] },
    ];
    if !corners_only {
        out.extend([
            Handle { grip: Grip::Side(Sides::of(false, false, true, false)), at: [mx, y0] },
            Handle { grip: Grip::Side(Sides::of(false, true, false, false)), at: [x1, my] },
            Handle { grip: Grip::Side(Sides::of(false, false, false, true)), at: [mx, y1] },
            Handle { grip: Grip::Side(Sides::of(true, false, false, false)), at: [x0, my] },
        ]);
    }
    out
}

/// The box round some points.
pub fn bounding(points: &[[f64; 2]]) -> Option<[f64; 4]> {
    let first = points.first()?;
    let mut b = [first[0], first[1], first[0], first[1]];
    for p in points {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    Some(b)
}

fn leader(markup: &Markup) -> Vec<[f64; 2]> {
    markup
        .dict
        .get("CL")
        .map(|o| o.numbers())
        .unwrap_or_default()
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| [c[0], c[1]])
        .collect()
}

fn set_leader(markup: &mut Markup, points: &[[f64; 2]]) {
    markup.set(
        "CL",
        pdf::Object::Array(
            points
                .iter()
                .flat_map(|p| [pdf::Object::real(p[0]), pdf::Object::real(p[1])])
                .collect(),
        ),
    );
}

/// The grips a markup offers, in sheet space.
pub fn handles(markup: &Markup, frame: &Frame) -> Vec<Handle> {
    let on_sheet = |points: &[[f64; 2]]| frame.points_to_sheet(points);
    match markup.subtype() {
        Subtype::Square | Subtype::Circle | Subtype::FreeText | Subtype::Stamp | Subtype::Link => {
            let Some(b) = bounding(&on_sheet(&markup.points())) else {
                return Vec::new();
            };
            let mut out = box_handles(b, is_picture(markup));
            // A callout's leader: its tip, and its bend when it has one. The
            // end on the box follows the box.
            let line = on_sheet(&leader(markup));
            if line.len() >= 2 {
                for (i, p) in line.iter().enumerate().take(line.len() - 1) {
                    out.push(Handle { grip: Grip::Leader(i), at: *p });
                }
            }
            out
        }
        Subtype::Ink => match bounding(&on_sheet(&markup.points())) {
            Some(b) => box_handles(b, false),
            None => Vec::new(),
        },
        Subtype::Line | Subtype::PolyLine | Subtype::Polygon => on_sheet(&markup.points())
            .into_iter()
            .enumerate()
            .map(|(i, at)| Handle { grip: Grip::Point(i), at })
            .collect(),
        _ => Vec::new(),
    }
}

/// Which grip, if any, is under a point, nearest first. `reach` is in sheet
/// units.
pub fn grip_at(handles: &[Handle], at: [f64; 2], reach: f64) -> Option<Grip> {
    handles
        .iter()
        .map(|h| (h.grip, (h.at[0] - at[0]).powi(2) + (h.at[1] - at[1]).powi(2)))
        .filter(|(_, d)| *d <= reach * reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(g, _)| g)
}

/// The pointer that says what a grip will do.
pub fn cursor(grip: Grip) -> egui::CursorIcon {
    match grip {
        Grip::Side(s) => match (s.left || s.right, s.top || s.bottom) {
            (true, false) => egui::CursorIcon::ResizeHorizontal,
            (false, true) => egui::CursorIcon::ResizeVertical,
            _ if (s.left && s.top) || (s.right && s.bottom) => egui::CursorIcon::ResizeNwSe,
            _ => egui::CursorIcon::ResizeNeSw,
        },
        Grip::Point(_) | Grip::Leader(_) => egui::CursorIcon::Crosshair,
    }
}

/// A box with some of its sides moved by `by`. It never turns inside out and
/// never gets smaller than `least`. With `keep` a corner grip keeps the box's
/// shape, following whichever way the pointer went further.
pub fn resize(start: [f64; 4], sides: Sides, by: [f64; 2], keep: bool, least: f64) -> [f64; 4] {
    let mut b = start;
    if sides.left {
        b[0] = (start[0] + by[0]).min(start[2] - least);
    }
    if sides.right {
        b[2] = (start[2] + by[0]).max(start[0] + least);
    }
    if sides.top {
        b[1] = (start[1] + by[1]).min(start[3] - least);
    }
    if sides.bottom {
        b[3] = (start[3] + by[1]).max(start[1] + least);
    }
    if keep && sides.is_corner() {
        let (w0, h0) = ((start[2] - start[0]).max(1e-9), (start[3] - start[1]).max(1e-9));
        let (w, h) = (b[2] - b[0], b[3] - b[1]);
        let scale = (w / w0).max(h / h0).max(least / w0.min(h0));
        let (nw, nh) = (w0 * scale, h0 * scale);
        if sides.left {
            b[0] = b[2] - nw;
        } else {
            b[2] = b[0] + nw;
        }
        if sides.top {
            b[1] = b[3] - nh;
        } else {
            b[3] = b[1] + nh;
        }
    }
    b
}

/// The nearest point on a box's edge to somewhere, for where a callout's
/// leader meets its box.
pub fn onto_edge(b: [f64; 4], p: [f64; 2]) -> [f64; 2] {
    let clamped = [p[0].clamp(b[0], b[2]), p[1].clamp(b[1], b[3])];
    if clamped != p {
        return clamped;
    }
    // Inside the box: out to whichever side is closest.
    let to = [p[0] - b[0], b[2] - p[0], p[1] - b[1], b[3] - p[1]];
    let nearest = (0..4).min_by(|a, c| to[*a].total_cmp(&to[*c])).unwrap_or(0);
    match nearest {
        0 => [b[0], p[1]],
        1 => [b[2], p[1]],
        2 => [p[0], b[1]],
        _ => [p[0], b[3]],
    }
}

/// Puts the end of a callout's leader back on its box, after the box or the
/// rest of the leader has moved.
pub fn reattach_leader(markup: &mut Markup) {
    let mut line = leader(markup);
    if line.len() < 2 {
        return;
    }
    let Some(b) = markup.drawn_box() else { return };
    let last = line.len() - 1;
    line[last] = onto_edge(b, line[last - 1]);
    set_leader(markup, &line);
}

/// The markup as it would be with a grip dragged `by` (sheet space) from
/// where it started. `keep` is Shift, which keeps a box's shape; a picture
/// keeps its shape unless Shift is held.
pub fn dragged(original: &Markup, frame: &Frame, grip: Grip, by: [f64; 2], shift: bool) -> Markup {
    let mut m = original.clone();
    match grip {
        Grip::Side(sides) => {
            let sheet_points = frame.points_to_sheet(&original.points());
            let Some(start) = bounding(&sheet_points) else { return m };
            let keep = if is_picture(original) { !shift } else { shift };
            let now = resize(start, sides, by, keep, LEAST);
            let (sx, sy) = (
                (now[2] - now[0]) / (start[2] - start[0]).max(1e-9),
                (now[3] - now[1]) / (start[3] - start[1]).max(1e-9),
            );
            let map = |p: [f64; 2]| [now[0] + (p[0] - start[0]) * sx, now[1] + (p[1] - start[1]) * sy];
            match original.subtype() {
                Subtype::Ink => {
                    let strokes: Vec<Vec<[f64; 2]>> = original
                        .ink()
                        .into_iter()
                        .map(|stroke| {
                            stroke
                                .into_iter()
                                .map(|p| frame.to_pdf(map(frame.to_sheet(p))))
                                .collect()
                        })
                        .collect();
                    m.set_ink(&strokes);
                    if let Some(b) = bounding(&strokes.concat()) {
                        m.set_box(b);
                    }
                }
                Subtype::Line | Subtype::PolyLine | Subtype::Polygon => {
                    let moved: Vec<[f64; 2]> =
                        sheet_points.iter().map(|p| frame.to_pdf(map(*p))).collect();
                    set_points(&mut m, &moved);
                }
                _ => {
                    let corners = [frame.to_pdf([now[0], now[1]]), frame.to_pdf([now[2], now[3]])];
                    if let Some(b) = bounding(&corners) {
                        // A callout's leader has to be kept: only the box
                        // changes, and the leader's end follows it.
                        let line = leader(original);
                        m.set_box(b);
                        if line.len() >= 2 {
                            set_leader(&mut m, &line);
                            reattach_leader(&mut m);
                        }
                    }
                }
            }
        }
        Grip::Point(i) => {
            let mut points = original.points();
            if let Some(p) = points.get_mut(i) {
                let s = frame.to_sheet(*p);
                *p = frame.to_pdf([s[0] + by[0], s[1] + by[1]]);
                set_points(&mut m, &points);
            }
        }
        Grip::Leader(i) => {
            let mut line = leader(original);
            if let Some(p) = line.get_mut(i) {
                let s = frame.to_sheet(*p);
                *p = frame.to_pdf([s[0] + by[0], s[1] + by[1]]);
                set_leader(&mut m, &line);
                // The words stay where they are; the box keeps its own size.
                if let Some(b) = original.drawn_box() {
                    m.set_box(b);
                }
                reattach_leader(&mut m);
            }
        }
    }
    // Drawn again from the new shape the next time it is saved — unless it
    // is a picture that can only be kept, which the new rectangle carries.
    if !m.keeps_its_appearance() {
        m.dict.remove("AP");
    }
    m
}

/// A callout whose box and leader follow a cloud that has changed size: the
/// tip stays on the same part of the cloud's edge, and the leader's end stays
/// on the box.
pub fn follow_cloud(callout: &Markup, was: [f64; 4], now: [f64; 4]) -> Markup {
    let mut m = callout.clone();
    let mut line = leader(callout);
    if let Some(tip) = line.first_mut() {
        let (sx, sy) = (
            (now[2] - now[0]) / (was[2] - was[0]).max(1e-9),
            (now[3] - now[1]) / (was[3] - was[1]).max(1e-9),
        );
        *tip = [now[0] + (tip[0] - was[0]) * sx, now[1] + (tip[1] - was[1]) * sy];
        set_leader(&mut m, &line);
        if let Some(b) = callout.drawn_box() {
            m.set_box(b);
        }
        reattach_leader(&mut m);
        m.dict.remove("AP");
    }
    m
}

/// Moves a callout's box and the end of its leader on the box, leaving its
/// tip where it points.
pub fn move_box_only(m: &mut Markup, dx: f64, dy: f64) {
    let line = leader(m);
    m.move_by(dx, dy);
    if let Some(tip) = line.first() {
        let mut moved = leader(m);
        if let Some(first) = moved.first_mut() {
            *first = *tip;
        }
        set_leader(m, &moved);
        reattach_leader(m);
    }
}

/// Sizes a text box to its words, the way a box put down with a click grows
/// as it is typed into: as wide as its longest line, as tall as its lines,
/// with the same room off the edges the saved drawing leaves.
///
/// It is held by the side its leader comes in on, so a callout's words grow
/// away from what they point at, and the leader stays on the middle of that
/// side. A box with no leader is held by its top left corner, where it was
/// put. Comes back true when the box changed.
pub fn fit_to_words(markup: &mut Markup, text: &str) -> bool {
    let Some(b) = markup.drawn_box() else { return false };
    let setting = annot::text::Setting::of(markup);
    let pad = 2.0 + markup.width().max(0.0);
    let lines = annot::text::wrap(text, f64::INFINITY, &setting); // WHAT: the lines as typed, split at every kind of line break. WHY: splitting at \n alone made a box Revu wrote with \r one long line wide.
    let longest = lines
        .iter()
        .map(|line| annot::content::width_of_in(line, setting.size, setting.family, setting.bold)) // WHAT: each line's width in its own family. WHY: a Courier box was sized as Helvetica.
        .fold(0.0_f64, f64::max);
    let lines = lines.len().max(1) as f64; // WHAT: how many lines. WHY: the box's height.
    // Room for the caret after the last letter, and never so narrow that
    // an empty box cannot be seen.
    let w = (longest + setting.size * 0.6).max(setting.size * 4.0) + pad * 2.0;
    let h = lines * setting.size * 1.18 + pad * 2.0;
    let mut line = leader(markup);
    let side = line.last().map(|end| {
        let away = [(end[0] - b[0]).abs(), (end[0] - b[2]).abs(), (end[1] - b[1]).abs(), (end[1] - b[3]).abs()];
        (0..4).min_by(|a, c| away[*a].total_cmp(&away[*c])).unwrap_or(0)
    });
    // PDF space: y runs up, so the top of the box is b[3].
    let now = match side {
        // Held on the right: grows to the left.
        Some(1) => [b[2] - w, b[3] - h, b[2], b[3]],
        // Held on the bottom: grows upward.
        Some(2) => [b[0], b[1], b[0] + w, b[1] + h],
        // Held on the left, the top, or nothing: grows right and down.
        _ => [b[0], b[3] - h, b[0] + w, b[3]],
    };
    if now.iter().zip(b.iter()).all(|(a, c)| (a - c).abs() < 0.01) {
        return false;
    }
    markup.set_box(now);
    if let (Some(side), Some(last)) = (side, line.last_mut()) {
        let (mx, my) = ((now[0] + now[2]) * 0.5, (now[1] + now[3]) * 0.5);
        *last = match side {
            0 => [now[0], my],
            1 => [now[2], my],
            2 => [mx, now[1]],
            _ => [mx, now[3]],
        };
        set_leader(markup, &line);
    }
    true
}

/// Makes a text box of a set width tall enough for its words as they wrap,
/// the way Revu's grows downward as the lines run past the bottom. It never
/// gets shorter than it was drawn: a box dragged out tall stays tall. Comes
/// back true when the box changed.
pub fn taller_for_words(markup: &mut Markup, text: &str) -> bool {
    let Some(b) = markup.drawn_box() else { return false };
    let setting = annot::text::Setting::of(markup);
    let pad = 2.0 + markup.width().max(0.0);
    let lines = annot::text::wrap(text, (b[2] - b[0] - pad * 2.0).max(1.0), &setting)
        .len()
        .max(1) as f64;
    // WHAT: no extra line for a trailing Enter. WHY: the wrap already gives a new line with nothing on it yet a line of its own, and counting it twice made the box a line taller than its words each time Enter ended the text.
    let needed = lines * setting.size * 1.18 + pad * 2.0;
    if needed <= b[3] - b[1] + 0.01 {
        return false;
    }
    // Downward, from where the words start, unless a leader holds the bottom.
    let held_below = leader(markup)
        .last()
        .is_some_and(|end| (end[1] - b[1]).abs() < (end[1] - b[3]).abs() && (end[1] - b[1]).abs() < 0.5);
    let now = if held_below {
        [b[0], b[1], b[2], b[1] + needed]
    } else {
        [b[0], b[3] - needed, b[2], b[3]]
    };
    markup.set_box(now);
    if !leader(markup).is_empty() {
        reattach_leader(markup);
    }
    true
}

/// Where a Cloud+'s leader starts and ends, for a cloud and a box: from the
/// cloud's edge nearest the box, to the box's edge nearest that.
pub fn leader_between(cloud: [f64; 4], words: [f64; 4]) -> Vec<[f64; 2]> {
    let middle = [(words[0] + words[2]) * 0.5, (words[1] + words[3]) * 0.5];
    let tip = onto_edge(cloud, middle);
    vec![tip, onto_edge(words, tip)]
}

/// Writes a run of points back into whichever key the markup keeps them in.
fn set_points(m: &mut Markup, points: &[[f64; 2]]) {
    match m.subtype() {
        Subtype::Line => {
            m.set(
                "L",
                pdf::Object::Array(
                    points
                        .iter()
                        .take(2)
                        .flat_map(|p| [pdf::Object::real(p[0]), pdf::Object::real(p[1])])
                        .collect(),
                ),
            );
        }
        _ => {
            m.set_vertices(points);
        }
    }
    if let Some(b) = bounding(points) {
        m.set_box(b);
    }
}

#[cfg(test)]
mod tests {
    fn words_box(at: [f64; 4]) -> Markup {
        let mut m = Markup::new(annot::Subtype::FreeText);
        m.set_box(at);
        m.set_width(1.0);
        m
    }

    #[test]
    fn a_clicked_text_box_grows_with_its_words_from_where_it_was_put() {
        let mut m = words_box([100.0, 480.0, 172.0, 500.0]);
        assert!(fit_to_words(&mut m, "Mech opening moved"));
        let b = m.drawn_box().unwrap();
        // Held at its top left.
        assert_eq!((b[0], b[3]), (100.0, 500.0));
        // Wide enough for the words and the border's room either side.
        let words = annot::content::width_of("Mech opening moved", 12.0, false);
        assert!(b[2] - b[0] >= words + 6.0, "{b:?}");
        // One line high.
        assert!((b[3] - b[1] - (12.0 * 1.18 + 6.0)).abs() < 1e-9, "{b:?}");
        // A second line makes it taller, downward, and no wider than it needs.
        assert!(fit_to_words(&mut m, "Mech opening moved\nRFI 014"));
        let two = m.drawn_box().unwrap();
        assert_eq!((two[0], two[3], two[2]), (100.0, 500.0, b[2]));
        assert!(two[1] < b[1]);
        // Nothing changes when nothing needs to.
        assert!(!fit_to_words(&mut m, "Mech opening moved\nRFI 014"));
    }

    #[test]
    fn a_dragged_text_box_keeps_its_width_and_grows_down_as_its_lines_wrap() {
        let mut m = words_box([100.0, 460.0, 200.0, 500.0]);
        assert!(!taller_for_words(&mut m, "Short"), "one line fits as drawn");
        assert_eq!(m.drawn_box().unwrap(), [100.0, 460.0, 200.0, 500.0]);
        let long = "Verify embed plate locations with the GC before fabrication starts.";
        assert!(taller_for_words(&mut m, long));
        let b = m.drawn_box().unwrap();
        assert_eq!((b[0], b[2], b[3]), (100.0, 200.0, 500.0), "same width, same top");
        let setting = annot::text::Setting::of(&m);
        let lines = annot::text::wrap(long, 100.0 - 6.0, &setting).len() as f64;
        assert!((b[3] - b[1] - (lines * 12.0 * 1.18 + 6.0)).abs() < 1e-9, "{b:?}");
    }

    #[test]
    fn a_callout_grows_away_from_what_it_points_at() {
        // The words to the left of the thing, the leader on their right side.
        let mut m = words_box([200.0, 490.0, 296.0, 510.0]);
        set_leader(&mut m, &[[400.0, 300.0], [296.0, 500.0]]);
        assert!(fit_to_words(&mut m, "SEE DETAIL 4 ON S-501 FOR THE PLATE"));
        let b = m.drawn_box().unwrap();
        assert_eq!(b[2], 296.0, "the side the leader comes in on stays put");
        assert!(b[0] < 200.0, "it grew to the left: {b:?}");
        let line = leader(&m);
        assert_eq!(line[0], [400.0, 300.0], "the tip does not move");
        assert_eq!(line[1], [296.0, (b[1] + b[3]) * 0.5], "the leader meets the middle of its side");
    }

    use super::*;

    fn flat() -> Frame {
        Frame { area: [0.0, 0.0, 1000.0, 800.0], rotation: 0 }
    }

    fn corner(left: bool, top: bool) -> Grip {
        Grip::Side(Sides { left, right: !left, top, bottom: !top })
    }

    fn a_box(subtype: Subtype) -> Markup {
        let mut m = Markup::new(subtype);
        m.set_box([100.0, 100.0, 300.0, 200.0]);
        m
    }

    #[test]
    fn a_side_moves_and_nothing_else_does() {
        let b = resize([0.0, 0.0, 100.0, 50.0], Sides::of(false, true, false, false), [20.0, 99.0], false, 4.0);
        assert_eq!(b, [0.0, 0.0, 120.0, 50.0]);
    }

    #[test]
    fn a_box_never_turns_inside_out() {
        let b = resize([0.0, 0.0, 100.0, 50.0], Sides::of(false, true, false, true), [-500.0, -500.0], false, 4.0);
        assert_eq!(b, [0.0, 0.0, 4.0, 4.0]);
    }

    #[test]
    fn keeping_the_shape_follows_the_bigger_pull() {
        // Pulled the bottom-right corner 50 across and 10 down: it grows by
        // half, both ways.
        let b = resize([0.0, 0.0, 100.0, 50.0], Sides::of(false, true, false, true), [50.0, 10.0], true, 4.0);
        assert_eq!(b, [0.0, 0.0, 150.0, 75.0]);
        // From the top-left, the bottom-right stays put.
        let b = resize([0.0, 0.0, 100.0, 50.0], Sides::of(true, false, true, false), [-100.0, 0.0], true, 4.0);
        assert_eq!(b, [-100.0, -50.0, 100.0, 50.0]);
    }

    #[test]
    fn dragging_a_boxs_corner_makes_it_bigger_on_the_sheet_and_in_the_file() {
        let frame = flat();
        let m = a_box(Subtype::Square);
        // Sheet y runs down; the top-right corner on screen is the file's
        // right and top.
        let grown = dragged(&m, &frame, corner(false, true), [50.0, -30.0], false);
        assert_eq!(grown.drawn_box(), Some([100.0, 100.0, 350.0, 230.0]));
        assert!(!grown.dict.has("AP"));
    }

    #[test]
    fn letting_go_where_it_started_leaves_it_as_it_was() {
        let frame = flat();
        let m = a_box(Subtype::Circle);
        let same = dragged(&m, &frame, corner(true, false), [0.0, 0.0], false);
        assert_eq!(same.drawn_box(), m.drawn_box());
    }

    #[test]
    fn a_box_grows_one_line_for_one_enter_and_reads_revu_line_breaks() {
        // WHAT: Enter at the end of the words makes room for one more line, not two. WHY: the trailing new line was counted twice, so the box grew a spare line every time.
        let mut m = Markup::new(Subtype::FreeText);
        m.set_box([0.0, 0.0, 200.0, 10.0]);
        let setting = annot::text::Setting::of(&m);
        let pad = 2.0 + m.width().max(0.0);
        assert!(taller_for_words(&mut m, "FIRST\n"));
        let b = m.drawn_box().unwrap();
        assert!((b[3] - b[1] - (2.0 * setting.size * 1.18 + pad * 2.0)).abs() < 0.01, "{b:?}");
        // WHAT: a box that grows with its words, from Revu with \r between its lines. WHY: it was made as wide as both lines end to end.
        let mut grown = Markup::new(Subtype::FreeText);
        grown.set_box([0.0, 0.0, 10.0, 10.0]);
        assert!(fit_to_words(&mut grown, "FIRST LINE\rSECOND"));
        let mut alone = Markup::new(Subtype::FreeText);
        alone.set_box([0.0, 0.0, 10.0, 10.0]);
        assert!(fit_to_words(&mut alone, "FIRST LINE\nSECOND"));
        assert_eq!(grown.drawn_box(), alone.drawn_box(), "\r and \n are the same line break");
    }

    #[test]
    fn a_picture_keeps_its_shape_unless_shift_is_held() {
        let frame = flat();
        let m = a_box(Subtype::Stamp);
        let kept = dragged(&m, &frame, corner(false, false), [100.0, 0.0], false);
        let b = kept.drawn_box().unwrap();
        assert!(((b[2] - b[0]) / (b[3] - b[1]) - 2.0).abs() < 1e-9, "{b:?}");
        let stretched = dragged(&m, &frame, corner(false, false), [100.0, 0.0], true);
        assert_eq!(stretched.drawn_box(), Some([100.0, 100.0, 400.0, 200.0]));
        // And it only offers its corners.
        assert_eq!(handles(&m, &frame).len(), 4);
        assert_eq!(handles(&a_box(Subtype::Square), &frame).len(), 8);
    }

    #[test]
    fn a_callouts_box_resizes_and_its_leader_follows() {
        let frame = flat();
        let mut m = a_box(Subtype::FreeText);
        set_leader(&mut m, &[[20.0, 20.0], [100.0, 150.0]]);
        // Pull the left side of the box out to the right: the box narrows,
        // the tip stays, the leader meets the box's new left side.
        let left = Grip::Side(Sides::of(true, false, false, false));
        let narrowed = dragged(&m, &frame, left, [80.0, 0.0], false);
        assert_eq!(narrowed.drawn_box(), Some([180.0, 100.0, 300.0, 200.0]));
        assert_eq!(leader(&narrowed), vec![[20.0, 20.0], [180.0, 100.0]]);
    }

    #[test]
    fn a_callouts_tip_can_point_somewhere_else() {
        let frame = flat();
        let mut m = a_box(Subtype::FreeText);
        set_leader(&mut m, &[[20.0, 20.0], [100.0, 150.0]]);
        let hs = handles(&m, &frame);
        assert!(hs.iter().any(|h| h.grip == Grip::Leader(0)));
        // Sheet y runs down, so +40 on screen is 40 lower in the file.
        let moved = dragged(&m, &frame, Grip::Leader(0), [0.0, 40.0], false);
        assert_eq!(leader(&moved)[0], [20.0, -20.0]);
        assert_eq!(moved.drawn_box(), m.drawn_box(), "the words stay where they are");
    }

    #[test]
    fn a_lines_end_goes_where_it_is_dragged() {
        let frame = flat();
        let mut m = Markup::new(Subtype::Line);
        m.set(
            "L",
            pdf::Object::Array([0.0, 0.0, 100.0, 0.0].iter().map(|v| pdf::Object::real(*v)).collect()),
        );
        let moved = dragged(&m, &frame, Grip::Point(1), [50.0, 0.0], false);
        assert_eq!(moved.points(), vec![[0.0, 0.0], [150.0, 0.0]]);
    }

    #[test]
    fn an_areas_corner_goes_where_it_is_dragged() {
        let frame = flat();
        let mut m = Markup::new(Subtype::Polygon);
        m.set_vertices(&[[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]]);
        let moved = dragged(&m, &frame, Grip::Point(2), [0.0, -100.0], false);
        assert_eq!(moved.points()[2], [100.0, 200.0]);
    }

    #[test]
    fn a_cloud_plus_leader_keeps_its_tip_on_the_cloud_as_the_cloud_grows() {
        let mut words = a_box(Subtype::FreeText);
        // Tip on the cloud's right side, halfway up.
        set_leader(&mut words, &[[50.0, 25.0], [100.0, 150.0]]);
        let was = [0.0, 0.0, 50.0, 50.0];
        let now = [0.0, 0.0, 80.0, 100.0];
        let followed = follow_cloud(&words, was, now);
        assert_eq!(leader(&followed)[0], [80.0, 50.0]);
        assert_eq!(followed.drawn_box(), words.drawn_box());
    }

    #[test]
    fn moving_a_callouts_box_leaves_its_tip_where_it_points() {
        let mut words = a_box(Subtype::FreeText);
        set_leader(&mut words, &[[20.0, 20.0], [100.0, 150.0]]);
        move_box_only(&mut words, 50.0, 0.0);
        assert_eq!(words.drawn_box(), Some([150.0, 100.0, 350.0, 200.0]));
        assert_eq!(leader(&words), vec![[20.0, 20.0], [150.0, 100.0]]);
    }

    #[test]
    fn a_new_leader_runs_from_the_cloud_to_the_box() {
        let line = leader_between([0.0, 0.0, 100.0, 100.0], [200.0, 40.0, 300.0, 60.0]);
        assert_eq!(line, vec![[100.0, 50.0], [200.0, 50.0]]);
    }

    #[test]
    fn freehand_scales_with_its_box() {
        let frame = flat();
        let mut m = Markup::new(Subtype::Ink);
        m.set_ink(&[vec![[0.0, 0.0], [100.0, 100.0]]]);
        let right = Grip::Side(Sides::of(false, true, false, false));
        let wider = dragged(&m, &frame, right, [100.0, 0.0], false);
        assert_eq!(wider.ink(), vec![vec![[0.0, 0.0], [200.0, 100.0]]]);
    }

    #[test]
    fn on_a_turned_sheet_a_grip_still_does_what_it_looks_like() {
        let frame = Frame { area: [0.0, 0.0, 1000.0, 800.0], rotation: 90 };
        let m = a_box(Subtype::Square);
        let before = bounding(&frame.points_to_sheet(&m.points())).unwrap();
        let right = Grip::Side(Sides::of(false, true, false, false));
        let after_markup = dragged(&m, &frame, right, [30.0, 0.0], false);
        let after = bounding(&frame.points_to_sheet(&after_markup.points())).unwrap();
        assert_eq!([after[0], after[1], after[3]], [before[0], before[1], before[3]]);
        assert!((after[2] - before[2] - 30.0).abs() < 1e-9);
    }

    #[test]
    fn the_nearest_grip_wins() {
        let frame = flat();
        let hs = handles(&a_box(Subtype::Square), &frame);
        // The top-left corner on the sheet is (100, 600).
        assert_eq!(grip_at(&hs, [102.0, 601.0], 5.0), Some(corner(true, true)));
        assert_eq!(grip_at(&hs, [150.0, 650.0], 5.0), None);
    }
}
