//! Laying two issues of a sheet over each other: how one sits on the other,
//! and the colours that come out.
//!
//! Two issues of a sheet are rarely the same picture moved a hair. One is a
//! half-size print, one was plotted on bigger paper, one is cropped tighter,
//! one came back from the field as a crooked grey scan, one was stored turned
//! a quarter. So the older sheet is fitted onto the newer one by a turn, a
//! scale and a shift together, found from the line-work itself, and the
//! overlay is drawn through that fit.
//!
//! Nothing here measures anything. The fit says where to lay one drawing so it
//! reads against the other; it never turns into a length or a weight.

use crate::compare::Ink; // WHAT: a sheet as ink and paper. WHY: the fit is found on line-work, not colours.

/// How the older sheet sits on the newer one.
///
/// A point of the older sheet goes to `scale * turned(point) + (dx, dy)` on
/// the newer one. In whatever units the two sides are in: working pixels while
/// it is found, sheet points once it is handed to the renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
// WHAT: a turn, a scale and a shift. WHY: how one issue sits on another.
pub struct Fit {
    pub scale: f64, // WHAT: how much bigger the older drawing is drawn. WHY: a half-size print is the same drawing at half the size.
    pub turn: f64, // WHAT: how far it is turned, in radians, clockwise on the page. WHY: a sideways sheet and a skewed scan.
    pub dx: f64, // WHAT: how far across it moves. WHY: a re-plot, a crop or a bigger sheet moves it.
    pub dy: f64, // WHAT: how far down it moves. WHY: the same.
}

// WHAT: what a fit does. WHY: used both ways and between units.
impl Fit {
    /// The older sheet laid where it is, as it is.
    // WHAT: the fit that changes nothing. WHY: a start.
    pub const SAME: Fit = Fit {
        scale: 1.0,
        turn: 0.0,
        dx: 0.0,
        dy: 0.0,
    }; // WHAT: no change at all. WHY: the start when nothing better is known.

    /// Where a point of the older sheet lands on the newer one.
    #[must_use]
    // WHAT: an older point onto the newer sheet. WHY: the fit forwards.
    pub fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        let (s, c) = self.turn.sin_cos(); // WHAT: the turn's sine and cosine. WHY: a rotation.
        [
            self.scale * (c * p[0] - s * p[1]) + self.dx,
            self.scale * (s * p[0] + c * p[1]) + self.dy,
        ] // WHAT: turned, scaled, moved. WHY: the fit itself.
    }

    /// Where a point of the newer sheet came from on the older one.
    #[must_use]
    // WHAT: a newer point back onto the older sheet. WHY: the fit backwards.
    pub fn back(&self, p: [f64; 2]) -> [f64; 2] {
        let (x, y) = (p[0] - self.dx, p[1] - self.dy); // WHAT: the shift undone. WHY: undone in the reverse order it was done.
        let (s, c) = self.turn.sin_cos(); // WHAT: the turn's sine and cosine. WHY: the turn undone.
        [(c * x + s * y) / self.scale, (-s * x + c * y) / self.scale] // WHAT: turned back and scaled back. WHY: the inverse of `apply`.
    }

    /// The fit through two points picked on each sheet: the first of each
    /// lands on the first, the second on the second.
    ///
    /// None when either pair is one point twice, because no turn or scale
    /// follows from a single point.
    #[must_use]
    // WHAT: the fit through two pairs of points. WHY: lining up by hand.
    pub fn from_points(older: [[f64; 2]; 2], newer: [[f64; 2]; 2]) -> Option<Fit> {
        let vo = [older[1][0] - older[0][0], older[1][1] - older[0][1]]; // WHAT: the older pair's span. WHY: its length and direction.
        let vn = [newer[1][0] - newer[0][0], newer[1][1] - newer[0][1]]; // WHAT: the newer pair's span. WHY: the same.
        let (lo, ln) = (vo[0].hypot(vo[1]), vn[0].hypot(vn[1])); // WHAT: both lengths. WHY: their ratio is the scale.

        // WHAT: a point picked twice, or nonsense. WHY: no fit follows.
        if lo < 1e-6 || ln < 1e-6 || !lo.is_finite() || !ln.is_finite() {
            return None; // WHAT: one point twice, or nonsense. WHY: no fit follows from it.
        }
        let turn = vn[1].atan2(vn[0]) - vo[1].atan2(vo[0]); // WHAT: the angle between the two spans. WHY: the turn.
        let mut fit = Fit {
            scale: ln / lo,
            turn,
            dx: 0.0,
            dy: 0.0,
        }; // WHAT: the turn and scale, not yet moved. WHY: the shift follows from them.
        let at = fit.apply(older[0]); // WHAT: where the first older point lands unmoved. WHY: the shift brings it onto the first newer point.
        fit.dx = newer[0][0] - at[0]; // WHAT: the shift across. WHY: the first points meet.
        fit.dy = newer[0][1] - at[1]; // WHAT: the shift down. WHY: the same.
        Some(fit) // WHAT: the fit. WHY: handed back.
    }

    /// The same fit between other units: `older_k` and `newer_k` are how many
    /// of the new units there are in one of the old, on each side.
    #[must_use]
    // WHAT: the fit in other units. WHY: pixels and points, coarse and fine.
    pub fn rescaled(&self, older_k: f64, newer_k: f64) -> Fit {
        Fit {
            scale: self.scale * newer_k / older_k,
            turn: self.turn,
            dx: self.dx * newer_k,
            dy: self.dy * newer_k,
        } // WHAT: the scale between the new units and the shift in them. WHY: pixels to points, or one level of detail to another.
    }

    /// The turn in whole degrees, nought to 359, for saying in words.
    #[must_use]
    // WHAT: the turn in degrees. WHY: said to people.
    pub fn degrees(&self) -> f64 {
        self.turn.to_degrees().rem_euclid(360.0) // WHAT: the turn, always positive. WHY: "turned 90°" reads better than "turned -270°".
    }
}

/// Ink at one level of detail, with what the fit search needs ready.
// WHAT: one level of detail of a sheet's ink. WHY: searched coarse to fine.
struct Level {
    width: usize,          // WHAT: pixels across. WHY: bounds.
    height: usize,         // WHAT: pixels down. WHY: bounds.
    on: Vec<bool>, // WHAT: the ink itself. WHY: exact hits break the tie between fits a pixel apart.
    near: Vec<bool>, // WHAT: ink grown by one pixel. WHY: a line a fraction of a pixel off still counts as there.
    points: Vec<[f64; 2]>, // WHAT: a spread sample of the ink's pixels. WHY: scoring visits the ink, not the paper.
}

// WHAT: building and reading a level. WHY: the search's own data.
impl Level {
    // WHAT: a level from ink. WHY: grown and sampled once.
    fn new(ink: &Ink) -> Level {
        let near = grow(&ink.on, ink.width, ink.height, 1); // WHAT: the ink, a pixel fatter. WHY: tolerance for the last fraction of a pixel.
        let all = ink.count(); // WHAT: how much ink there is. WHY: thinned to a fixed budget.
        let every = (all / MOST_POINTS).max(1); // WHAT: keep one ink pixel in this many. WHY: scoring a dense sheet costs no more than a light one.
        let mut points = Vec::with_capacity(all / every + 1); // WHAT: the sample. WHY: filled below.
        let mut seen = 0usize; // WHAT: ink pixels passed so far. WHY: every nth is kept.

        // WHAT: every row. WHY: ink is sampled.
        for y in 0..ink.height {
            // WHAT: every pixel. WHY: the same.
            for x in 0..ink.width {
                // WHAT: an ink pixel. WHY: may be sampled.
                if ink.on[y * ink.width + x] {
                    // WHAT: every nth ink pixel. WHY: a fixed budget.
                    if seen % every == 0 {
                        points.push([x as f64 + 0.5, y as f64 + 0.5]); // WHAT: the pixel's middle. WHY: mapped through a fit.
                    }
                    seen += 1; // WHAT: counts it. WHY: the sampling.
                }
            }
        }
        Level {
            width: ink.width,
            height: ink.height,
            on: ink.on.clone(),
            near,
            points,
        } // WHAT: the level. WHY: ready to score against.
    }

    /// Whether there is ink near a point, and whether exactly on it.
    // WHAT: ink near and on a point. WHY: the score's two tests.
    fn at(&self, p: [f64; 2]) -> (bool, bool) {
        // WHAT: left of or above the sheet. WHY: no ink.
        if p[0] < 0.0 || p[1] < 0.0 {
            return (false, false); // WHAT: off the sheet. WHY: no ink there.
        }
        let (x, y) = (p[0] as usize, p[1] as usize); // WHAT: the pixel it falls in. WHY: looked up.

        // WHAT: right of or below it. WHY: no ink.
        if x >= self.width || y >= self.height {
            return (false, false); // WHAT: off the sheet. WHY: the same.
        }
        let i = y * self.width + x; // WHAT: its index. WHY: both maps are looked up.
        (self.near[i], self.on[i]) // WHAT: near ink, and on ink. WHY: near decides agreement; on breaks ties.
    }
}

/// The most ink pixels any one score visits.
const MOST_POINTS: usize = 6000; // WHAT: a fixed budget per side. WHY: enough to tell a fit from a near miss, small enough to try hundreds.

/// The largest side of the coarsest level, in pixels.
const COARSEST: usize = 240; // WHAT: where the wide search happens. WHY: every shift there is cheap, and a sheet's layout still shows.

/// How far, as a share of the sheet, a sheet may have moved from where a start puts it.
const MOST_SHIFT: f64 = 0.04; // WHAT: four percent of the sheet. WHY: re-plots and title blocks move a sheet by less; each start covers its own case beyond that.

/// Grows ink by `r` pixels in every direction.
///
/// Two passes, across then down, each a running count over a window, so the
/// cost does not grow with `r`.
#[must_use]
// WHAT: ink grown by r pixels. WHY: a line near enough counts.
pub fn grow(on: &[bool], width: usize, height: usize, r: usize) -> Vec<bool> {
    // WHAT: nothing to grow. WHY: the ink as it is.
    if r == 0 || width == 0 || height == 0 {
        return on.to_vec(); // WHAT: nothing to grow. WHY: the same ink.
    }
    let mut across = vec![false; on.len()]; // WHAT: grown sideways. WHY: the first pass.

    // WHAT: every row. WHY: the sideways pass.
    for y in 0..height {
        let row = &on[y * width..(y + 1) * width]; // WHAT: one row. WHY: the window slides along it.
        let mut inside = 0usize; // WHAT: ink pixels in the window. WHY: the running count.

        // WHAT: across the row and r past it. WHY: the window slides off the end.
        for x in 0..(width + r) {
            // WHAT: ink entering. WHY: counted.
            if x < width && row[x] {
                inside += 1; // WHAT: the pixel entering on the right. WHY: in the window now.
            }
            // WHAT: ink leaving. WHY: uncounted.
            if x >= 2 * r + 1 && row[x - 2 * r - 1] {
                inside -= 1; // WHAT: the pixel leaving on the left. WHY: out of the window now.
            }
            // WHAT: ink in reach. WHY: this pixel is grown.
            if x >= r && inside > 0 {
                across[y * width + x - r] = true; // WHAT: ink within r of this pixel. WHY: grown.
            }
        }
    }
    let mut out = vec![false; on.len()]; // WHAT: grown both ways. WHY: the second pass.

    // WHAT: every column. WHY: the downward pass.
    for x in 0..width {
        let mut inside = 0usize; // WHAT: the running count, down this column. WHY: the same window, upright.

        // WHAT: down the column and r past it. WHY: the same.
        for y in 0..(height + r) {
            // WHAT: ink entering. WHY: counted.
            if y < height && across[y * width + x] {
                inside += 1; // WHAT: entering at the bottom. WHY: in the window.
            }
            // WHAT: ink leaving. WHY: uncounted.
            if y >= 2 * r + 1 && across[(y - 2 * r - 1) * width + x] {
                inside -= 1; // WHAT: leaving at the top. WHY: out of it.
            }
            // WHAT: ink in reach. WHY: grown.
            if y >= r && inside > 0 {
                out[(y - r) * width + x] = true; // WHAT: ink within r. WHY: grown.
            }
        }
    }
    out // WHAT: the grown ink. WHY: handed back.
}

/// Half the size, a pixel inked where any of the four it covers was.
///
/// Inked where any was, not where most were, so a hairline survives being
/// made smaller instead of disappearing.
#[must_use]
// WHAT: ink at half size. WHY: the next coarser level.
pub fn halved(ink: &Ink) -> Ink {
    let (w, h) = (ink.width.div_ceil(2).max(1), ink.height.div_ceil(2).max(1)); // WHAT: the new size. WHY: half, rounded up.
    let mut on = vec![false; w * h]; // WHAT: the smaller ink. WHY: filled below.

    // WHAT: every row. WHY: each pixel lands in a half-size one.
    for y in 0..ink.height {
        // WHAT: every pixel. WHY: the same.
        for x in 0..ink.width {
            // WHAT: an ink pixel. WHY: inks the one it lands in.
            if ink.on[y * ink.width + x] {
                on[(y / 2) * w + x / 2] = true; // WHAT: inks the pixel it falls in. WHY: any ink keeps it.
            }
        }
    }
    Ink {
        width: w,
        height: h,
        on,
    } // WHAT: the smaller ink. WHY: one level up.
}

/// How well a fit lays the older ink on the newer: nought to one for ink
/// landing near ink, and up to a quarter more for landing exactly on it.
///
/// Both ways at once: how many of the older's ink pixels land on ink of the
/// newer, and how many of the newer's come from ink of the older. Either alone
/// can be fooled — a sheet covered in ink covers anything — and both together
/// cannot.
// WHAT: how good a fit is. WHY: the search compares them.
fn score(older: &Level, newer: &Level, fit: &Fit) -> f64 {
    let (one, other) = (older.points.len(), newer.points.len()); // WHAT: how much ink each side has. WHY: the shares below.

    // WHAT: no ink either side. WHY: blank agrees with blank.
    if one == 0 && other == 0 {
        return 1.0; // WHAT: two blank sheets agree. WHY: saying otherwise would colour an empty page.
    }
    let [landed, came, exact_landed, exact_came] = hits(older, newer, fit); // WHAT: ink that meets ink, each way, near and exactly. WHY: the agreement.
    (landed + came) as f64 / (one + other) as f64
        + 0.25 * (exact_landed + exact_came) as f64 / (one + other) as f64 // WHAT: the share that agrees, plus a little for landing exactly. WHY: near alone is a pixel-wide plateau; exact hits find the middle of it.
}

/// How many older ink pixels land on newer ink, and how many newer ink pixels
/// come from older ink, through a fit.
///
/// The fit is turned into its four numbers once here, rather than a sine and
/// a cosine worked out again for each of thousands of points.
// WHAT: counts ink meeting ink through a fit. WHY: the score and the coverage.
fn hits(older: &Level, newer: &Level, fit: &Fit) -> [usize; 4] {
    let (s, c) = fit.turn.sin_cos(); // WHAT: the turn, once. WHY: used for every point.
    let (a, b) = (fit.scale * c, fit.scale * s); // WHAT: the turn and scale together. WHY: one multiply each way.
    let inv = 1.0 / (fit.scale * fit.scale).max(1e-18); // WHAT: undoes the scale twice over. WHY: the inverse of a turn-and-scale.
    let mut out = [0usize; 4]; // WHAT: landed near, came near, landed exactly, came exactly. WHY: counted below.

    // WHAT: each sampled older ink pixel. WHY: does it land on newer ink.
    for p in &older.points {
        let (near, on) = newer.at([a * p[0] - b * p[1] + fit.dx, b * p[0] + a * p[1] + fit.dy]); // WHAT: where older ink lands on the newer. WHY: one direction.
        out[0] += usize::from(near); // WHAT: counts a near landing. WHY: agreement.
        out[2] += usize::from(on); // WHAT: counts an exact one. WHY: the tie-break.
    }
    // WHAT: each sampled newer ink pixel. WHY: did it come from older ink.
    for p in &newer.points {
        let (x, y) = (p[0] - fit.dx, p[1] - fit.dy); // WHAT: the shift undone. WHY: the inverse.
        let (near, on) = older.at([(a * x + b * y) * inv, (-b * x + a * y) * inv]); // WHAT: where newer ink came from on the older. WHY: the other direction.
        out[1] += usize::from(near); // WHAT: counts it. WHY: agreement.
        out[3] += usize::from(on); // WHAT: counts an exact one. WHY: the tie-break.
    }
    out // WHAT: the four counts. WHY: handed back.
}

/// How much of one sheet's ink the other accounts for, at this fit.
///
/// The larger of the two directions, for the same reason the comparison uses
/// it: a revision that added a detail has all of the old sheet inside the new
/// one, and should not be called a different sheet for it.
// WHAT: how much of one sheet the other accounts for. WHY: lined up or not.
fn coverage(older: &Level, newer: &Level, fit: &Fit) -> f64 {
    let (one, other) = (older.points.len(), newer.points.len()); // WHAT: how much ink each side has. WHY: the shares.

    // WHAT: no ink either side. WHY: blank is the same sheet.
    if one == 0 && other == 0 {
        return 1.0; // WHAT: two blank sheets are the same sheet. WHY: as above.
    }
    let [landed, came, ..] = hits(older, newer, fit); // WHAT: ink that meets ink, each way. WHY: both directions.
    let of_older = if one == 0 {
        0.0
    } else {
        landed as f64 / one as f64
    }; // WHAT: the older sheet's share accounted for. WHY: compared below.
    let of_newer = if other == 0 {
        0.0
    } else {
        came as f64 / other as f64
    }; // WHAT: the newer sheet's share. WHY: the same.
    of_older.max(of_newer) // WHAT: the better direction. WHY: added or removed, a revision reads near one.
}

/// What fitting found.
#[derive(Clone, Copy, Debug, PartialEq)]
// WHAT: a fit and how well it agrees. WHY: what a search hands back.
pub struct Found {
    pub fit: Fit, // WHAT: the best fit, in the working pixels it was found in. WHY: the overlay is drawn through it.
    pub agreement: f32, // WHAT: coverage at that fit, nought to one. WHY: under ENOUGH_AGREEMENT it did not line up.
}

/// Finds the fit that lays `older` best on `newer`, starting from each of `starts`.
///
/// Coarse to fine. At the coarsest level every shift within reach of each
/// start is tried, which is what finds a sheet that moved; then each level
/// down, the best few are nudged a pixel at a time in shift, turn and scale,
/// which is what straightens a skewed scan and settles a half-size print.
#[must_use]
// WHAT: the best fit of one sheet on another. WHY: lining up by the drawing.
pub fn find(older: &Ink, newer: &Ink, starts: &[Fit]) -> Found {
    let (olders, newers) = (pyramid(older), pyramid(newer)); // WHAT: both sheets at every level of detail. WHY: searched coarse first.
    let top = olders.len().min(newers.len()) - 1; // WHAT: the coarsest level both have. WHY: fits there are compared like for like.
    let shrink = f64::from(1u32 << top); // WHAT: how much smaller that level is. WHY: fits move between levels by it.
    let (o, n) = (&olders[top], &newers[top]); // WHAT: the coarsest level of each. WHY: the wide search.
    let reach = ((n.width.max(n.height) as f64 * MOST_SHIFT).ceil() as i32).max(2); // WHAT: how far to look, in coarse pixels. WHY: four percent of the sheet.

    let mut tried: Vec<(f64, Fit)> = Vec::new(); // WHAT: the best of each start, scored. WHY: the best few go down the levels.

    // WHAT: each start. WHY: each covers a case: same size, half size, turned.
    for start in starts {
        let coarse = Fit {
            dx: start.dx / shrink,
            dy: start.dy / shrink,
            ..*start
        }; // WHAT: the start at the coarsest level, its scale and turn kept. WHY: both sides shrink alike, so only the shift is in different pixels.
        let mut best = (score(o, n, &coarse), coarse); // WHAT: the start itself. WHY: beaten only by something better.

        // WHAT: every shift down in reach. WHY: the wide search.
        for dy in -reach..=reach {
            // WHAT: every shift across in reach. WHY: the same.
            for dx in -reach..=reach {
                let fit = Fit {
                    dx: coarse.dx + f64::from(dx),
                    dy: coarse.dy + f64::from(dy),
                    ..coarse
                }; // WHAT: the start, shifted. WHY: every shift in reach.
                let how = score(o, n, &fit); // WHAT: how well that shift agrees. WHY: the search.

                // WHAT: better than the best. WHY: kept.
                if how > best.0 + 1e-9 {
                    best = (how, fit); // WHAT: the best so far. WHY: kept.
                }
            }
        }
        best.1 = nudge(o, n, best.1, 1.0, true); // WHAT: settles turn and scale at this level too. WHY: a skew wide enough to matter shows here first.
        tried.push((score(o, n, &best.1), best.1)); // WHAT: this start's best. WHY: compared with the others.
    }
    // WHAT: no starts were given. WHY: the sheet as it is still gets tried.
    if tried.is_empty() {
        tried.push((score(o, n, &Fit::SAME), Fit::SAME)); // WHAT: no starts given. WHY: the sheet as it is is still a start.
    }
    tried.sort_by(|a, b| b.0.total_cmp(&a.0)); // WHAT: best first, and the earlier start first among equals. WHY: the sort is stable, so the order starts were given in breaks ties.
    tried.truncate(3); // WHAT: the best three. WHY: refining them all would be slow, and the rest are far behind.

    let mut finest: Vec<(f64, Fit)> = Vec::new(); // WHAT: each refined to the finest level. WHY: the winner is chosen there.

    // WHAT: each of the best few. WHY: refined down the levels.
    for (_, fit) in tried {
        let mut fit = fit; // WHAT: the fit being refined. WHY: carried down.

        // WHAT: each finer level. WHY: coarse to fine.
        for level in (0..top).rev() {
            fit = Fit {
                dx: fit.dx * 2.0,
                dy: fit.dy * 2.0,
                ..fit
            }; // WHAT: the shift in this level's pixels. WHY: each level is twice the last.
            fit = nudge(&olders[level], &newers[level], fit, 1.0, false); // WHAT: nudged at this level. WHY: finer detail settles it finer.
        }
        finest.push((score(&olders[0], &newers[0], &fit), fit)); // WHAT: scored at full detail. WHY: the winner is chosen there.
    }
    let mut best = finest[0]; // WHAT: the first, so far best. WHY: compared below.

    // WHAT: the rest of the refined fits. WHY: the best is kept.
    for one in &finest[1..] {
        // WHAT: better than the best. WHY: kept.
        if one.0 > best.0 + 1e-9 {
            best = *one; // WHAT: a better one. WHY: kept.
        }
    }
    Found {
        fit: best.1,
        agreement: coverage(&olders[0], &newers[0], &best.1) as f32,
    } // WHAT: the fit and how well it agrees. WHY: the overlay and the message use both.
}

/// Settles a fit somebody chose — two points picked on each sheet — at the
/// finest two levels, a pixel at a time.
///
/// Never a wide search: the points said roughly where, and the job here is
/// only to take out the half-pixel a click cannot place.
#[must_use]
// WHAT: settles a hand-picked fit. WHY: a click is never exact.
pub fn settle(older: &Ink, newer: &Ink, fit: Fit) -> Found {
    let (olders, newers) = (pyramid(older), pyramid(newer)); // WHAT: both sheets at every level. WHY: settled at the finest two.
    let mut fit = fit; // WHAT: the fit being settled. WHY: carried down.

    // WHAT: there is a coarser level. WHY: settled there first.
    if olders.len() > 1 && newers.len() > 1 {
        let coarse = Fit {
            dx: fit.dx / 2.0,
            dy: fit.dy / 2.0,
            ..fit
        }; // WHAT: the fit one level up. WHY: a click's error is a few pixels, which is one there.
        let coarse = nudge(&olders[1], &newers[1], coarse, 1.0, false); // WHAT: settled there. WHY: the wider basin.
        fit = Fit {
            dx: coarse.dx * 2.0,
            dy: coarse.dy * 2.0,
            ..coarse
        }; // WHAT: back at full detail. WHY: finished there.
    }
    let fit = nudge(&olders[0], &newers[0], fit, 1.0, false); // WHAT: settled at full detail. WHY: the last fraction of a pixel.
    Found {
        fit,
        agreement: coverage(&olders[0], &newers[0], &fit) as f32,
    } // WHAT: the fit and how well it agrees. WHY: the message says it.
}

/// How well a fit lays one sheet on the other, nought to one, without moving it.
#[must_use]
// WHAT: agreement of an unmoved fit. WHY: shown as they are, still say how well.
pub fn agreement(older: &Ink, newer: &Ink, fit: &Fit) -> f32 {
    coverage(&Level::new(older), &Level::new(newer), fit) as f32 // WHAT: coverage at that fit. WHY: "shown as they are" still says how well they match.
}

/// The older ink redrawn on the newer sheet's pixels through a fit.
///
/// Each newer pixel looks back to where it came from on the older sheet,
/// so every pixel is filled once and nothing is left as a gap.
#[must_use]
// WHAT: the older ink on the newer's pixels. WHY: compared pixel for pixel.
pub fn warp(older: &Ink, fit: &Fit, width: usize, height: usize) -> Ink {
    // WHAT: drawn much smaller. WHY: halve first so hairlines survive.
    if fit.scale < 0.75 && older.width > 1 && older.height > 1 {
        return warp(
            &halved(older),
            &Fit {
                scale: fit.scale * 2.0,
                ..*fit
            },
            width,
            height,
        ); // WHAT: shrinks the older ink first, keeping every hairline. WHY: drawn smaller pixel by pixel, a thin line would fall between the pixels looked at.
    }
    let mut on = vec![false; width * height]; // WHAT: the older ink, in the newer's frame. WHY: filled below.

    // WHAT: every row of the newer. WHY: each pixel looks back.
    for y in 0..height {
        // WHAT: every pixel. WHY: the same.
        for x in 0..width {
            let p = fit.back([x as f64 + 0.5, y as f64 + 0.5]); // WHAT: where this pixel came from. WHY: looked up on the older sheet.

            // WHAT: comes from on the older sheet. WHY: nothing above or left of it.
            if p[0] >= 0.0 && p[1] >= 0.0 {
                on[y * width + x] = older.at(p[0] as usize, p[1] as usize); // WHAT: ink there or not. WHY: the older drawing, moved.
            }
        }
    }
    Ink { width, height, on } // WHAT: the moved ink. WHY: compared pixel for pixel with the newer.
}

/// How the older sheet is to be laid on the newer one.
#[derive(Clone, Debug, PartialEq)]
// WHAT: how to line up. WHY: chosen in the window, carried to the engine.
pub enum Lining {
    /// Found from the line-work: shift, quarter turns, skew, scale.
    Auto,
    /// Through two points picked on each sheet, in each sheet's own points
    /// (top left, down the page): the first of each meet, and the second.
    Points {
        older: [[f64; 2]; 2],
        newer: [[f64; 2]; 2],
    },
    /// Where the two files' own coordinates put them, uncorrected.
    AsTheyAre,
}

/// Which older sheet each newer sheet is laid over, by sheet number.
///
/// By number where both issues have one, so a sheet put in the middle of a
/// set does not push every sheet after it onto the wrong partner. A sheet
/// with no number is paired by position, but only with an older sheet that
/// has no number either. When neither issue has numbers at all, it is by
/// position throughout, which is all there is to go on.
#[must_use]
// WHAT: pairs sheets of two issues. WHY: by number, so an added sheet shifts nothing.
pub fn pair_sheets(newer: &[String], older: &[String]) -> Vec<Option<usize>> {
    let numbered = |list: &[String]| list.iter().any(|n| !n.trim().is_empty()); // WHAT: does a list have any sheet numbers. WHY: none at all means by position.

    // WHAT: either issue has no numbers. WHY: by place is all there is.
    if !numbered(newer) || !numbered(older) {
        return (0..newer.len())
            .map(|i| (i < older.len()).then_some(i))
            .collect(); // WHAT: by position. WHY: there is nothing else to match on.
    }
    let key = |n: &str| n.split_whitespace().collect::<String>().to_uppercase(); // WHAT: a number without spaces or case. WHY: "s-101" and "S-101 " are the same sheet.
    let mut taken = vec![false; older.len()]; // WHAT: older sheets already paired. WHY: each pairs once.
    let mut out = Vec::with_capacity(newer.len()); // WHAT: the pairing. WHY: one per newer sheet.

    // WHAT: each newer sheet. WHY: each gets a partner or none.
    for (i, number) in newer.iter().enumerate() {
        let wanted = key(number); // WHAT: this sheet's number. WHY: looked for in the older issue.
        let found = if wanted.is_empty() {
            // WHAT: same place, both unnumbered. WHY: the only safe guess for a sheet with no number.
            (i < older.len() && key(&older[i]).is_empty() && !taken[i]).then_some(i)
        } else {
            (0..older.len()).find(|j| !taken[*j] && key(&older[*j]) == wanted) // WHAT: the older sheet of the same number. WHY: matched by number.
        };
        // WHAT: a partner was found. WHY: taken.
        if let Some(j) = found {
            taken[j] = true; // WHAT: used. WHY: each older sheet pairs once.
        }
        out.push(found); // WHAT: this sheet's partner, or none. WHY: none is shown as all new.
    }
    out // WHAT: the pairing. WHY: handed back.
}

/// Settles a fit at one level: tries a step either way in shift, turn and
/// scale, keeps whatever helps, and halves the steps when nothing does.
///
/// `wide` starts the turn and scale steps big — half a degree, one percent —
/// which is what a crooked scan needs at the coarsest level.
// WHAT: settles a fit at one level. WHY: small steps in shift, turn and scale.
fn nudge(older: &Level, newer: &Level, fit: Fit, step: f64, wide: bool) -> Fit {
    let span = (newer.width.max(newer.height) as f64).max(1.0); // WHAT: the sheet's size in pixels. WHY: a turn of 1/span moves its far edge a pixel.
    let mut steps = if wide {
        [step, step, 0.5_f64.to_radians(), 0.01]
    } else {
        [step, step, 1.0 / span, 1.0 / span]
    }; // WHAT: how far each of shift, turn and scale is tried. WHY: wide at the top, a pixel's worth below.
    let finest = [0.25, 0.25, 0.25 / span, 0.25 / span]; // WHAT: the smallest steps. WHY: a quarter pixel is past what the ink can show.
    let mut best = (score(older, newer, &fit), fit); // WHAT: where it starts. WHY: only improvements are kept.
    let mut rounds = 0; // WHAT: how many rounds so far. WHY: a fixed limit.

    // WHAT: up to sixty rounds. WHY: always ends.
    while rounds < 60 {
        rounds += 1; // WHAT: counts the round. WHY: the limit.
        let mut better = false; // WHAT: did anything help this round. WHY: if not, the steps halve.

        // WHAT: shift across, shift down, turn, scale. WHY: each tried in turn.
        for which in 0..4 {
            // WHAT: a step either way. WHY: whichever helps.
            for sign in [-1.0, 1.0] {
                let mut fit = best.1; // WHAT: the best so far. WHY: one thing changed from it.

                // WHAT: which one to change. WHY: one at a time.
                match which {
                    0 => fit.dx += sign * steps[0], // WHAT: a step across. WHY: shift.
                    1 => fit.dy += sign * steps[1], // WHAT: a step down. WHY: shift.
                    2 => fit = turned_about_middle(fit, sign * steps[2], newer), // WHAT: a little turn. WHY: skew.
                    _ => fit = scaled_about_middle(fit, 1.0 + sign * steps[3], newer), // WHAT: a little bigger or smaller. WHY: a print not quite to size.
                }
                let how = score(older, newer, &fit); // WHAT: how well it agrees now. WHY: kept if better.

                // WHAT: it helped. WHY: kept.
                if how > best.0 + 1e-9 {
                    best = (how, fit); // WHAT: better. WHY: kept.
                    better = true; // WHAT: something helped. WHY: the steps stay.
                }
            }
        }
        // WHAT: nothing helped. WHY: smaller steps.
        if !better {
            let mut all_fine = true; // WHAT: are the steps as small as they go. WHY: then it is settled.

            // WHAT: each step and its smallest. WHY: halved down to it.
            for (s, f) in steps.iter_mut().zip(finest) {
                // WHAT: not at its smallest. WHY: halved.
                if *s > f {
                    *s = (*s * 0.5).max(f); // WHAT: half the step. WHY: closer in.
                    all_fine = false; // WHAT: not settled yet. WHY: smaller steps to try.
                }
            }
            // WHAT: every step at its smallest. WHY: settled.
            if all_fine {
                break; // WHAT: settled. WHY: nothing smaller to try.
            }
        }
    }
    best.1 // WHAT: the settled fit. WHY: handed back.
}

/// A fit turned a little more, about the middle of the newer sheet.
///
/// About the middle, not the corner, so a turn does not also throw the whole
/// sheet sideways and undo the shift already found.
// WHAT: a fit turned a little about the middle. WHY: the shift found is kept.
fn turned_about_middle(fit: Fit, by: f64, newer: &Level) -> Fit {
    let m = [newer.width as f64 / 2.0, newer.height as f64 / 2.0]; // WHAT: the newer sheet's middle. WHY: the turn is about it.
    let (s, c) = by.sin_cos(); // WHAT: the extra turn. WHY: applied to the shift too.
    let (x, y) = (fit.dx - m[0], fit.dy - m[1]); // WHAT: the shift, from the middle. WHY: turned about it.
    Fit {
        turn: fit.turn + by,
        dx: m[0] + c * x - s * y,
        dy: m[1] + s * x + c * y,
        ..fit
    } // WHAT: the turned fit. WHY: the middle stays put.
}

/// A fit made a little bigger or smaller, about the middle of the newer sheet.
// WHAT: a fit scaled a little about the middle. WHY: the same.
fn scaled_about_middle(fit: Fit, by: f64, newer: &Level) -> Fit {
    let m = [newer.width as f64 / 2.0, newer.height as f64 / 2.0]; // WHAT: the middle. WHY: the scale is about it.
    Fit {
        scale: fit.scale * by,
        dx: m[0] + (fit.dx - m[0]) * by,
        dy: m[1] + (fit.dy - m[1]) * by,
        ..fit
    } // WHAT: the scaled fit. WHY: the middle stays put.
}

/// Every level of detail of a sheet, finest first, down to about `COARSEST` across.
// WHAT: a sheet at every level of detail. WHY: coarse to fine.
fn pyramid(ink: &Ink) -> Vec<Level> {
    let mut levels = vec![Level::new(ink)]; // WHAT: full detail. WHY: the last refinement happens there.
    let mut at = ink.clone(); // WHAT: the current level's ink. WHY: halved each time.

    // WHAT: still bigger than the coarsest. WHY: halve again.
    while at.width.max(at.height) > COARSEST && levels.len() < 6 {
        at = halved(&at); // WHAT: half the size. WHY: the next level.
        levels.push(Level::new(&at)); // WHAT: kept. WHY: searched.
    }
    levels // WHAT: the levels. WHY: handed back.
}

/// The usual starting places for one sheet on another, in working pixels.
///
/// For each quarter turn: the older sheet at the same size in points as the
/// newer (bigger paper, a crop), and fitted to the newer's paper (a half-size
/// print); each from the top left corner and from the middle. The fit the
/// two files' own coordinates give, when known, goes first, because a
/// re-issued plot nearly always has its drawing where it was.
#[must_use]
// WHAT: the places to start looking. WHY: each a common way two issues differ.
pub fn starts(older: [f64; 2], newer: [f64; 2], same_points: f64, own: Option<Fit>) -> Vec<Fit> {
    let mut out: Vec<Fit> = own.into_iter().collect(); // WHAT: the files' own fit first. WHY: the commonest truth.

    // WHAT: each quarter turn. WHY: sheets stored or scanned turned.
    for quarter in 0..4 {
        let turn = f64::from(quarter) * std::f64::consts::FRAC_PI_2; // WHAT: this quarter turn. WHY: a sheet stored or scanned sideways.
        let (w, h) = if quarter % 2 == 1 {
            (older[1], older[0])
        } else {
            (older[0], older[1])
        }; // WHAT: the older sheet's size once turned. WHY: a quarter turn swaps across and down.
        let fitted = (newer[0] / w.max(1e-9)).min(newer[1] / h.max(1e-9)); // WHAT: the scale that fits it on the newer paper. WHY: a half-size print.

        // WHAT: same size in points, and fitted to the paper. WHY: bigger paper, or a half-size print.
        for scale in [same_points, fitted] {
            let fit = Fit {
                scale,
                turn,
                dx: 0.0,
                dy: 0.0,
            }; // WHAT: turned and scaled, not yet moved. WHY: the shift follows.
            let corners = [
                [0.0, 0.0],
                [older[0], 0.0],
                [0.0, older[1]],
                [older[0], older[1]],
            ]
            .map(|p| fit.apply(p)); // WHAT: where its corners go. WHY: its box once turned.
            let left = corners.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min); // WHAT: its left edge. WHY: the top left anchor.
            let top = corners.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min); // WHAT: its top edge. WHY: the same.
            let (cw, ch) = (w * scale, h * scale); // WHAT: its size once scaled. WHY: the middle anchor.

            // WHAT: from the top left, and from the middle. WHY: where extra paper went.
            for (dx, dy) in [
                (-left, -top),
                (-left + (newer[0] - cw) / 2.0, -top + (newer[1] - ch) / 2.0),
            ] {
                let candidate = Fit { dx, dy, ..fit }; // WHAT: anchored. WHY: one starting place.
                let close = out.iter().any(|f| {
                    (f.scale / candidate.scale - 1.0).abs() < 0.005
                        && (f.turn - candidate.turn).abs() < 1e-3
                        && (f.dx - candidate.dx).abs() < 2.0
                        && (f.dy - candidate.dy).abs() < 2.0
                }); // WHAT: already have one like it. WHY: same-size sheets give the same start four ways.

                // WHAT: not one already listed. WHY: no start tried twice.
                if !close {
                    out.push(candidate); // WHAT: a new start. WHY: tried.
                }
            }
        }
    }
    out // WHAT: the starts. WHY: handed back.
}

/// What a sheet's own paper looks like: the brightness most of it has.
///
/// Not white on a scan. A grey page read against white would be ink from edge
/// to edge, so ink is judged against the sheet's own paper.
#[must_use]
// WHAT: the sheet's paper brightness. WHY: ink is judged against it.
pub fn paper_of(luma: &[u8]) -> u8 {
    let mut counts = [0usize; 256]; // WHAT: how many pixels at each brightness. WHY: the histogram.

    // WHAT: every pixel. WHY: the histogram.
    for v in luma {
        counts[usize::from(*v)] += 1; // WHAT: counts it. WHY: the same.
    }
    let wanted = luma.len() / 10; // WHAT: the brightest tenth. WHY: paper is most of a sheet, and the brightest of it.
    let mut seen = 0usize; // WHAT: pixels counted from the bright end. WHY: stops at the tenth.

    // WHAT: from white down. WHY: the brightest tenth is paper.
    for level in (0..256).rev() {
        seen += counts[level]; // WHAT: adds this brightness. WHY: walking down from white.

        // WHAT: a tenth counted. WHY: that is the paper.
        if seen > wanted {
            return u8::try_from(level).unwrap_or(255).max(32); // WHAT: the paper's brightness. WHY: never so dark that nothing is ink.
        }
    }
    255 // WHAT: an empty picture. WHY: white paper.
}

/// Ink and paper, judged against the sheet's own paper.
#[must_use]
// WHAT: ink against the sheet's own paper. WHY: a grey scan is not all ink.
pub fn ink_of(luma: &[u8], width: usize, height: usize, darkness: f32) -> Ink {
    let paper = f32::from(paper_of(luma)); // WHAT: the sheet's paper. WHY: ink is darker than it, not darker than white.
    let cut = paper * darkness.clamp(0.05, 0.99); // WHAT: how dark is ink. WHY: the darkness setting, against this paper.
    Ink {
        width,
        height,
        on: luma.iter().map(|v| f32::from(*v) < cut).collect(),
    } // WHAT: each pixel, ink or not. WHY: the sheet as line-work.
}

/// The colour of the older issue: red.
pub const OLDER: [u8; 3] = [0xC0, 0x39, 0x2B]; // WHAT: red. WHY: told from blue by hue, so it reads photocopied and to the colour-blind.
/// The colour of the newer issue: blue.
pub const NEWER: [u8; 3] = [0x1F, 0x6F, 0xB2]; // WHAT: blue. WHY: the same.
/// The colour where both agree: dark grey.
pub const BOTH: [u8; 3] = [0x33, 0x33, 0x33]; // WHAT: dark grey. WHY: what did not move reads as an ordinary drawing.

/// The two issues, already drawn on the same paper, coloured into one picture.
///
/// `older` and `newer` are brightness, one byte a pixel, the same size. A
/// line in one with a line of the other within `tolerance` pixels is both;
/// otherwise it takes its own issue's colour. Soft edges stay soft: each
/// pixel is only as strong as the darker of the two drawings there.
#[must_use]
// WHAT: the two issues coloured into one picture. WHY: the overlay.
pub fn colour(
    older: &[u8],
    newer: &[u8],
    width: usize,
    height: usize,
    darkness: f32,
    tolerance: usize,
) -> Vec<[u8; 3]> {
    let (pa, pn) = (f32::from(paper_of(older)), f32::from(paper_of(newer))); // WHAT: each sheet's paper. WHY: a scan's grey is not ink.
    let ink_a = ink_of(older, width, height, darkness); // WHAT: the older's ink. WHY: what counts as a line.
    let ink_n = ink_of(newer, width, height, darkness); // WHAT: the newer's ink. WHY: the same.
    let near_a = grow(&ink_a.on, width, height, tolerance); // WHAT: near the older's ink. WHY: a line within tolerance of the other is the same line.
    let near_n = grow(&ink_n.on, width, height, tolerance); // WHAT: near the newer's ink. WHY: the same.
    let strength = |v: u8, paper: f32| -> f32 {
        (((paper - f32::from(v)) / paper.max(1.0) - 0.1) / 0.65).clamp(0.0, 1.0)
    }; // WHAT: how dark a pixel is against its paper, nought to one. WHY: a tenth below paper is noise; the rest fades in.
    let mut out = vec![[255u8; 3]; width * height]; // WHAT: white paper. WHY: drawn on below.

    // WHAT: every pixel. WHY: coloured.
    for i in 0..width * height {
        let (sa, sn) = (strength(older[i], pa), strength(newer[i], pn)); // WHAT: how dark each is here. WHY: the pixel's strength and colour.

        // WHAT: paper in both. WHY: left white.
        if sa <= 0.0 && sn <= 0.0 {
            continue; // WHAT: paper in both. WHY: stays white.
        }
        let both = (sa > 0.0 && near_n[i]) || (sn > 0.0 && near_a[i]); // WHAT: a line of one with the other's close by. WHY: that line did not move.
        let hue = if both {
            BOTH // WHAT: dark grey. WHY: did not move.
        } else if sa >= sn {
            // WHAT: the older is darker here. WHY: its colour.
            OLDER // WHAT: red. WHY: gone in the newer.
        } else {
            NEWER
        }; // WHAT: the colour. WHY: both, gone or arrived.
        let s = sa.max(sn); // WHAT: as strong as the darker. WHY: soft edges stay soft.
        out[i] = hue.map(|c| {
            (255.0 - s * (255.0 - f32::from(c)))
                .round()
                .clamp(0.0, 255.0) as u8
        }); // WHAT: the colour at that strength, on white. WHY: transparent ink, not stickers.
    }
    out // WHAT: the picture. WHY: handed back.
}

/// A picture made smaller by averaging, to at most `most` pixels across or down.
#[must_use]
// WHAT: a picture made smaller. WHY: the window's copy.
pub fn smaller(
    pixels: &[[u8; 3]],
    width: usize,
    height: usize,
    most: usize,
) -> (Vec<[u8; 3]>, usize, usize) {
    let factor = width.max(height).div_ceil(most.max(1)).max(1); // WHAT: how many pixels make one. WHY: under the limit.

    // WHAT: small enough already. WHY: unchanged.
    if factor == 1 {
        return (pixels.to_vec(), width, height); // WHAT: already small enough. WHY: unchanged.
    }
    let (w, h) = (width.div_ceil(factor), height.div_ceil(factor)); // WHAT: the new size. WHY: rounded up so nothing is cut off.
    let mut sums = vec![[0u32; 4]; w * h]; // WHAT: the three colours and a count, per new pixel. WHY: averaged at the end.

    // WHAT: every row. WHY: summed into the smaller.
    for y in 0..height {
        // WHAT: every pixel. WHY: the same.
        for x in 0..width {
            let p = pixels[y * width + x]; // WHAT: one pixel. WHY: added to its new pixel.
            let s = &mut sums[(y / factor) * w + x / factor]; // WHAT: the new pixel it falls in. WHY: summed there.
            s[0] += u32::from(p[0]); // WHAT: red. WHY: summed.
            s[1] += u32::from(p[1]); // WHAT: green. WHY: summed.
            s[2] += u32::from(p[2]); // WHAT: blue. WHY: summed.
            s[3] += 1; // WHAT: counted. WHY: the average's bottom.
        }
    }
    let out = sums
        .iter()
        .map(|s| [s[0], s[1], s[2]].map(|c| u8::try_from(c / s[3].max(1)).unwrap_or(255)))
        .collect(); // WHAT: each new pixel, averaged. WHY: a smooth smaller picture.
    (out, w, h)
}

#[cfg(test)]
// WHAT: the module's tests. WHY: fits, colours and pairing checked without pdfium.
mod tests {
    use super::*; // WHAT: the module under test. WHY: tests sit beside it.

    // WHAT: an empty sheet. WHY: drawn on.
    fn blank(w: usize, h: usize) -> Ink {
        Ink {
            width: w,
            height: h,
            on: vec![false; w * h],
        } // WHAT: an empty sheet. WHY: drawn on by each test.
    }

    /// A line drawn by stepping along it, a pixel thick.
    // WHAT: draws a line. WHY: test drawings.
    fn line(ink: &mut Ink, a: [f64; 2], b: [f64; 2]) {
        let steps = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()) * 2.0)
            .ceil()
            .max(1.0) as usize; // WHAT: half-pixel steps. WHY: no gaps.

        // WHAT: each step along it. WHY: no gaps.
        for i in 0..=steps {
            let t = i as f64 / steps as f64; // WHAT: how far along. WHY: the point.
            let (x, y) = (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t); // WHAT: the point. WHY: inked.

            // WHAT: on the sheet. WHY: inked.
            if x >= 0.0 && y >= 0.0 && (x as usize) < ink.width && (y as usize) < ink.height {
                let w = ink.width; // WHAT: the row length. WHY: the index.
                ink.on[y as usize * w + x as usize] = true; // WHAT: inks it. WHY: the line.
            }
        }
    }

    /// A small plan, drawn through a fit, on a sheet of the given size.
    // WHAT: a small plan through a fit. WHY: the same drawing moved, turned or scaled.
    fn plan(w: usize, h: usize, fit: Fit) -> Ink {
        let mut ink = blank(w, h); // WHAT: the sheet. WHY: drawn on.
        let mut seg = |a: [f64; 2], b: [f64; 2]| line(&mut ink, fit.apply(a), fit.apply(b)); // WHAT: a segment through the fit. WHY: the same plan anywhere.
        seg([10.0, 10.0], [390.0, 10.0]); // WHAT: the border, top. WHY: a sheet has one.
        seg([390.0, 10.0], [390.0, 260.0]); // WHAT: right. WHY: the same.
        seg([390.0, 260.0], [10.0, 260.0]); // WHAT: bottom. WHY: the same.
        seg([10.0, 260.0], [10.0, 10.0]); // WHAT: left. WHY: the same.

        // WHAT: seven grid lines. WHY: a plan's grid.
        for i in 0..7 {
            let x = 40.0 + f64::from(i) * 40.0; // WHAT: a grid line across. WHY: a plan's grid.
            seg([x, 30.0], [x, 200.0]); // WHAT: draws it. WHY: the same.
        }
        seg([300.0, 210.0], [385.0, 210.0]); // WHAT: the title block's top. WHY: makes the sheet lopsided, as real ones are.
        seg([300.0, 210.0], [300.0, 255.0]); // WHAT: its side. WHY: the same.
        seg([310.0, 230.0], [370.0, 245.0]); // WHAT: something in it. WHY: the same.
        ink // WHAT: the plan. WHY: handed back.
    }

    #[test]
    // WHAT: the test. WHY: an added sheet must not shift the pairs after it.
    fn sheets_pair_by_number_so_an_added_sheet_does_not_shift_the_rest() {
        let names = |l: &[&str]| l.iter().map(|s| (*s).to_string()).collect::<Vec<_>>(); // WHAT: sheet numbers as owned text. WHY: the shape the reader gives.
        let older = names(&["S-101", "S-102", "S-103"]); // WHAT: three sheets. WHY: the older issue.
        let newer = names(&["S-101", "S-101A", "s-102 ", "S-103"]); // WHAT: one added, one written differently. WHY: a revision.
        assert_eq!(
            pair_sheets(&newer, &older),
            vec![Some(0), None, Some(1), Some(2)]
        ); // WHAT: matched by number. WHY: the added sheet has no partner.
        let blank = names(&["", "", ""]); // WHAT: no numbers at all. WHY: by position.
        assert_eq!(
            pair_sheets(&names(&["", ""]), &blank),
            vec![Some(0), Some(1)]
        ); // WHAT: by position. WHY: nothing else to go on.
        assert_eq!(
            pair_sheets(&names(&["", "", "", ""]), &blank),
            vec![Some(0), Some(1), Some(2), None]
        ); // WHAT: the extra sheet has none. WHY: no older sheet there.
        let mixed = names(&["S-101", ""]); // WHAT: one numbered, one not. WHY: a cover sheet with no number.
        assert_eq!(
            pair_sheets(&mixed, &names(&["S-101", ""])),
            vec![Some(0), Some(1)]
        ); // WHAT: the unnumbered pair by place. WHY: both unnumbered there.
        assert_eq!(
            pair_sheets(&mixed, &names(&["S-101", "S-102"])),
            vec![Some(0), None]
        ); // WHAT: an unnumbered sheet never takes a numbered one. WHY: that would be a guess.
        assert_eq!(
            pair_sheets(&names(&["S-1", "S-1"]), &names(&["S-1"])),
            vec![Some(0), None]
        ); // WHAT: a number used twice pairs once. WHY: each older sheet pairs once.
    }

    #[test]
    // WHAT: the test. WHY: a hand-picked fit a pixel or two out is settled.
    fn a_fit_picked_by_hand_is_settled_onto_the_line_work() {
        let older = plan(400, 270, Fit::SAME); // WHAT: the reference. WHY: the same plan both sides.
        let newer = plan(
            400,
            270,
            Fit {
                dx: 20.0,
                dy: 10.0,
                ..Fit::SAME
            },
        ); // WHAT: moved twenty and ten. WHY: what the points should find.
        let found = settle(
            &older,
            &newer,
            Fit {
                dx: 18.5,
                dy: 11.0,
                ..Fit::SAME
            },
        ); // WHAT: clicked a pixel or two out. WHY: a click is never exact.
        assert!(
            (found.fit.dx - 20.0).abs() < 0.75 && (found.fit.dy - 10.0).abs() < 0.75,
            "{found:?}"
        ); // WHAT: settled onto it. WHY: within a pixel.
        assert!(found.agreement > 0.9, "{found:?}"); // WHAT: it agrees. WHY: the same drawing.
        assert!(
            agreement(&older, &newer, &Fit::SAME) < found.agreement,
            "left as it is, it agrees less"
        ); // WHAT: unmoved is worse. WHY: the fit is what lines them up.
    }

    #[test]
    // WHAT: the test. WHY: warping moves ink by the fit and keeps hairlines.
    fn warping_moves_the_ink_by_the_fit() {
        let mut older = blank(50, 40); // WHAT: a small sheet. WHY: easy to check.
        older.on[10 * 50 + 10] = true; // WHAT: one dot at 10,10. WHY: moved below.
        let moved = warp(
            &older,
            &Fit {
                dx: 5.0,
                dy: 3.0,
                ..Fit::SAME
            },
            50,
            40,
        ); // WHAT: five across, three down. WHY: the test.
        assert!(
            moved.at(15, 13) && moved.count() == 1,
            "the dot moved and only it: {}",
            moved.count()
        ); // WHAT: at 15,13. WHY: moved by the fit.
        let half = warp(
            &older,
            &Fit {
                scale: 0.5,
                ..Fit::SAME
            },
            50,
            40,
        ); // WHAT: half size. WHY: a scaled fit.
        assert!(half.at(5, 5), "half size puts it at 5,5"); // WHAT: halved. WHY: the scale.
    }

    #[test]
    // WHAT: the test. WHY: back undoes apply.
    fn a_fit_and_its_inverse_undo_each_other() {
        let fit = Fit {
            scale: 0.5,
            turn: 0.3,
            dx: 12.0,
            dy: -7.0,
        }; // WHAT: any fit. WHY: the property holds for all.
        let p = [123.0, 45.0]; // WHAT: any point. WHY: the same.
        let q = fit.back(fit.apply(p)); // WHAT: there and back. WHY: should be where it started.
        assert!(
            (q[0] - p[0]).abs() < 1e-9 && (q[1] - p[1]).abs() < 1e-9,
            "{q:?}"
        ); // WHAT: the same point. WHY: `back` is `apply` undone.
    }

    #[test]
    // WHAT: the test. WHY: two points fix the whole fit.
    fn two_points_on_each_sheet_give_the_fit_that_joins_them() {
        let truth = Fit {
            scale: 2.0,
            turn: std::f64::consts::FRAC_PI_2,
            dx: 100.0,
            dy: 5.0,
        }; // WHAT: the fit to recover. WHY: scaled, turned and moved.
        let (a, b) = ([10.0, 20.0], [300.0, 40.0]); // WHAT: two points on the older sheet. WHY: picked by somebody.
        let fit = Fit::from_points([a, b], [truth.apply(a), truth.apply(b)]).expect("a fit"); // WHAT: the fit from where they landed. WHY: the two-point line-up.

        // WHAT: two other points. WHY: the whole sheet follows the fit.
        for p in [[0.0, 0.0], [500.0, 300.0]] {
            let (want, got) = (truth.apply(p), fit.apply(p)); // WHAT: anywhere else on the sheet. WHY: the whole fit, not just the two points.
            assert!(
                (want[0] - got[0]).abs() < 1e-6 && (want[1] - got[1]).abs() < 1e-6,
                "{want:?} {got:?}"
            ); // WHAT: the same. WHY: two points fix a turn, a scale and a shift.
        }
        assert!(
            Fit::from_points([a, a], [b, [1.0, 2.0]]).is_none(),
            "one point twice is not a line-up"
        ); // WHAT: refused. WHY: no turn or scale follows.
        assert!(
            Fit::from_points([a, b], [b, b]).is_none(),
            "nor on the newer sheet"
        ); // WHAT: refused. WHY: the same.
    }

    #[test]
    // WHAT: the test. WHY: growing reaches exactly r.
    fn growing_ink_reaches_exactly_as_far_as_asked() {
        let mut ink = blank(11, 11); // WHAT: a small sheet. WHY: easy to check by hand.
        ink.on[5 * 11 + 5] = true; // WHAT: one dot in the middle. WHY: grown below.
        let grown = grow(&ink.on, 11, 11, 2); // WHAT: two pixels out. WHY: the test.

        // WHAT: every row. WHY: checked.
        for y in 0..11usize {
            // WHAT: every pixel. WHY: checked.
            for x in 0..11usize {
                let inside = x.abs_diff(5) <= 2 && y.abs_diff(5) <= 2; // WHAT: within two of the dot. WHY: what should be inked.
                assert_eq!(grown[y * 11 + x], inside, "at {x},{y}"); // WHAT: exactly that square. WHY: no more, no less.
            }
        }
        assert_eq!(
            grow(&ink.on, 11, 11, 0),
            ink.on,
            "growing by nothing changes nothing"
        ); // WHAT: unchanged. WHY: the edge case.
    }

    #[test]
    // WHAT: the test. WHY: halving keeps a hairline.
    fn halving_keeps_a_hairline() {
        let mut ink = blank(10, 10); // WHAT: a small sheet. WHY: easy to check.
        line(&mut ink, [0.0, 3.0], [9.0, 3.0]); // WHAT: a one-pixel line. WHY: the kind averaging would lose.
        let half = halved(&ink); // WHAT: half the size. WHY: the test.
        assert_eq!((half.width, half.height), (5, 5)); // WHAT: half. WHY: rounded up.
        assert_eq!(half.count(), 5, "the hairline is still a whole line"); // WHAT: the line survives. WHY: any ink keeps a pixel.
    }

    #[test]
    // WHAT: the test. WHY: a re-plot's shift is found.
    fn a_moved_sheet_is_found_where_it_moved_to() {
        let older = plan(400, 270, Fit::SAME); // WHAT: the older issue. WHY: the reference.
        let newer = plan(
            400,
            270,
            Fit {
                dx: 9.0,
                dy: -6.0,
                ..Fit::SAME
            },
        ); // WHAT: replotted nine across and six up. WHY: a different plotter.
        let found = find(
            &older,
            &newer,
            &starts([400.0, 270.0], [400.0, 270.0], 1.0, None),
        ); // WHAT: fits it. WHY: the test.
        assert!(
            (found.fit.dx - 9.0).abs() < 1.0 && (found.fit.dy + 6.0).abs() < 1.0,
            "{found:?}"
        ); // WHAT: the shift found. WHY: within a pixel.
        assert!(found.agreement > 0.9, "{found:?}"); // WHAT: it agrees. WHY: the same drawing.
    }

    #[test]
    // WHAT: the test. WHY: a half-size print is fitted at half size.
    fn a_half_size_print_is_fitted_at_half_size() {
        let older = plan(400, 270, Fit::SAME); // WHAT: full size. WHY: the reference.
        let newer = plan(
            200,
            135,
            Fit {
                scale: 0.5,
                ..Fit::SAME
            },
        ); // WHAT: half size, on half-size paper. WHY: an 11 by 17 set of a 22 by 34 one.
        let found = find(
            &older,
            &newer,
            &starts([400.0, 270.0], [200.0, 135.0], 1.0, None),
        ); // WHAT: same points would say scale one. WHY: the fitted start has to win.
        assert!((found.fit.scale - 0.5).abs() < 0.02, "{found:?}"); // WHAT: half. WHY: the print is half size.
        assert!(found.agreement > 0.85, "{found:?}"); // WHAT: it agrees. WHY: the same drawing.
    }

    #[test]
    // WHAT: the test. WHY: a sheet stored turned is turned back.
    fn a_sheet_turned_a_quarter_is_turned_back() {
        let older = plan(400, 270, Fit::SAME); // WHAT: landscape. WHY: the reference.
        let newer = plan(
            270,
            400,
            Fit {
                turn: std::f64::consts::FRAC_PI_2,
                dx: 270.0,
                ..Fit::SAME
            },
        ); // WHAT: the same, turned a quarter on portrait paper. WHY: stored sideways.
        let found = find(
            &older,
            &newer,
            &starts([400.0, 270.0], [270.0, 400.0], 1.0, None),
        ); // WHAT: fits it. WHY: the test.
        assert!(
            (found.fit.degrees() - 90.0).abs() < 1.0,
            "{found:?} {}",
            found.fit.degrees()
        ); // WHAT: a quarter turn. WHY: how it was stored.
        assert!(found.agreement > 0.85, "{found:?}"); // WHAT: it agrees. WHY: the same drawing.
    }

    #[test]
    // WHAT: the test. WHY: a skewed scan is straightened.
    fn a_skewed_scan_is_straightened() {
        let older = plan(400, 270, Fit::SAME); // WHAT: straight. WHY: the reference.
        let skew = turned_about_middle(Fit::SAME, 1.2_f64.to_radians(), &Level::new(&older)); // WHAT: turned 1.2 degrees about the middle. WHY: a sheet feeder's skew.
        let newer = plan(400, 270, skew); // WHAT: the skewed scan. WHY: the test.
        let found = find(
            &older,
            &newer,
            &starts([400.0, 270.0], [400.0, 270.0], 1.0, None),
        ); // WHAT: fits it. WHY: the same.
        assert!((found.fit.turn.to_degrees() - 1.2).abs() < 0.3, "{found:?}"); // WHAT: the skew found. WHY: within a third of a degree.
        assert!(found.agreement > 0.85, "{found:?}"); // WHAT: it agrees. WHY: the same drawing.
    }

    #[test]
    // WHAT: the test. WHY: different sheets do not line up.
    fn two_different_sheets_do_not_agree() {
        let older = plan(400, 270, Fit::SAME); // WHAT: a plan. WHY: one sheet.
        let mut newer = blank(400, 270); // WHAT: something else entirely. WHY: not an issue of the same sheet.
        line(&mut newer, [20.0, 20.0], [380.0, 250.0]); // WHAT: a diagonal. WHY: nothing like the plan.
        line(&mut newer, [20.0, 250.0], [380.0, 20.0]); // WHAT: the other. WHY: the same.
        let found = find(
            &older,
            &newer,
            &starts([400.0, 270.0], [400.0, 270.0], 1.0, None),
        ); // WHAT: fits it. WHY: the test.
        assert!(
            found.agreement < crate::compare::ENOUGH_AGREEMENT,
            "{found:?}"
        ); // WHAT: under the bar. WHY: they are different sheets.
    }

    #[test]
    // WHAT: the test. WHY: the files' own fit is tried first.
    fn the_files_own_fit_wins_a_tie() {
        let own = Fit {
            dx: 3.0,
            ..Fit::SAME
        }; // WHAT: what the files' coordinates say. WHY: offered first.
        let list = starts([400.0, 270.0], [400.0, 270.0], 1.0, Some(own)); // WHAT: the starts. WHY: checked below.
        assert_eq!(list[0], own, "the files' own fit is tried first"); // WHAT: first. WHY: it wins ties.
        assert!(list.len() >= 4, "every quarter turn is tried: {list:?}"); // WHAT: the turns are there. WHY: a sideways sheet.
    }

    #[test]
    // WHAT: the test. WHY: blank sheets agree, blank and drawn do not.
    fn two_blank_sheets_agree_and_a_blank_one_does_not_agree_with_a_drawing() {
        let found = find(&blank(100, 80), &blank(100, 80), &[Fit::SAME]); // WHAT: two blanks. WHY: the edge case.
        assert!((found.agreement - 1.0).abs() < 1e-6, "{found:?}"); // WHAT: they agree. WHY: an empty page has not changed.
        let found = find(&blank(400, 270), &plan(400, 270, Fit::SAME), &[]); // WHAT: blank against a plan, with no starts. WHY: the other edge case.
        assert!(found.agreement < 0.5, "{found:?}"); // WHAT: they do not. WHY: one has a drawing.
    }

    #[test]
    // WHAT: the test. WHY: a grey scan's paper is not ink.
    fn a_grey_scan_has_grey_paper_not_ink_everywhere() {
        let mut luma = vec![200u8; 1000]; // WHAT: grey paper. WHY: a scan.

        // WHAT: the first fifty pixels. WHY: a line.
        for v in luma.iter_mut().take(50) {
            *v = 20; // WHAT: some ink. WHY: a line.
        }
        assert_eq!(paper_of(&luma), 200, "the paper is the grey"); // WHAT: grey. WHY: most of the sheet.
        let ink = ink_of(&luma, 1000, 1, 0.65); // WHAT: ink against that paper. WHY: the test.
        assert_eq!(ink.count(), 50, "only the line is ink"); // WHAT: fifty pixels. WHY: the paper is not ink.
        assert_eq!(paper_of(&[]), 255, "nothing at all reads as white"); // WHAT: white. WHY: the edge case.
        assert_eq!(
            paper_of(&[0u8; 100]),
            32,
            "an all-black picture still leaves room for ink"
        ); // WHAT: the floor. WHY: never so dark that nothing is ink.
    }

    #[test]
    // WHAT: the test. WHY: the three colours mean what they say.
    fn colours_say_older_newer_and_both() {
        let (w, h) = (20, 3); // WHAT: a strip. WHY: three cases side by side.
        let mut older = vec![255u8; w * h]; // WHAT: the older issue. WHY: drawn below.
        let mut newer = vec![255u8; w * h]; // WHAT: the newer. WHY: the same.
        older[w + 2] = 0; // WHAT: a dot only in the older. WHY: removed.
        newer[w + 10] = 0; // WHAT: a dot only in the newer. WHY: added.
        older[w + 16] = 0; // WHAT: a dot in both, one pixel apart. WHY: within tolerance.
        newer[w + 17] = 0; // WHAT: the other half. WHY: the same.
        let out = colour(&older, &newer, w, h, 0.65, 1); // WHAT: coloured. WHY: the test.
        assert_eq!(out[w + 2], OLDER, "only in the older is red"); // WHAT: red. WHY: gone.
        assert_eq!(out[w + 10], NEWER, "only in the newer is blue"); // WHAT: blue. WHY: arrived.
        assert_eq!(out[w + 16], BOTH, "within tolerance of each other is both"); // WHAT: dark. WHY: did not move.
        assert_eq!(out[w + 17], BOTH, "both halves"); // WHAT: dark. WHY: the same.
        assert_eq!(out[0], [255, 255, 255], "paper stays white"); // WHAT: white. WHY: nothing there.
        let strict = colour(&older, &newer, w, h, 0.65, 0); // WHAT: no tolerance. WHY: then a pixel apart is apart.
        assert_eq!(
            strict[w + 16],
            OLDER,
            "with no tolerance a pixel apart is two colours"
        ); // WHAT: red. WHY: tolerance is what makes it both.
    }

    #[test]
    // WHAT: the test. WHY: soft edges and scan grey come out right.
    fn soft_edges_stay_soft_and_scan_grey_stays_paper() {
        let older = vec![200u8, 120, 0]; // WHAT: grey paper, a mid grey, black. WHY: a scan's pixels.
        let newer = vec![255u8, 255, 255]; // WHAT: white. WHY: nothing in the newer.
        let out = colour(&older, &newer, 3, 1, 0.65, 0); // WHAT: coloured. WHY: the test.
        assert_eq!(out[0], [255, 255, 255], "the scan's own paper is white"); // WHAT: white. WHY: grey paper is not ink.
        assert_eq!(out[2], OLDER, "black is full red"); // WHAT: full colour. WHY: full strength.
        assert!(
            out[1][1] > OLDER[1] && out[1][1] < 255,
            "mid grey is a lighter red: {:?}",
            out[1]
        ); // WHAT: between. WHY: transparent ink.
    }

    #[test]
    // WHAT: the test. WHY: shrinking averages and loses nothing.
    fn a_picture_made_smaller_is_averaged_and_never_cut_off() {
        let pixels = vec![
            [0u8, 0, 0],
            [255, 255, 255],
            [0, 0, 0],
            [255, 255, 255],
            [10, 20, 30],
        ]; // WHAT: five pixels in a row. WHY: halved to three.
        let (out, w, h) = smaller(&pixels, 5, 1, 3); // WHAT: at most three across. WHY: the test.
        assert_eq!((w, h), (3, 1)); // WHAT: three. WHY: rounded up.
        assert_eq!(out[0], [127, 127, 127], "two averaged"); // WHAT: the average. WHY: smooth.
        assert_eq!(out[2], [10, 20, 30], "the last one is not lost"); // WHAT: kept. WHY: nothing cut off.
        let (same, w, _) = smaller(&pixels, 5, 1, 10); // WHAT: already small enough. WHY: the edge case.
        assert_eq!((same.len(), w), (5, 5)); // WHAT: unchanged. WHY: no averaging needed.
    }
}
