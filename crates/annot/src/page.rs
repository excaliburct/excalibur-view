//! Sheet coordinates and PDF coordinates.
//!
//! Two coordinate systems meet here and getting them confused puts markups in
//! the wrong place on the sheet.
//!
//! * **Sheet space** is what the screen and the renderer use: the top left
//!   corner of the sheet as it appears is (0,0), x goes right, y goes down, and
//!   the sheet has already been turned by whatever `/Rotate` says.
//! * **PDF space** is what the file uses: the origin is the bottom left corner
//!   of the page box, which is often not (0,0) at all, y goes up, and nothing
//!   has been rotated.
//!
//! Everything the user clicks arrives in sheet space. Everything written into
//! the file has to be in PDF space.

/// The page box and its rotation, and the sums that get between the two.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// The page box in PDF space: left, bottom, right, top.
    pub area: [f64; 4],
    /// 0, 90, 180 or 270, clockwise.
    pub rotation: i32,
}

impl Frame {
    pub fn new(area: [f64; 4], rotation: i32) -> Frame {
        Frame {
            area,
            rotation: rotation.rem_euclid(360) / 90 * 90, // WHAT: a quarter turn, 0 to 270. WHY: rem_euclid is the same sum, written the way the strict lint asks
        }
    }

    /// Reads it off a page.
    pub fn of(doc: &pdf::Document, page: &pdf::Dict) -> Frame {
        Frame::new(doc.page_box(page), doc.page_rotation(page))
    }

    /// The width and height of the sheet as it appears, after rotation.
    pub fn size(&self) -> (f64, f64) {
        let across = self.area[2] - self.area[0];
        let down = self.area[3] - self.area[1];
        if self.rotation == 90 || self.rotation == 270 {
            (down, across)
        } else {
            (across, down)
        }
    }

    /// Sheet space to PDF space.
    pub fn to_pdf(&self, at: [f64; 2]) -> [f64; 2] {
        let [x0, y0, x1, y1] = self.area;
        let across = x1 - x0;
        let down = y1 - y0;
        let (dx, dy) = (at[0], at[1]);
        let _ = (across, down);
        match self.rotation {
            90 => [x0 + dy, y0 + dx],
            180 => [x1 - dx, y0 + dy],
            270 => [x1 - dy, y1 - dx],
            _ => [x0 + dx, y1 - dy],
        }
    }

    /// PDF space to sheet space.
    pub fn to_sheet(&self, at: [f64; 2]) -> [f64; 2] {
        let [x0, y0, x1, y1] = self.area;
        let (px, py) = (at[0], at[1]);
        match self.rotation {
            90 => [py - y0, px - x0],
            180 => [x1 - px, py - y0],
            270 => [y1 - py, x1 - px],
            _ => [px - x0, y1 - py],
        }
    }

    /// A whole run of points, sheet to PDF.
    pub fn points_to_pdf(&self, points: &[[f64; 2]]) -> Vec<[f64; 2]> {
        points.iter().map(|p| self.to_pdf(*p)).collect()
    }

    /// A whole run of points, PDF to sheet.
    pub fn points_to_sheet(&self, points: &[[f64; 2]]) -> Vec<[f64; 2]> {
        points.iter().map(|p| self.to_sheet(*p)).collect()
    }

    /// Distances are the same in both systems, since neither scales the other.
    /// Only the direction changes, which is why a length can be measured in
    /// sheet space and written in PDF space without converting it.
    pub fn same_scale(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    #[test]
    fn a_plain_sheet_flips_top_for_bottom() {
        let f = Frame::new([0.0, 0.0, 612.0, 792.0], 0);
        assert_eq!(f.size(), (612.0, 792.0));
        // The top left of the sheet is the top left of the page box.
        assert!(close(f.to_pdf([0.0, 0.0]), [0.0, 792.0]));
        assert!(close(f.to_pdf([612.0, 792.0]), [612.0, 0.0]));
        assert!(close(f.to_sheet([0.0, 792.0]), [0.0, 0.0]));
    }

    #[test]
    fn a_page_box_that_does_not_start_at_zero_is_allowed_for() {
        // The combined Elm Theater set lays its sheets out around the origin.
        let f = Frame::new([-1512.0, -1080.0, 1512.0, 1080.0], 0);
        assert_eq!(f.size(), (3024.0, 2160.0));
        assert!(close(f.to_pdf([0.0, 0.0]), [-1512.0, 1080.0]));
        assert!(close(f.to_pdf([3024.0, 2160.0]), [1512.0, -1080.0]));
        assert!(close(f.to_sheet([0.0, 0.0]), [1512.0, 1080.0]));
    }

    #[test]
    fn every_rotation_round_trips() {
        for rotation in [0, 90, 180, 270] {
            let f = Frame::new([10.0, 20.0, 610.0, 820.0], rotation);
            for point in [[0.0, 0.0], [100.0, 50.0], [599.0, 799.0], [7.5, 321.25]] {
                let there = f.to_pdf(point);
                let back = f.to_sheet(there);
                assert!(close(back, point), "{rotation}: {point:?} -> {there:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn a_turned_sheet_swaps_its_width_and_height() {
        let f = Frame::new([0.0, 0.0, 612.0, 792.0], 90);
        assert_eq!(f.size(), (792.0, 612.0));
        let f = Frame::new([0.0, 0.0, 612.0, 792.0], 270);
        assert_eq!(f.size(), (792.0, 612.0));
        let f = Frame::new([0.0, 0.0, 612.0, 792.0], 180);
        assert_eq!(f.size(), (612.0, 792.0));
    }

    #[test]
    fn the_corners_of_a_turned_sheet_land_where_they_should() {
        let f = Frame::new([0.0, 0.0, 612.0, 792.0], 90);
        // Turned a quarter clockwise, the sheet's top left is the page's
        // bottom left corner.
        assert!(close(f.to_pdf([0.0, 0.0]), [0.0, 0.0]));
        // And the sheet's top right is the page's bottom right... which in the
        // unrotated page is its top left.
        assert!(close(f.to_pdf([792.0, 0.0]), [0.0, 792.0]));
        assert!(close(f.to_pdf([0.0, 612.0]), [612.0, 0.0]));
    }

    #[test]
    fn a_distance_is_the_same_length_in_both_systems() {
        for rotation in [0, 90, 180, 270] {
            let f = Frame::new([-100.0, -50.0, 500.0, 750.0], rotation);
            let a = f.to_pdf([100.0, 100.0]);
            let b = f.to_pdf([400.0, 500.0]);
            let in_pdf = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            let in_sheet = ((400.0f64 - 100.0).powi(2) + (500.0f64 - 100.0).powi(2)).sqrt();
            assert!((in_pdf - in_sheet).abs() < 1e-9, "{rotation}");
        }
    }

    #[test]
    fn an_odd_rotation_is_rounded_to_a_quarter_turn() {
        assert_eq!(Frame::new([0.0, 0.0, 1.0, 1.0], -90).rotation, 270);
        assert_eq!(Frame::new([0.0, 0.0, 1.0, 1.0], 450).rotation, 90);
        assert_eq!(Frame::new([0.0, 0.0, 1.0, 1.0], 100).rotation, 90);
    }
}
