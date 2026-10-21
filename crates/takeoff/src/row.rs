//! One line of a takeoff: a markup, measured.

use annot::measure::Measure;
use annot::{Kind, Markup, Subtype};
use pdf::{Document, Ref};

use crate::geometry;

/// Which user column carries a unit weight. These are the positions the user's
/// own tool chest uses; they are settings rather than assumptions so a
/// different chest can say something different.
#[derive(Clone, Copy, Debug)]
pub struct WeightColumns {
    /// Pounds per foot, for the linear shapes.
    pub per_length: usize,
    /// Pounds per square foot, for plate, grating and sheet.
    pub per_area: usize,
}

impl Default for WeightColumns {
    fn default() -> WeightColumns {
        WeightColumns {
            per_length: 0,
            per_area: 5,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub page: usize,
    pub reference: Ref,
    pub subject: String,
    pub label: String,
    pub author: String,
    pub comments: String,
    pub status: String,
    pub date: String,
    pub created: String,
    pub colour: [f32; 3],
    pub kind: Kind,
    /// What the markup says on the sheet.
    pub caption: String,
    /// The unit lengths are counted in — feet on an imperial drawing.
    pub unit: String,
    pub count: f64,
    pub length: Option<f64>,
    pub area: Option<f64>,
    pub perimeter: Option<f64>,
    pub volume: Option<f64>,
    pub depth: Option<f64>,
    pub wall_area: Option<f64>,
    pub angle: Option<f64>,
    pub radius: Option<f64>,
    pub diameter: Option<f64>,
    pub slope: Option<f64>,
    /// The run the slope's rise is over — twelve, unless the markup says.
    pub pitch_run: f64,
    pub area_box: [f64; 4],
    pub columns: [String; 6],
    /// The scale this row was measured with, kept so the grid can show the
    /// numbers the way the sheet reads them.
    pub scale: Option<Measure>,
    /// False when the sheet has no scale. Such a row is reported, and left out
    /// of every total, rather than counted as zero.
    pub scaled: bool,
    /// Where this is on the sheet's grid: "B-4", "B/C-4", or empty.
    ///
    /// A label, never a quantity. It is filled in by whoever knows the
    /// sheet's grid — nothing in here works it out, and nothing in here adds
    /// it up. Empty means the sheet has no grid, or this is off the end of
    /// it, and both of those are answers rather than gaps.
    pub grid: String,
    pub locked: bool,
    /// How many of this there are: a beam drawn once on a typical floor that
    /// repeats on four is four beams. One unless somebody said otherwise.
    /// Every total multiplies by it; the measurement itself does not change.
    pub quantity: f64,
}

impl Row {
    /// WHAT: Feet in one of the unit this row was measured in; `None` on an
    /// unscaled sheet or a unit not known. WHY: lengths, areas and volumes go
    /// out in feet whatever the sheet was scaled in (a metric sheet measures in
    /// millimetres), and an unknown unit is never sent as feet.
    #[must_use]
    pub fn feet_per_unit(&self) -> Option<f64> {
        // WHAT: the scale's own unit in feet. WHY: the row's numbers are in that unit.
        self.scale.as_ref().and_then(Measure::feet_per_unit)
    }

    /// WHAT: The length in feet; `None` when unmeasured or the unit isn't known.
    /// WHY: what a reader asking for feet must get.
    #[must_use]
    pub fn length_ft(&self) -> Option<f64> {
        // WHAT: converted by the foot. WHY: as above.
        self.length.zip(self.feet_per_unit()).map(|(v, f)| v * f)
    }

    /// WHAT: The area in square feet; `None` as for the length. WHY: an area
    /// converts by the square of the unit.
    #[must_use]
    pub fn area_sqft(&self) -> Option<f64> {
        // WHAT: converted by the square. WHY: as above.
        self.area.zip(self.feet_per_unit()).map(|(v, f)| v * f * f)
    }

    /// WHAT: The volume in cubic feet; `None` as for the length. WHY: a volume
    /// converts by the cube.
    #[must_use]
    pub fn volume_cuft(&self) -> Option<f64> {
        // WHAT: converted by the cube. WHY: as above.
        self.volume
            .zip(self.feet_per_unit())
            .map(|(v, f)| v * f * f * f)
    }

    /// WHAT: The unit this row was measured in, as its scale writes it; "" on
    /// an unscaled sheet. WHY: sent beside the feet, so a reader sees what was
    /// converted.
    #[must_use]
    pub fn measured_unit(&self) -> &str {
        // WHAT: the scale's first unit. WHY: the numbers' own unit.
        self.scale.as_ref().map_or("", Measure::primary_unit)
    }

    /// Weight in pounds, from the markup's own column data. `None` means the
    /// markup carries no weight, which is not the same as weighing nothing.
    pub fn pounds(&self, which: WeightColumns) -> Option<f64> {
        if !self.scaled {
            return None;
        }
        let per_length = number(self.columns.get(which.per_length)?);
        let per_area = number(self.columns.get(which.per_area)?);
        let each = match (per_length, self.length, per_area, self.area) {
            (Some(w), Some(run), _, _) => Some(w * run),
            (_, _, Some(w), Some(area)) => Some(w * area),
            _ => None,
        };
        each.map(|lb| lb * self.quantity)
    }

    pub fn tons(&self, which: WeightColumns) -> Option<f64> {
        self.pounds(which).map(|lb| lb / 2000.0)
    }

    /// Pounds per foot from the tool chest, for a linear shape.
    ///
    /// The cut list needs the unit weight itself rather than the total, so it
    /// can say what a shape weighs per foot beside what it weighs altogether.
    /// `None` means the chest carried no weight for this markup, which is not
    /// the same as a weight of zero.
    pub fn per_length_weight(&self, which: WeightColumns) -> Option<f64> {
        number(self.columns.get(which.per_length)?)
    }

    /// Pounds per square foot from the tool chest, for plate and sheet.
    pub fn per_area_weight(&self, which: WeightColumns) -> Option<f64> {
        number(self.columns.get(which.per_area)?)
    }

    /// An empty row: no page, no measurement, nothing measured.
    ///
    /// Every quantity on it is `None` rather than zero, because the difference
    /// between "nothing was measured" and "it measured nothing" is the whole
    /// argument this program is built around.
    pub fn blank() -> Row {
        Row {
            page: 0,
            reference: Ref::new(0, 0),
            subject: String::new(),
            label: String::new(),
            author: String::new(),
            comments: String::new(),
            status: String::new(),
            date: String::new(),
            created: String::new(),
            colour: [0.0, 0.0, 0.0],
            kind: Kind::Markup,
            caption: String::new(),
            unit: String::new(),
            count: 0.0,
            length: None,
            area: None,
            perimeter: None,
            volume: None,
            depth: None,
            wall_area: None,
            angle: None,
            radius: None,
            diameter: None,
            slope: None,
            pitch_run: 12.0,
            area_box: [0.0; 4],
            columns: Default::default(),
            scale: None,
            scaled: false,
            grid: String::new(),
            locked: false,
            quantity: 1.0,
        }
    }
}

fn number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let value: f64 = text.parse().ok()?;
    (value > 0.0).then_some(value)
}

/// Measures one markup against a scale. `scale` is `None` on an unscaled sheet.
pub fn measure(
    page: usize,
    reference: Ref,
    markup: &Markup,
    scale: Option<&Measure>,
) -> Row {
    let points = markup.points();
    let kind = markup.kind();
    let area_box = geometry::bounds(&points).unwrap_or([0.0; 4]);

    let mut row = Row {
        page,
        reference,
        subject: markup.subject(),
        label: markup.text_of("Label"),
        author: markup.text_of("T"),
        comments: markup.text_of("Contents"),
        status: markup.text_of("BSIStatus"),
        date: markup.text_of("M"),
        created: markup.text_of("CreationDate"),
        colour: markup.colour(),
        kind,
        caption: markup.contents(),
        unit: String::new(),
        count: if kind.measures() { 1.0 } else { 0.0 },
        length: None,
        area: None,
        perimeter: None,
        volume: None,
        depth: None,
        wall_area: None,
        angle: None,
        radius: None,
        diameter: None,
        slope: None,
        pitch_run: 12.0,
        area_box,
        columns: read_columns(markup),
        scale: scale.cloned(),
        scaled: scale.is_some(),
        grid: String::new(),
        locked: markup
            .dict
            .get("BSILock")
            .and_then(|o| o.as_bool())
            .unwrap_or(false),
        quantity: quantity_of(markup),
    };

    let Some(scale) = scale else {
        return row;
    };
    row.unit = scale
        .distance
        .first()
        .or_else(|| scale.x.first())
        .map(|f| f.unit.clone())
        .unwrap_or_default();
    let per_point = scale.per_point();

    // A pitch is written as rise over a run of `PitchRun`, and turns the run on
    // the sheet into the true length up the slope.
    let pitch = pitch_of(markup);
    let slope = |flat: f64| up_the_slope(markup, flat);
    row.slope = pitch.map(|(rise, _)| rise);
    row.pitch_run = pitch.map(|(_, run)| run).unwrap_or(12.0);

    let depth = markup.dict.get("Depth").and_then(|o| o.as_f64());

    match kind {
        Kind::Length | Kind::Polylength => {
            // A perimeter is a length measured round a closed shape, and a
            // closed shape's last side is the one back to where it started.
            // Measuring it as an open run leaves a takeoff one side short on
            // every shape — quietly, and always low.
            let run = if markup.subtype() == Subtype::Polygon {
                geometry::perimeter(&points)
            } else {
                geometry::length(&points)
            } * per_point;
            row.length = Some(slope(run));
            if markup.subtype() == Subtype::Polygon {
                row.perimeter = Some(run);
            }
        }
        Kind::Area | Kind::Volume => {
            let square = geometry::area(&points) * per_point * per_point;
            let edge = geometry::perimeter(&points) * per_point;
            row.area = Some(square);
            row.perimeter = Some(edge);
            if let Some(depth) = depth {
                let deep = depth * per_point;
                row.depth = Some(deep);
                row.volume = Some(square * deep);
                // What a wall of that height around the shape would cover.
                row.wall_area = Some(edge * deep);
            }
        }
        Kind::Count => {
            row.count = markup
                .dict
                .get("NumCounts")
                .and_then(|o| o.as_f64())
                .unwrap_or(1.0)
                .max(1.0);
        }
        Kind::Diameter => {
            let radius = match markup.subtype() {
                Subtype::Circle | Subtype::Square => geometry::radius_of_box(area_box),
                _ => geometry::radius_through(&points).unwrap_or(0.0),
            } * per_point;
            row.radius = Some(radius);
            row.diameter = Some(radius * 2.0);
            row.length = Some(radius * 2.0);
        }
        Kind::Radius => {
            let radius = geometry::radius_through(&points)
                .unwrap_or_else(|| geometry::radius_of_box(area_box))
                * per_point;
            row.radius = Some(radius);
            row.diameter = Some(radius * 2.0);
            row.length = Some(radius);
        }
        Kind::Angle => {
            row.angle = Some(geometry::angle(&points));
            row.count = 1.0;
        }
        Kind::Markup => {
            row.count = 0.0;
        }
    }

    // A markup that carries no caption of its own gets one worked out from its
    // own geometry against this sheet's scale. Revu writes a caption into
    // every measurement it makes, but a markup that arrived from somewhere
    // else may not have one, and leaving the cell blank would look like a
    // measurement of nothing rather than a caption nobody wrote.
    if row.caption.trim().is_empty() {
        row.caption = caption_for(&row, scale);
    }
    row
}

/// Where a markup keeps how many of it there are. Hyperview's own key: Revu
/// leaves keys it does not know alone.
pub const QUANTITY_KEY: &str = "HVQuantity";

/// The pitch on a markup, as rise over run — Revu's own `SlopeRise` and
/// `PitchRun`, so a slope set in either program reads the same in both.
/// `None` when it is flat.
pub fn pitch_of(markup: &Markup) -> Option<(f64, f64)> {
    let rise = markup.dict.get("SlopeRise").and_then(|o| o.as_f64())?;
    let run = markup
        .dict
        .get("PitchRun")
        .and_then(|o| o.as_f64())
        .filter(|r| *r > 0.0)
        .unwrap_or(12.0);
    (rise.is_finite() && rise != 0.0).then_some((rise, run))
}

/// The true length of a run that the sheet shows flat.
pub fn up_the_slope(markup: &Markup, flat: f64) -> f64 {
    match pitch_of(markup) {
        Some((rise, run)) => geometry::along_slope(flat, rise, run),
        None => flat,
    }
}

/// Puts a pitch on a markup, in the keys Revu reads. A rise of nothing takes
/// the pitch off.
pub fn set_pitch(markup: &mut Markup, rise: f64, run: f64) {
    if !rise.is_finite() || rise == 0.0 {
        markup.dict.remove("SlopeRise");
        markup.dict.remove("PitchRun");
        markup.dict.remove("SlopeType");
    } else {
        let run = if run.is_finite() && run > 0.0 { run } else { 12.0 };
        markup.set("SlopeRise", pdf::Object::real(rise));
        markup.set("PitchRun", pdf::Object::real(run));
        // Revu's own word for "the pitch is rise over run".
        markup.set("SlopeType", pdf::Object::Int(0));
    }
}

/// How many of this markup there are: one, unless it says more.
pub fn quantity_of(markup: &Markup) -> f64 {
    markup
        .dict
        .get(QUANTITY_KEY)
        .and_then(|o| o.as_f64())
        .filter(|q| q.is_finite() && *q > 0.0)
        .unwrap_or(1.0)
}

/// Writes how many of this markup there are. One is the default, so setting
/// one takes the key away and the file stays exactly what Revu would write.
pub fn set_quantity(markup: &mut Markup, quantity: f64) {
    if !quantity.is_finite() || quantity <= 0.0 || (quantity - 1.0).abs() < 1e-9 {
        markup.dict.remove(QUANTITY_KEY);
    } else {
        markup.set(QUANTITY_KEY, pdf::Object::real(quantity));
    }
}

/// Writes one of the six custom columns, keeping the others as they are.
pub fn set_column(markup: &mut Markup, index: usize, value: &str) {
    if index >= 6 {
        return;
    }
    let mut columns = read_columns(markup);
    columns[index] = value.to_string();
    // As long as the last one written, the way Revu writes them.
    let keep = columns.iter().rposition(|c| !c.is_empty()).map(|i| i + 1).unwrap_or(0);
    markup.set(
        "BSIColumnData",
        pdf::Object::Array(columns[..keep.max(index + 1)].iter().map(|c| pdf::Object::text(c)).collect()),
    );
}

/// A quantity as it reads in a cell: `6`, or `2.5` when it is not whole.
pub fn quantity_text(quantity: f64) -> String {
    if (quantity - quantity.round()).abs() < 1e-9 {
        format!("{:.0}", quantity)
    } else {
        let text = format!("{quantity:.3}");
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// The words a measurement reads as, from the numbers just worked out.
fn caption_for(row: &Row, scale: &Measure) -> String {
    let per_point = scale.per_point();
    match row.kind {
        Kind::Length | Kind::Polylength | Kind::Diameter | Kind::Radius => match row.length {
            Some(length) => scale.length(length / per_point),
            None => String::new(),
        },
        Kind::Area => match row.area {
            Some(area) => scale.area(area / (per_point * per_point)),
            None => String::new(),
        },
        Kind::Volume => match row.volume {
            Some(volume) => scale.volume(volume / (per_point * per_point * per_point)),
            None => row
                .area
                .map(|a| scale.area(a / (per_point * per_point)))
                .unwrap_or_default(),
        },
        Kind::Angle => match row.angle {
            Some(angle) => scale.angle(angle),
            None => String::new(),
        },
        Kind::Count => format!("{:.0}", row.count),
        Kind::Markup => String::new(),
    }
}

fn read_columns(markup: &Markup) -> [String; 6] {
    let mut out: [String; 6] = Default::default();
    let Some(items) = markup
        .dict
        .get("BSIColumnData")
        .and_then(|o| o.as_array())
    else {
        return out;
    };
    for (i, item) in items.iter().take(6).enumerate() {
        if let Some(text) = item.as_text() {
            out[i] = text;
        }
    }
    out
}

/// Measures every markup in a document against each sheet's own scale.
pub fn read_document(doc: &Document) -> Vec<Row> {
    let mut rows = Vec::new();
    for page in 0..doc.page_count() {
        let Some(sheet) = doc.page(page) else { continue };
        for (reference, markup) in annot::place::read_page(doc, page) {
            // A markup's own scale is what it was measured with; the sheet's is
            // the fallback for one that carries none.
            let at = markup.points().first().copied().unwrap_or([0.0, 0.0]);
            let scale = markup
                .dict
                .get("Measure")
                .and_then(|o| o.as_dict())
                .and_then(Measure::read)
                .or_else(|| annot::viewport::scale_at(doc, &sheet, at));
            rows.push(measure(page, reference, &markup, scale.as_ref()));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use annot::measure::{feet_in, imperial, metric};

    fn quarter() -> Measure {
        imperial(48.0, "1/4\" = 1'-0\"", 16)
    }

    fn beam(points: &[[f64; 2]]) -> Markup {
        let mut m = Markup::new(Subtype::Line);
        m.set("IT", pdf::Object::name("LineDimension"));
        m.set_subject("W12x26");
        m.set(
            "BSIColumnData",
            pdf::Object::Array(vec![pdf::Object::text("26.00")]),
        );
        m.set_line(points[0], points[1]);
        m
    }

    fn plate(points: &[[f64; 2]]) -> Markup {
        let mut m = Markup::new(Subtype::Polygon);
        m.set("IT", pdf::Object::name("PolygonDimension"));
        m.set_subject("PL1/2");
        m.set(
            "BSIColumnData",
            pdf::Object::Array(vec![
                pdf::Object::text(""),
                pdf::Object::text(""),
                pdf::Object::text(""),
                pdf::Object::text(""),
                pdf::Object::text(""),
                pdf::Object::text("20.42"),
            ]),
        );
        m.set_vertices(points);
        m
    }

    #[test]
    fn a_beam_measures_in_feet_and_weighs_what_its_column_says() {
        let m = beam(&[[0.0, 0.0], [720.0, 0.0]]);
        let row = measure(0, Ref::new(1, 0), &m, Some(&quarter()));
        assert_eq!(row.kind, Kind::Length);
        assert!((row.length.unwrap() - 40.0).abs() < 1e-9);
        assert_eq!(row.unit, "'");
        assert_eq!(row.count, 1.0);
        assert!((row.pounds(Default::default()).unwrap() - 1040.0).abs() < 1e-9);
        assert!((row.tons(Default::default()).unwrap() - 0.52).abs() < 1e-9);
    }

    #[test]
    fn an_unscaled_sheet_reports_no_measurement_and_no_weight_rather_than_zero() {
        let m = beam(&[[0.0, 0.0], [720.0, 0.0]]);
        let row = measure(0, Ref::new(1, 0), &m, None);
        assert!(!row.scaled);
        assert_eq!(row.length, None);
        assert_eq!(row.pounds(Default::default()), None);
        assert_eq!(row.count, 1.0, "a pick is still a pick");
    }

    #[test]
    fn a_plate_weighs_by_area_not_by_length() {
        let m = plate(&[[0.0, 0.0], [720.0, 0.0], [720.0, 360.0], [0.0, 360.0]]);
        let row = measure(0, Ref::new(1, 0), &m, Some(&quarter()));
        assert_eq!(row.kind, Kind::Area);
        // 40 ft by 20 ft is 800 square feet.
        assert!((row.area.unwrap() - 800.0).abs() < 1e-6, "{:?}", row.area);
        assert!((row.perimeter.unwrap() - 120.0).abs() < 1e-6);
        assert_eq!(row.length, None);
        assert!((row.pounds(Default::default()).unwrap() - 800.0 * 20.42).abs() < 1e-6);
    }

    #[test]
    fn a_depth_turns_an_area_into_a_volume_and_a_wall() {
        let mut m = plate(&[[0.0, 0.0], [720.0, 0.0], [720.0, 360.0], [0.0, 360.0]]);
        // Six inches, given in sheet points at this scale.
        m.set("Depth", pdf::Object::real(0.5 / 4.0 * 72.0 / 12.0 * 12.0));
        let row = measure(0, Ref::new(1, 0), &m, Some(&quarter()));
        assert!(row.volume.is_some());
        assert!(row.wall_area.is_some());
        let depth = row.depth.unwrap();
        assert!((row.volume.unwrap() - 800.0 * depth).abs() < 1e-6);
        assert!((row.wall_area.unwrap() - 120.0 * depth).abs() < 1e-6);
    }

    #[test]
    fn a_sloped_run_is_longer_than_the_plan_shows() {
        let mut m = beam(&[[0.0, 0.0], [720.0, 0.0]]);
        m.set("SlopeRise", pdf::Object::real(4.0));
        m.set("PitchRun", pdf::Object::real(12.0));
        let row = measure(0, Ref::new(1, 0), &m, Some(&quarter()));
        let flat = 40.0;
        let expected = flat * (1.0 + (4.0f64 / 12.0).powi(2)).sqrt();
        assert!((row.length.unwrap() - expected).abs() < 1e-9);
        assert!(row.length.unwrap() > flat);
    }

    #[test]
    fn one_line_for_six_beams_counts_six_everywhere() {
        let mut m = beam(&[[0.0, 0.0], [720.0, 0.0]]);
        set_quantity(&mut m, 6.0);
        let row = measure(0, Ref::new(1, 0), &m, Some(&quarter()));
        assert_eq!(row.quantity, 6.0);
        // The length is one piece's; the weight is all six.
        assert!((row.length.unwrap() - 40.0).abs() < 1e-9);
        assert!((row.pounds(Default::default()).unwrap() - 6.0 * 1040.0).abs() < 1e-9);
        let summary = crate::summary::summarise(&[row.clone()], Default::default());
        let group = &summary.groups[0];
        assert!((group.length - 240.0).abs() < 1e-9, "{}", group.length);
        assert_eq!(group.count, 6.0);
        assert_eq!(crate::Column::Quantity.text(&row, Default::default()), "6");
        assert_eq!(crate::Column::Quantity.number(&row, Default::default()), Some(6.0));
    }

    #[test]
    fn a_pitch_set_here_reads_as_revu_writes_it_and_climbs_the_right_amount() {
        let mut m = beam(&[[0.0, 0.0], [216.0, 0.0]]); // twelve feet on the sheet
        set_pitch(&mut m, 4.0, 12.0);
        assert_eq!(pitch_of(&m), Some((4.0, 12.0)));
        let row = measure(0, Ref::new(1, 0), &m, Some(&quarter()));
        assert!((row.length.unwrap() - 12.649_110_640_673_518).abs() < 1e-9);
        assert_eq!(crate::Column::Slope.text(&row, Default::default()), "4 in 12");
        // Twelve feet of run at 4 in 12 climbs four feet — not 4.2, which is
        // what the sloped length times a third would say.
        let climb = crate::Column::RiseDrop.text(&row, Default::default());
        assert!(climb.starts_with("4'"), "{climb}");
        set_pitch(&mut m, 0.0, 12.0);
        assert_eq!(pitch_of(&m), None);
        assert!(!m.dict.has("SlopeRise") && !m.dict.has("PitchRun"));
    }

    #[test]
    fn a_quantity_of_one_leaves_nothing_behind_in_the_file() {
        let mut m = beam(&[[0.0, 0.0], [720.0, 0.0]]);
        set_quantity(&mut m, 4.0);
        assert!(m.dict.has(QUANTITY_KEY));
        set_quantity(&mut m, 1.0);
        assert!(!m.dict.has(QUANTITY_KEY), "one is the default and is never written");
        set_quantity(&mut m, -3.0);
        assert!(!m.dict.has(QUANTITY_KEY), "a nonsense quantity is no quantity");
        m.set(QUANTITY_KEY, pdf::Object::real(0.0));
        assert_eq!(quantity_of(&m), 1.0, "zero written by something else still counts one");
        assert_eq!(quantity_text(2.5), "2.5");
        assert_eq!(quantity_text(12.0), "12");
    }

    #[test]
    fn a_count_markup_counts_and_measures_nothing_else() {
        let mut m = Markup::new(Subtype::Polygon);
        m.set("IT", pdf::Object::name("PolygonCount"));
        m.set_subject("Shear Conn");
        m.set_vertices(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
        let row = measure(0, Ref::new(1, 0), &m, Some(&quarter()));
        assert_eq!(row.kind, Kind::Count);
        assert_eq!(row.count, 1.0);
        assert_eq!(row.length, None);
        assert_eq!(row.area, None);
    }

    #[test]
    fn a_weight_of_zero_in_the_column_is_not_a_weight() {
        let mut m = beam(&[[0.0, 0.0], [720.0, 0.0]]);
        m.set(
            "BSIColumnData",
            pdf::Object::Array(vec![pdf::Object::text("0.00")]),
        );
        let row = measure(0, Ref::new(1, 0), &m, Some(&quarter()));
        assert_eq!(row.pounds(Default::default()), None);
    }

    #[test]
    fn a_plain_markup_is_not_counted_as_a_pick() {
        let mut m = Markup::new(Subtype::Square);
        m.set_subject("cloud");
        m.set_box([0.0, 0.0, 10.0, 10.0]);
        let row = measure(0, Ref::new(1, 0), &m, Some(&quarter()));
        assert_eq!(row.kind, Kind::Markup);
        assert_eq!(row.count, 0.0);
    }

    // WHAT: a forty-foot beam on an imperial sheet is 40 ft; the same drawn
    // length on a 1:50 metric sheet is sent in feet, not millimetres. WHY: the
    // field says feet, and a program reading it takes it as feet.
    #[test]
    fn lengths_go_out_in_feet_whatever_the_sheet_was_scaled_in() {
        // WHAT: a straight line 720 points long. WHY: forty feet at 1/4" = 1'-0".
        let line = beam(&[[0.0, 0.0], [720.0, 0.0]]);
        // WHAT: measured at 1/4" = 1'-0". WHY: forty feet.
        let imperial_row = measure(0, pdf::Ref::new(0, 0), &line, Some(&quarter()));
        // WHAT: 40 ft. WHY: feet already.
        assert!((imperial_row.length_ft().unwrap() - 40.0).abs() < 1e-9);
        // WHAT: its unit said, the foot mark. WHY: sent beside it.
        assert_eq!(imperial_row.measured_unit(), "'");
        // WHAT: the same line at 1:50. WHY: measured in millimetres.
        let metric_row = measure(0, pdf::Ref::new(0, 0), &line, Some(&metric(50.0, "1:50")));
        // WHAT: the length in the sheet's unit. WHY: 12,700 mm.
        let millimetres = metric_row.length.unwrap();
        // WHAT: 720/72 in × 50 × 25.4. WHY: as the metric scale reads.
        assert!((millimetres - 12_700.0).abs() < 1e-6, "{millimetres}");
        // WHAT: sent as 41.67 ft. WHY: never 12,700 "feet".
        assert!((metric_row.length_ft().unwrap() - 12_700.0 / 304.8).abs() < 1e-9);
        // WHAT: its unit said. WHY: a reader sees it was converted.
        assert_eq!(metric_row.measured_unit(), "mm");
        // WHAT: inches and yards, any capitals and spaces. WHY: as a scale may write them.
        assert_eq!(feet_in("IN"), Some(1.0 / 12.0));
        // WHAT: yards, with spaces round it. WHY: the same.
        assert_eq!(feet_in(" yd "), Some(3.0));
        // WHAT: centimetres. WHY: 30.48 to the foot.
        assert!((feet_in("cm").unwrap() * 30.48 - 1.0).abs() < 1e-12);
        // WHAT: a unit not known. WHY: never taken as feet.
        assert_eq!(feet_in("furlong"), None);
        // WHAT: a scale in a unit not known. WHY: its numbers are not sent.
        let mut odd = metric(50.0, "1:50");
        // WHAT: renamed. WHY: as above.
        odd.x[0].unit = "furlong".into();
        // WHAT: measured on it. WHY: as above.
        let odd_row = measure(0, pdf::Ref::new(0, 0), &line, Some(&odd));
        // WHAT: none sent. WHY: a wrong number is worse than none.
        assert_eq!(odd_row.length_ft(), None);
    }
}
