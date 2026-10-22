//! Comparing two issues of the same sheet, and laying one over the other.
//!
//! This is the thing a fabricator opens Revu for the morning a revision
//! arrives: Rev 2 of S-201 is on the desk and the only question is *what moved*.
//! A cloud round every change is worth an hour of squinting.
//!
//! How it works, and what it will not do.
//!
//! Both issues are rendered to ink at the same size and compared pixel by
//! pixel. Ink that is in one and not the other is a change. Changed pixels are
//! then grown into rectangles, because a person wants "this detail changed", not
//! four thousand dots.
//!
//! **It does not measure anything.** A comparison points at a region; it never
//! reports a length, an area or a weight for it. Two issues of a sheet can be at
//! different scales, the second can be a scan, and a number derived from a
//! difference between two rasters would be a guess with a decimal point on it.
//! Somebody looks at what changed and measures it with a tool, which is the
//! same rule as everywhere else: every quantity traces to a click.
//!
//! **It says when it could not line them up.** Two issues are often shifted by
//! a few thousandths — a different plotter, a re-scan, a title block that grew.
//! Comparing without correcting for that marks the entire sheet as changed,
//! which is worse than useless because it looks like an answer. So the offset is
//! found first, and if no offset makes the two agree, that is reported instead
//! of a page full of clouds.

/// One page as ink: true where there is line-work.
#[derive(Clone)]
pub struct Ink {
    pub width: usize,
    pub height: usize,
    pub on: Vec<bool>,
}

impl Ink {
    pub fn at(&self, x: usize, y: usize) -> bool {
        x < self.width && y < self.height && self.on[y * self.width + x]
    }

    pub fn count(&self) -> usize {
        self.on.iter().filter(|o| **o).count()
    }
}

/// How far apart two issues of a sheet are, and how well they agree once moved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alignment {
    pub dx: i32,
    pub dy: i32,
    /// Nought to one: how much of the smaller sheet's line-work is accounted
    /// for by the other at this offset. See [`ENOUGH_AGREEMENT`].
    pub agreement: f32,
}

/// Under this, the two sheets are not the same drawing — or are so different
/// that marking every change would mark everything.
///
/// Measured as *coverage*, not as similarity, and the difference matters. An
/// issue that adds a large detail shares all of the older sheet's line-work and
/// only half of the combined total; judging it on similarity would call a real
/// revision "not the same sheet" precisely when it has the most to say. So the
/// question asked is "is nearly all of one sheet present in the other", which a
/// revision answers yes to whether it added or removed, and two unrelated
/// sheets answer no to.
pub const ENOUGH_AGREEMENT: f32 = 0.70;

/// The most a sheet is assumed to have shifted, as a fraction of its width.
/// A re-plot moves things by a hair; a different sheet entirely moves nothing
/// into place at any offset, which is what the agreement test is for.
const MOST_SHIFT: f64 = 0.02;

/// Finds the offset that makes two issues of a sheet agree best.
///
/// Coarse to fine: a whole-sheet search at a big step, then a small search
/// around the best of those. Trying every offset at full resolution on a
/// 3000-pixel sheet is millions of comparisons for an answer that is nearly
/// always within a few pixels of zero.
pub fn align(a: &Ink, b: &Ink) -> Alignment {
    let reach = ((a.width as f64 * MOST_SHIFT) as i32).clamp(4, 64);

    // A small search is done exactly, because a coarse pass followed by a fine
    // one around its best answer can only ever reach two steps from where it
    // started — and on a small sheet that is less than the shift being looked
    // for. Under a few hundred offsets it is not worth being clever.
    if reach <= 8 {
        let mut best = Alignment { dx: 0, dy: 0, agreement: 0.0 };
        for dy in -reach..=reach {
            for dx in -reach..=reach {
                let how = agreement_every(a, b, dx, dy, 1);
                if how > best.agreement {
                    best = Alignment { dx, dy, agreement: how };
                }
            }
        }
        // The offset was chosen by overlap, which is the better discriminator
        // for *where*. What is reported is coverage, which is the right
        // question for *whether*.
        best.agreement = coverage(a, b, best.dx, best.dy);
        return best;
    }

    // A big one is coarse to fine: a whole-sheet sweep at a big step, then a
    // small search around the best of those. Trying every offset at full
    // resolution on a 3000-pixel sheet is millions of comparisons for an answer
    // that is nearly always within a few pixels of nothing.
    let mut best = Alignment {
        dx: 0,
        dy: 0,
        agreement: agreement(a, b, 0, 0),
    };
    let mut step = (reach / 2).max(1);
    let (mut cx, mut cy) = (0i32, 0i32);
    let mut rescored = false;
    loop {
        for dy in -2..=2 {
            for dx in -2..=2 {
                let (x, y) = (cx + dx * step, cy + dy * step);
                if x.abs() > reach || y.abs() > reach {
                    continue;
                }
                let how = if step == 1 {
                    agreement_every(a, b, x, y, 1)
                } else {
                    agreement(a, b, x, y)
                };
                if how > best.agreement {
                    best = Alignment { dx: x, dy: y, agreement: how };
                }
            }
        }
        cx = best.dx;
        cy = best.dy;
        if step == 1 {
            break;
        }
        step /= 2;
        if step == 1 && !rescored {
            // The coarse pass and the fine pass score differently — one
            // samples, one does not — so the coarse winner has to be rescored
            // before the fine pass can be compared against it.
            best.agreement = agreement_every(a, b, best.dx, best.dy, 1);
            rescored = true;
        }
    }
    best.agreement = coverage(a, b, best.dx, best.dy);
    best
}

/// How much of one sheet's line-work the other accounts for, at this offset.
///
/// The larger of the two directions. A revision that added a detail has all of
/// the old sheet inside the new one; a revision that deleted one has all of the
/// new sheet inside the old. Either way that direction reads near one, and two
/// unrelated sheets read near nothing in both.
fn coverage(a: &Ink, b: &Ink, dx: i32, dy: i32) -> f32 {
    let (mut both, mut in_a, mut in_b) = (0u32, 0u32, 0u32);
    for y in 0..a.height {
        for x in 0..a.width {
            let one = a.at(x, y);
            let other = {
                let bx = x as i32 + dx;
                let by = y as i32 + dy;
                bx >= 0 && by >= 0 && b.at(bx as usize, by as usize)
            };
            if one {
                in_a += 1;
            }
            if other {
                in_b += 1;
            }
            if one && other {
                both += 1;
            }
        }
    }
    if in_a == 0 && in_b == 0 {
        // Two blank sheets are the same sheet. Saying otherwise would cloud an
        // empty page.
        return 1.0;
    }
    let of_a = if in_a == 0 { 0.0 } else { both as f32 / in_a as f32 };
    let of_b = if in_b == 0 { 0.0 } else { both as f32 / in_b as f32 };
    of_a.max(of_b)
}

/// How much of the two sheets' ink coincides when the second is moved by
/// (dx, dy). Jaccard — shared ink over all ink — so a sheet with more on it
/// than another cannot score well just by covering it.
fn agreement(a: &Ink, b: &Ink, dx: i32, dy: i32) -> f32 {
    agreement_every(a, b, dx, dy, 4)
}

/// The same, at a chosen sampling.
///
/// Sampling every fourth pixel is fine for finding roughly where two sheets
/// line up, and useless for telling an offset of one from an offset of two —
/// at that step the two answers are the same number. So the coarse pass
/// samples and the last pass does not.
fn agreement_every(a: &Ink, b: &Ink, dx: i32, dy: i32, step: usize) -> f32 {
    let step = step.max(1);
    let (mut both, mut either) = (0u32, 0u32);
    let mut y = 0;
    while y < a.height {
        let mut x = 0;
        while x < a.width {
            let one = a.at(x, y);
            let other = {
                let bx = x as i32 + dx;
                let by = y as i32 + dy;
                bx >= 0 && by >= 0 && b.at(bx as usize, by as usize)
            };
            if one && other {
                both += 1;
            }
            if one || other {
                either += 1;
            }
            x += step;
        }
        y += step;
    }
    if either == 0 {
        // Two blank sheets are the same sheet. Saying otherwise would cloud an
        // empty page.
        return 1.0;
    }
    both as f32 / either as f32
}

/// One thing that changed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Change {
    /// In pixels of the compared rasters: left, top, right, bottom.
    pub area: [usize; 4],
    /// How many pixels inside it actually differ. A big rectangle with few
    /// differing pixels is a moved line, not a redrawn detail.
    pub pixels: usize,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Ink in the newer issue that is not in the older one.
    Added,
    /// Ink in the older issue that is gone from the newer one.
    Removed,
    /// Both, in the same place — something was redrawn.
    Changed,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Added => "added",
            Kind::Removed => "removed",
            Kind::Changed => "changed",
        }
    }
}

/// What a comparison found.
pub struct Comparison {
    pub alignment: Alignment,
    pub changes: Vec<Change>,
    /// True when the two could not be lined up, in which case `changes` is
    /// empty and nothing is marked.
    pub could_not_align: bool,
}

impl Comparison {
    pub fn says(&self) -> String {
        if self.could_not_align {
            return format!(
                "These two sheets could not be lined up — only {:.0}% of the line-work \
                 matches at the best offset. They are probably not two issues of the same \
                 sheet, or one has been rescaled. Nothing has been marked, because marking \
                 the whole sheet would not tell you anything.",
                self.alignment.agreement * 100.0
            );
        }
        if self.changes.is_empty() {
            return "Nothing changed between these two sheets.".into();
        }
        let added = self.changes.iter().filter(|c| c.kind == Kind::Added).count();
        let removed = self
            .changes
            .iter()
            .filter(|c| c.kind == Kind::Removed)
            .count();
        let changed = self
            .changes
            .iter()
            .filter(|c| c.kind == Kind::Changed)
            .count();
        let mut parts = Vec::new();
        if added > 0 {
            parts.push(format!("{added} added"));
        }
        if removed > 0 {
            parts.push(format!("{removed} removed"));
        }
        if changed > 0 {
            parts.push(format!("{changed} changed"));
        }
        format!(
            "{} difference{}: {}. Measure anything that matters — a comparison shows \
             you where to look, it does not count.",
            self.changes.len(),
            if self.changes.len() == 1 { "" } else { "s" },
            parts.join(", ")
        )
    }
}

/// How close two changed pixels have to be to count as one change, as a
/// fraction of the sheet's width. Generous on purpose: a dimension line and the
/// number above it are one change to a person.
const TOGETHER: f64 = 0.012;

/// A change smaller than this many pixels is noise — a rasteriser rounding an
/// edge differently, or a scan's speckle.
const SMALLEST: usize = 12;

/// Compares two issues of a sheet.
pub fn compare(older: &Ink, newer: &Ink) -> Comparison {
    let alignment = align(older, newer);
    if alignment.agreement < ENOUGH_AGREEMENT {
        return Comparison {
            alignment,
            changes: Vec::new(),
            could_not_align: true,
        };
    }

    let (w, h) = (older.width, older.height);
    // Two maps rather than one: what appeared and what went away are different
    // questions to somebody reading a revision, and a single "different" map
    // cannot tell them apart.
    let mut added = vec![false; w * h];
    let mut removed = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let was = older.at(x, y);
            let now = {
                let nx = x as i32 + alignment.dx;
                let ny = y as i32 + alignment.dy;
                nx >= 0 && ny >= 0 && newer.at(nx as usize, ny as usize)
            };
            if now && !was {
                added[y * w + x] = true;
            } else if was && !now {
                removed[y * w + x] = true;
            }
        }
    }

    let reach = ((w as f64 * TOGETHER) as usize).clamp(3, 48);
    let mut changes = clump(&added, w, h, reach, Kind::Added);
    changes.extend(clump(&removed, w, h, reach, Kind::Removed));

    // Where something was taken out and something else put in the same place,
    // that is one change — a detail redrawn — not two.
    let changes = merge_overlapping(changes);

    Comparison {
        alignment,
        changes,
        could_not_align: false,
    }
}

/// Compares two issues already laid on the same pixels by an overlay fit.
///
/// The older ink has been redrawn on the newer sheet's pixels, so nothing
/// more is lined up here. A line counts as still there when the other issue
/// has ink within `tolerance` pixels, so a fit good to a fraction of a pixel
/// does not fringe every line with "added" and "removed".
#[must_use]
pub fn compare_fitted(older: &Ink, newer: &Ink, agreement: f32, tolerance: usize) -> Comparison { // WHAT: compares two inks already laid on the same pixels. WHY: Compare Documents lines sheets up the overlay's way first.
    let alignment = Alignment { dx: 0, dy: 0, agreement }; // WHAT: already lined up, and how well. WHY: the message reads it.
    if agreement < ENOUGH_AGREEMENT { // WHAT: too little agrees. WHY: nothing is marked rather than everything.
        return Comparison { alignment, changes: Vec::new(), could_not_align: true }; // WHAT: nothing marked. WHY: marking the whole sheet would tell nobody anything.
    }
    let (w, h) = (newer.width, newer.height); // WHAT: the newer sheet's pixels. WHY: both are on them now.
    let near_old = crate::overlay::grow(&older.on, w, h, tolerance); // WHAT: near the older ink. WHY: within tolerance is not a change.
    let near_new = crate::overlay::grow(&newer.on, w, h, tolerance); // WHAT: near the newer ink. WHY: the same.
    let added: Vec<bool> = (0..w * h).map(|i| newer.on[i] && !near_old[i]).collect(); // WHAT: newer ink with no older ink near. WHY: what arrived.
    let removed: Vec<bool> = (0..w * h).map(|i| older.on[i] && !near_new[i]).collect(); // WHAT: older ink with no newer ink near. WHY: what went.
    let reach = ((w as f64 * TOGETHER) as usize).clamp(3, 48); // WHAT: how close is one change. WHY: the same rule as an unfitted comparison.
    let mut changes = clump(&added, w, h, reach, Kind::Added); // WHAT: what arrived, in rectangles. WHY: a person wants regions, not dots.
    changes.extend(clump(&removed, w, h, reach, Kind::Removed)); // WHAT: what went. WHY: the same.
    Comparison { alignment, changes: merge_overlapping(changes), could_not_align: false } // WHAT: what was found. WHY: a redrawn detail is one change.
}

/// Gathers scattered pixels into the rectangles a person would draw round them.
///
/// The sheet is cut into squares `reach` across, and squares that hold a
/// changed pixel and touch each other are one change. Two changed pixels
/// within `reach` of each other are always in the same or touching squares,
/// so nothing that belongs together is split (two changes up to twice the
/// reach apart can be joined, which only makes one cloud a little bigger);
/// and the work grows with the
/// size of the sheet, not with the size of the sheet times the size of the
/// reach, which on a sheet that changed all over took minutes.
fn clump(map: &[bool], w: usize, h: usize, reach: usize, kind: Kind) -> Vec<Change> {
    let reach = reach.max(1); // WHAT: at least one pixel. WHY: a square nought across holds nothing.
    let (cw, ch) = (w.div_ceil(reach), h.div_ceil(reach)); // WHAT: how many squares across and down. WHY: the grid.
    let mut cells = vec![[usize::MAX, usize::MAX, 0, 0, 0]; cw * ch]; // WHAT: per square, its changed pixels' box and count. WHY: one pass over the sheet fills them.
    for y in 0..h { // WHAT: every row. WHY: one pass over the sheet.
        for x in 0..w { // WHAT: every pixel in it. WHY: the same.
            if map[y * w + x] { // WHAT: a changed pixel. WHY: tallied in its square.
                let c = &mut cells[(y / reach) * cw + x / reach]; // WHAT: the square it is in. WHY: tallied there.
                c[0] = c[0].min(x); // WHAT: the box's left. WHY: the change's rectangle.
                c[1] = c[1].min(y); // WHAT: top. WHY: the same.
                c[2] = c[2].max(x); // WHAT: right. WHY: the same.
                c[3] = c[3].max(y); // WHAT: bottom. WHY: the same.
                c[4] += 1; // WHAT: one more pixel. WHY: tiny changes are noise.
            }
        }
    }
    let mut seen = vec![false; cw * ch]; // WHAT: squares already gathered. WHY: each joins one change.
    let mut out: Vec<Change> = Vec::new(); // WHAT: the changes. WHY: handed back.
    for start in 0..cw * ch { // WHAT: every square. WHY: each starts a change if it holds one and is not taken.
        if cells[start][4] == 0 || seen[start] { // WHAT: an empty or already gathered square. WHY: nothing to start.
            continue; // WHAT: empty or done. WHY: nothing to start from.
        }
        // Iterative, because a change can be a whole revision cloud's worth of
        // squares and a recursive walk would run out of stack.
        let mut stack = vec![start]; // WHAT: squares still to visit. WHY: the flood.
        seen[start] = true; // WHAT: visited. WHY: not again.
        let (mut x0, mut y0, mut x1, mut y1, mut pixels) = (usize::MAX, usize::MAX, 0usize, 0usize, 0usize); // WHAT: the change's box and size. WHY: grown as squares join.
        while let Some(i) = stack.pop() { // WHAT: the next square to take in. WHY: the flood, without recursion.
            let c = cells[i]; // WHAT: this square. WHY: its pixels join the change.
            x0 = x0.min(c[0]); // WHAT: grows the box. WHY: the change's rectangle.
            y0 = y0.min(c[1]); // WHAT: the same. WHY: the same.
            x1 = x1.max(c[2]); // WHAT: the same. WHY: the same.
            y1 = y1.max(c[3]); // WHAT: the same. WHY: the same.
            pixels += c[4]; // WHAT: its pixels. WHY: the change's size.
            let (cx, cy) = (i % cw, i / cw); // WHAT: where the square is. WHY: its neighbours.
            for ny in cy.saturating_sub(1)..=(cy + 1).min(ch - 1) { // WHAT: the rows of its neighbours. WHY: touching squares join.
                for nx in cx.saturating_sub(1)..=(cx + 1).min(cw - 1) { // WHAT: the columns of its neighbours. WHY: the same.
                    let j = ny * cw + nx; // WHAT: a neighbour. WHY: touching squares are one change.
                    if cells[j][4] > 0 && !seen[j] { // WHAT: a neighbour with change in it, not yet taken. WHY: part of this change.
                        seen[j] = true; // WHAT: visited. WHY: not again.
                        stack.push(j); // WHAT: to visit. WHY: the flood.
                    }
                }
            }
        }
        if pixels >= SMALLEST { // WHAT: big enough to be real. WHY: specks are noise.
            out.push(Change { area: [x0, y0, x1, y1], pixels, kind }); // WHAT: one change. WHY: big enough to be real.
        }
    }
    out // WHAT: the changes found. WHY: handed back.
}

/// The old way of gathering changed pixels, kept only so the tests can show
/// the new one finds the same changes.
#[cfg(test)]
fn clump_pixel_by_pixel(map: &[bool], w: usize, h: usize, reach: usize, kind: Kind) -> Vec<Change> { // WHAT: the earlier pixel-by-pixel gathering, unchanged inside. WHY: the reference the faster one is checked against.
    let mut seen = vec![false; w * h];
    let mut out: Vec<Change> = Vec::new();

    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !map[i] || seen[i] {
                continue;
            }
            // Flood outward, taking anything within reach. Iterative, because a
            // change can be a whole revision cloud's worth of pixels and a
            // recursive walk would run out of stack on the first real drawing.
            let mut stack = vec![(x, y)];
            seen[i] = true;
            let (mut x0, mut y0, mut x1, mut y1) = (x, y, x, y);
            let mut pixels = 0usize;
            while let Some((cx, cy)) = stack.pop() {
                pixels += 1;
                x0 = x0.min(cx);
                y0 = y0.min(cy);
                x1 = x1.max(cx);
                y1 = y1.max(cy);
                let from_x = cx.saturating_sub(reach);
                let to_x = (cx + reach).min(w - 1);
                let from_y = cy.saturating_sub(reach);
                let to_y = (cy + reach).min(h - 1);
                for ny in from_y..=to_y {
                    for nx in from_x..=to_x {
                        let j = ny * w + nx;
                        if map[j] && !seen[j] {
                            seen[j] = true;
                            stack.push((nx, ny));
                        }
                    }
                }
            }
            if pixels >= SMALLEST {
                out.push(Change {
                    area: [x0, y0, x1, y1],
                    pixels,
                    kind,
                });
            }
        }
    }
    out
}

/// An added region sitting on top of a removed one is one thing redrawn.
fn merge_overlapping(mut changes: Vec<Change>) -> Vec<Change> {
    changes.sort_by_key(|c| (c.area[1], c.area[0]));
    let mut out: Vec<Change> = Vec::new();
    for change in changes {
        let mut merged = false;
        for already in out.iter_mut() {
            if overlaps(already.area, change.area) {
                already.area = [
                    already.area[0].min(change.area[0]),
                    already.area[1].min(change.area[1]),
                    already.area[2].max(change.area[2]),
                    already.area[3].max(change.area[3]),
                ];
                already.pixels += change.pixels;
                if already.kind != change.kind {
                    already.kind = Kind::Changed;
                }
                merged = true;
                break;
            }
        }
        if !merged {
            out.push(change);
        }
    }
    out
}

fn overlaps(a: [usize; 4], b: [usize; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(w: usize, h: usize) -> Ink {
        Ink {
            width: w,
            height: h,
            on: vec![false; w * h],
        }
    }

    fn box_in(ink: &mut Ink, x0: usize, y0: usize, x1: usize, y1: usize) {
        for y in y0..=y1.min(ink.height - 1) {
            for x in x0..=x1.min(ink.width - 1) {
                ink.on[y * ink.width + x] = true;
            }
        }
    }

    #[test]
    fn two_identical_sheets_have_nothing_to_report() {
        let mut a = blank(200, 200);
        box_in(&mut a, 20, 20, 60, 60);
        box_in(&mut a, 120, 120, 160, 160);
        let b = a.clone();

        let found = compare(&a, &b);
        assert!(!found.could_not_align);
        assert!(found.changes.is_empty(), "{:?}", found.changes);
        assert!(found.says().contains("Nothing changed"));
    }

    #[test]
    fn something_drawn_on_the_newer_issue_is_found_and_called_added() {
        let mut older = blank(200, 200);
        box_in(&mut older, 20, 20, 60, 60);
        let mut newer = older.clone();
        box_in(&mut newer, 120, 120, 150, 150);

        let found = compare(&older, &newer);
        assert!(!found.could_not_align);
        assert_eq!(found.changes.len(), 1, "{:?}", found.changes);
        assert_eq!(found.changes[0].kind, Kind::Added);
        // And it is where the new box is.
        let [x0, y0, x1, y1] = found.changes[0].area;
        assert!(x0 >= 115 && y0 >= 115 && x1 <= 155 && y1 <= 155, "{:?}", found.changes[0].area);
    }

    #[test]
    fn something_taken_off_the_newer_issue_is_called_removed() {
        let mut older = blank(200, 200);
        box_in(&mut older, 20, 20, 60, 60);
        box_in(&mut older, 120, 120, 150, 150);
        let mut newer = blank(200, 200);
        box_in(&mut newer, 20, 20, 60, 60);

        let found = compare(&older, &newer);
        assert_eq!(found.changes.len(), 1);
        assert_eq!(found.changes[0].kind, Kind::Removed);
    }

    #[test]
    fn a_detail_redrawn_in_the_same_place_is_one_change_not_two() {
        let mut older = blank(200, 200);
        box_in(&mut older, 100, 100, 140, 140);
        let mut newer = blank(200, 200);
        box_in(&mut newer, 105, 105, 145, 145);

        let found = compare(&older, &newer);
        assert_eq!(found.changes.len(), 1, "{:?}", found.changes);
        assert_eq!(found.changes[0].kind, Kind::Changed);
    }

    #[test]
    fn a_sheet_replotted_a_few_pixels_over_is_not_all_change() {
        // The case that makes a naive comparison useless: the same drawing,
        // shifted. Every pixel differs and nothing has actually changed.
        let mut older = blank(200, 200);
        box_in(&mut older, 20, 20, 80, 80);
        box_in(&mut older, 120, 30, 170, 90);
        let mut newer = blank(200, 200);
        box_in(&mut newer, 23, 22, 83, 82);
        box_in(&mut newer, 123, 32, 173, 92);

        let found = compare(&older, &newer);
        assert!(!found.could_not_align, "{}", found.says());
        assert_eq!(found.alignment.dx, 3, "the shift should be found");
        assert_eq!(found.alignment.dy, 2);
        assert!(
            found.changes.is_empty(),
            "a shifted sheet has not changed: {:?}",
            found.changes
        );
    }

    #[test]
    fn two_different_sheets_are_reported_rather_than_clouded_all_over() {
        let mut one = blank(200, 200);
        box_in(&mut one, 10, 10, 90, 90);
        let mut other = blank(200, 200);
        box_in(&mut other, 110, 110, 190, 190);

        let found = compare(&one, &other);
        assert!(found.could_not_align);
        assert!(found.changes.is_empty(), "nothing is marked");
        assert!(found.says().contains("could not be lined up"));
        assert!(found.says().contains("Nothing has been marked"));
    }

    #[test]
    fn a_comparison_never_offers_a_measurement() {
        let mut older = blank(200, 200);
        box_in(&mut older, 20, 20, 60, 60);
        let mut newer = older.clone();
        box_in(&mut newer, 120, 120, 160, 160);
        let said = compare(&older, &newer).says();
        // It points; it does not count. No feet, no inches, no square footage.
        for number_word in ["sq ft", "square", "feet", "lb", "tons", "\""] {
            assert!(!said.contains(number_word), "{said}");
        }
        assert!(said.contains("does not count"));
    }

    #[test]
    fn speckle_on_a_scan_is_not_a_change() {
        let mut older = blank(200, 200);
        box_in(&mut older, 20, 20, 90, 90);
        let mut newer = older.clone();
        // Three stray pixels, scattered, as a scanner leaves.
        newer.on[10 * 200 + 150] = true;
        newer.on[50 * 200 + 180] = true;
        newer.on[190 * 200 + 20] = true;

        let found = compare(&older, &newer);
        assert!(found.changes.is_empty(), "{:?}", found.changes);
    }

    #[test]
    fn two_blank_sheets_are_the_same_sheet() {
        let a = blank(100, 100);
        let b = blank(100, 100);
        let found = compare(&a, &b);
        assert!(!found.could_not_align);
        assert!(found.changes.is_empty());
    }

    #[test]
    fn gathering_by_squares_finds_what_gathering_by_pixels_did() { // WHAT: the test. WHY: the faster gathering must find what the old one did.
        let mut map = vec![false; 300 * 200]; // WHAT: a changed-pixel map. WHY: gathered both ways.
        for (x0, y0, x1, y1) in [(10, 10, 30, 20), (36, 12, 50, 30), (200, 150, 230, 190), (120, 40, 121, 41)] { // WHAT: four blocks of change. WHY: close ones, a far one and a speck.
            for y in y0..=y1 { // WHAT: each row of a block. WHY: filled in.
                for x in x0..=x1 { // WHAT: each pixel of it. WHY: the same.
                    map[y * 300 + x] = true; // WHAT: a block of change. WHY: some close, some far, one tiny.
                }
            }
        }
        let by_squares = clump(&map, 300, 200, 8, Kind::Added); // WHAT: the new way. WHY: compared.
        let by_pixels = clump_pixel_by_pixel(&map, 300, 200, 8, Kind::Added); // WHAT: the old way. WHY: the reference.
        let mut a: Vec<[usize; 4]> = by_squares.iter().map(|c| c.area).collect(); // WHAT: the new way's rectangles. WHY: sorted to compare.
        let mut b: Vec<[usize; 4]> = by_pixels.iter().map(|c| c.area).collect(); // WHAT: the old way's. WHY: the same.
        a.sort_unstable(); // WHAT: in order. WHY: compared as sets.
        b.sort_unstable(); // WHAT: the same. WHY: the same.
        assert_eq!(a, b, "the same changes, the same rectangles"); // WHAT: identical. WHY: only the speed was to change.
    }

    #[test]
    fn a_sheet_changed_all_over_is_gathered_quickly() { // WHAT: the test. WHY: a sheet changed all over took minutes before.
        let (w, h) = (1700, 1200); // WHAT: the size a sheet is compared at. WHY: the real case.
        let map: Vec<bool> = (0..w * h).map(|i| (i / w + i % w) % 3 == 0).collect(); // WHAT: a third of every pixel changed. WHY: a scan against a vector sheet, at its worst.
        let started = std::time::Instant::now(); // WHAT: the clock. WHY: how long it takes.
        let found = clump(&map, w, h, 20, Kind::Added); // WHAT: gathers it. WHY: the test.
        assert_eq!(found.len(), 1, "one change covering the sheet"); // WHAT: one region. WHY: it is all touching.
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed()); // WHAT: seconds, not minutes, even unoptimised. WHY: the pixel-by-pixel way took minutes here.
    }

    #[test]
    fn a_fitted_comparison_ignores_a_fraction_of_a_pixel() { // WHAT: the test. WHY: a fit good to a fraction of a pixel must not fringe every line.
        let mut older = blank(200, 200); // WHAT: the older issue, already laid on the newer. WHY: what an overlay fit hands over.
        box_in(&mut older, 20, 20, 21, 120); // WHAT: a line two pixels wide. WHY: line-work.
        let mut newer = blank(200, 200); // WHAT: the newer issue. WHY: compared.
        box_in(&mut newer, 21, 20, 22, 120); // WHAT: the same line, a pixel over. WHY: the fit's last fraction.
        box_in(&mut newer, 120, 120, 150, 150); // WHAT: something really new. WHY: must still be found.
        let found = compare_fitted(&older, &newer, 0.95, 1); // WHAT: compared with a pixel of tolerance. WHY: the test.
        assert_eq!(found.changes.len(), 1, "{:?}", found.changes); // WHAT: only the new box. WHY: the line did not move.
        assert_eq!(found.changes[0].kind, Kind::Added); // WHAT: added. WHY: it arrived.
        let refused = compare_fitted(&older, &newer, 0.4, 1); // WHAT: badly lined up. WHY: the other case.
        assert!(refused.could_not_align && refused.changes.is_empty()); // WHAT: nothing marked. WHY: as an unfitted comparison.
    }

    #[test]
    fn nearby_marks_are_one_change_and_far_ones_are_two() {
        let mut older = blank(400, 400);
        box_in(&mut older, 10, 10, 20, 20);
        let mut newer = older.clone();
        // Two marks close together: a dimension and its number.
        box_in(&mut newer, 100, 100, 108, 108);
        box_in(&mut newer, 100, 112, 108, 120);
        // And one on the other side of the sheet.
        box_in(&mut newer, 350, 350, 360, 360);

        let found = compare(&older, &newer);
        assert_eq!(found.changes.len(), 2, "{:?}", found.changes);
    }
}
