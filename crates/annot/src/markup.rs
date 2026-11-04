//! A markup is a PDF annotation dictionary with a little help.
//!
//! Hyperview deliberately does **not** invent its own markup format and convert at
//! the edges. A tool from the Tool Chest arrives as an annotation dictionary;
//! placing it fills in the geometry and leaves every other key exactly as it
//! was. That is why a markup made here opens in Revu with its measurement, its
//! subject, its slope and its column data intact — none of it was ever
//! translated into something else and back.

use pdf::{Dict, Name, Object, Stream};

use crate::appearance;

/// The annotation subtypes Hyperview draws.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Subtype {
    Line,
    PolyLine,
    Polygon,
    Square,
    Circle,
    Ink,
    FreeText,
    Text,
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
    Stamp,
    Link,
    Other,
}

impl Subtype {
    pub fn read(dict: &Dict) -> Subtype {
        match dict
            .get("Subtype")
            .and_then(|o| o.as_name())
            .map(|n| n.as_str().to_string())
            .unwrap_or_default()
            .as_str()
        {
            "Line" => Subtype::Line,
            "PolyLine" => Subtype::PolyLine,
            "Polygon" => Subtype::Polygon,
            "Square" => Subtype::Square,
            "Circle" => Subtype::Circle,
            "Ink" => Subtype::Ink,
            "FreeText" => Subtype::FreeText,
            "Text" => Subtype::Text,
            "Highlight" => Subtype::Highlight,
            "Underline" => Subtype::Underline,
            "StrikeOut" => Subtype::StrikeOut,
            "Squiggly" => Subtype::Squiggly,
            "Stamp" => Subtype::Stamp,
            "Link" => Subtype::Link,
            _ => Subtype::Other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Subtype::Line => "Line",
            Subtype::PolyLine => "PolyLine",
            Subtype::Polygon => "Polygon",
            Subtype::Square => "Square",
            Subtype::Circle => "Circle",
            Subtype::Ink => "Ink",
            Subtype::FreeText => "FreeText",
            Subtype::Text => "Text",
            Subtype::Highlight => "Highlight",
            Subtype::Underline => "Underline",
            Subtype::StrikeOut => "StrikeOut",
            Subtype::Squiggly => "Squiggly",
            Subtype::Stamp => "Stamp",
            Subtype::Link => "Link",
            Subtype::Other => "Square",
        }
    }
}

/// What a markup started from a tool never takes from it: an action (`A`),
/// actions on events (`AA`) and an attached file (`FS`). A PDF reader runs
/// the first two — a script, a program, a web address — and the third
/// carries any file at all.
pub const TOOL_NEVER_CARRIES: [&str; 3] = ["A", "AA", "FS"]; // WHAT: the three keys. WHY: no measuring tool needs any of them.

/// What a markup measures, if anything.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Length,
    Polylength,
    Area,
    Volume,
    Count,
    Diameter,
    Radius,
    Angle,
    /// A drawing markup that measures nothing.
    Markup,
}

impl Kind {
    pub fn measures(self) -> bool {
        !matches!(self, Kind::Markup)
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Length => "Length",
            Kind::Polylength => "Polylength",
            Kind::Area => "Area",
            Kind::Volume => "Volume",
            Kind::Count => "Count",
            Kind::Diameter => "Diameter",
            Kind::Radius => "Radius",
            Kind::Angle => "Angle",
            Kind::Markup => "Markup",
        }
    }

    /// Reads Bluebeam's `/IT` intent, falling back to the `/MeasurementTypes`
    /// bit field when a markup carries no intent.
    pub fn read(annotation: &Dict) -> Kind {
        if let Some(intent) = annotation.get("IT").and_then(|o| o.as_name()) {
            match intent.as_str() {
                "LineDimension" | "PolyLineDimension" => return Kind::Length,
                "Polylength" => return Kind::Polylength,
                "PolygonDimension" => return Kind::Area,
                "PolygonVolume" => return Kind::Volume,
                "PolygonCount" => return Kind::Count,
                "CircleDimension" => return Kind::Diameter,
                "PolygonRadius" => return Kind::Radius,
                "PolyLineAngle" => return Kind::Angle,
                _ => {}
            }
        }
        let bits = annotation
            .get("MeasurementTypes")
            .and_then(|o| o.as_i64())
            .unwrap_or(0);
        // Confirmed against 1,572 of his tools: 1 area, 2 length, 4 volume,
        // 128 count, 256 diameter, 1024 angle, 2048 radius.
        if bits & 2048 != 0 {
            Kind::Radius
        } else if bits & 1024 != 0 {
            Kind::Angle
        } else if bits & 256 != 0 {
            Kind::Diameter
        } else if bits & 4 != 0 {
            Kind::Volume
        } else if bits & 1 != 0 {
            Kind::Area
        } else if bits & 2 != 0 {
            Kind::Length
        } else if bits & 128 != 0 {
            Kind::Count
        } else {
            Kind::Markup
        }
    }
}

#[derive(Clone, Debug)]
pub struct Markup {
    pub dict: Dict,
    /// A picture this markup shows, for a stamp or an image placed on the
    /// sheet. Held beside the dictionary rather than in it because it is not
    /// an entry in the annotation — it goes into the appearance stream, as its
    /// own object, when the markup is drawn.
    pub picture: Option<Picture>,
}

/// A picture ready to go into a PDF: already in the form the file wants, so
/// nothing has to be decoded and re-encoded on the way in.
#[derive(Clone)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// The image data exactly as it will be written.
    pub data: Vec<u8>,
    /// The filter it is written under: `DCTDecode` for a JPEG kept whole,
    /// `FlateDecode` for anything else.
    pub filter: &'static str,
    /// 8-bit RGB unless the picture is grey.
    pub grey: bool,
    /// Transparency, as its own greyscale image, when the picture has any.
    pub mask: Option<Vec<u8>>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "picture {}x{} ({} bytes)", self.width, self.height, self.data.len())
    }
}

impl Markup {
    pub fn new(subtype: Subtype) -> Markup {
        let mut dict = Dict::new();
        dict.set(Name::new("Type"), Object::name("Annot"));
        dict.set(Name::new("Subtype"), Object::name(subtype.as_str()));
        // Print, and do not scale or rotate with the view.
        dict.set(Name::new("F"), Object::Int(4));
        Markup { dict, picture: None }
    }

    /// The same markup, showing a picture.
    pub fn showing(mut self, picture: Picture) -> Markup {
        self.picture = Some(picture);
        self
    }

    /// Starts from a Tool Chest tool, keeping everything it carries except
    /// what runs something or carries a file (`TOOL_NEVER_CARRIES`).
    pub fn from_tool(tool: &Dict) -> Markup {
        let mut dict = tool.clone();
        dict.set(Name::new("Type"), Object::name("Annot"));
        if !dict.has("F") {
            dict.set(Name::new("F"), Object::Int(4));
        }
        // Geometry is about to be replaced by where the user actually clicked.
        for key in ["L", "Vertices", "InkList", "QuadPoints", "AP", "Rect"] {
            dict.remove(key);
        }
        for key in TOOL_NEVER_CARRIES { // WHAT: each key that runs something or carries a file. WHY: a tool chest is a file anybody can send; what it puts in a markup goes into every drawing saved with it and opened elsewhere.
            dict.remove(key); // WHAT: taken out. WHY: the same.
        }
        Markup { dict, picture: None }
    }

    pub fn subtype(&self) -> Subtype {
        Subtype::read(&self.dict)
    }

    pub fn kind(&self) -> Kind {
        Kind::read(&self.dict)
    }

    pub fn set(&mut self, key: &str, value: Object) -> &mut Markup {
        self.dict.set(Name::new(key), value);
        self
    }

    pub fn text_of(&self, key: &str) -> String {
        self.dict.get(key).and_then(|o| o.as_text()).unwrap_or_default()
    }

    /// The name that identifies this markup wherever it goes: in the file, on
    /// a server, on somebody else's screen.
    pub fn name(&self) -> String {
        self.text_of("NM")
    }

    pub fn set_name(&mut self, name: &str) -> &mut Markup {
        self.set("NM", Object::text(name));
        self
    }

    /// Gives it a name if it has none, and says what it is called.
    pub fn named(&mut self) -> String {
        let have = self.name();
        if !have.is_empty() {
            return have;
        }
        let fresh = crate::name::fresh();
        self.set_name(&fresh);
        fresh
    }

    pub fn subject(&self) -> String {
        self.text_of("Subj")
    }

    pub fn contents(&self) -> String {
        self.text_of("Contents")
    }

    pub fn set_subject(&mut self, text: &str) -> &mut Markup {
        self.set("Subj", Object::text(text))
    }

    /// Sets the caption. Both keys are written: `/Contents` for every reader,
    /// and `/RC` in the rich form Revu reads back into its own editor.
    pub fn set_contents(&mut self, text: &str) -> &mut Markup {
        self.set("Contents", Object::text(text));
        let colour = self.colour();
        let size = self.font_size();
        let style = format!(
            "font:Helvetica {}pt; text-align:center; color:#{:02X}{:02X}{:02X}",
            crate::number_text(size),
            (colour[0] * 255.0).round() as u8,
            (colour[1] * 255.0).round() as u8,
            (colour[2] * 255.0).round() as u8
        );
        let rich = format!(
            "<?xml version=\"1.0\"?><body xmlns=\"http://www.w3.org/1999/xhtml\" \
             xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\" \
             xfa:contentType=\"text/html\" xfa:spec=\"2.2.0\" style=\"{style}\"><p>{}</p></body>",
            escape_xml(text)
        );
        self.set("RC", Object::text(&rich));
        self
    }

    pub fn set_author(&mut self, who: &str) -> &mut Markup {
        self.set("T", Object::text(who))
    }

    pub fn colour(&self) -> [f32; 3] {
        colour_of(self.dict.get("C")).unwrap_or([0.0, 0.0, 0.0])
    }

    pub fn set_colour(&mut self, c: [f32; 3]) -> &mut Markup {
        let value = Object::Array(vec![
            Object::real(c[0] as f64),
            Object::real(c[1] as f64),
            Object::real(c[2] as f64),
        ]);
        self.dict.set(Name::new("C"), value);
        self
    }

    pub fn interior(&self) -> Option<[f32; 3]> {
        colour_of(self.dict.get("IC"))
    }

    pub fn width(&self) -> f64 {
        self.dict
            .get("BS")
            .and_then(|o| o.as_dict())
            .and_then(|d| d.get("W"))
            .and_then(|o| o.as_f64())
            .unwrap_or(1.0)
            .max(0.0)
    }

    pub fn set_width(&mut self, width: f64) -> &mut Markup {
        let mut bs = self
            .dict
            .get("BS")
            .and_then(|o| o.as_dict())
            .cloned()
            .unwrap_or_default();
        bs.set(Name::new("Type"), Object::name("Border"));
        bs.set(Name::new("W"), Object::real(width));
        if !bs.has("S") {
            bs.set(Name::new("S"), Object::name("S"));
        }
        self.dict.set(Name::new("BS"), Object::Dict(bs));
        self
    }

    pub fn opacity(&self) -> f32 {
        self.dict
            .get("CA")
            .and_then(|o| o.as_f64())
            .unwrap_or(1.0)
            .clamp(0.0, 1.0) as f32
    }

    pub fn fill_opacity(&self) -> f32 {
        self.dict
            .get("FillOpacity")
            .and_then(|o| o.as_f64())
            .map(|v| v.clamp(0.0, 1.0) as f32)
            .unwrap_or_else(|| self.opacity())
    }

    /// Whether this markup's look can only be kept, not drawn again: a stamp
    /// read from a file, whose picture — a signature, a rubber stamp made in
    /// another program, a photograph — is in its appearance and nowhere
    /// else. Moving or resizing one changes its rectangle, and every reader
    /// fits the appearance to the rectangle, so the picture goes with it.
    pub fn keeps_its_appearance(&self) -> bool {
        self.picture.is_none() && self.subtype() == Subtype::Stamp && self.dict.has("AP")
    }

    /// Whether the markup is blended onto the sheet by multiplying, so the
    /// drawing under a fill still shows through it the way it does under a
    /// highlighter. `/BM` in the annotation, which is where PDF 2.0 put it
    /// and where Revu has always written it.
    pub fn multiplies(&self) -> bool {
        self.dict
            .get("BM")
            .and_then(|o| o.as_name())
            .is_some_and(|n| n.as_str() == "Multiply")
    }

    pub fn set_multiply(&mut self, on: bool) -> &mut Markup {
        if on {
            self.dict.set(Name::new("BM"), Object::name("Multiply"));
        } else {
            self.dict.remove("BM");
        }
        self
    }

    /// Reads the point size out of Bluebeam's `/DS` style string.
    pub fn font_size(&self) -> f64 {
        let ds = self.text_of("DS");
        for part in ds.split(';') {
            let part = part.trim();
            let Some(rest) = part.strip_prefix("font:").or_else(|| part.strip_prefix("font ")) else {
                continue;
            };
            for word in rest.split_whitespace() {
                if let Some(size) = word.strip_suffix("pt") {
                    if let Ok(v) = size.parse::<f64>() {
                        return v.clamp(4.0, 144.0);
                    }
                }
            }
        }
        12.0
    }

    /// True when the caption sits along the measurement rather than in a box.
    pub fn along_segment(&self) -> bool {
        self.dict
            .get("AlignOnSegment")
            .and_then(|o| o.as_bool())
            .unwrap_or(false)
    }

    // ---- geometry -------------------------------------------------------

    pub fn set_line(&mut self, a: [f64; 2], b: [f64; 2]) -> &mut Markup {
        self.dict.set(
            Name::new("L"),
            Object::Array(vec![
                Object::real(a[0]),
                Object::real(a[1]),
                Object::real(b[0]),
                Object::real(b[1]),
            ]),
        );
        self
    }

    pub fn set_vertices(&mut self, points: &[[f64; 2]]) -> &mut Markup {
        let mut flat = Vec::with_capacity(points.len() * 2);
        for p in points {
            flat.push(Object::real(p[0]));
            flat.push(Object::real(p[1]));
        }
        self.dict.set(Name::new("Vertices"), Object::Array(flat));
        self
    }

    pub fn set_ink(&mut self, strokes: &[Vec<[f64; 2]>]) -> &mut Markup {
        let list = strokes
            .iter()
            .map(|stroke| {
                let mut flat = Vec::with_capacity(stroke.len() * 2);
                for p in stroke {
                    flat.push(Object::real(p[0]));
                    flat.push(Object::real(p[1]));
                }
                Object::Array(flat)
            })
            .collect();
        self.dict.set(Name::new("InkList"), Object::Array(list));
        self
    }

    /// Sets the box the markup is drawn in: its shape, for a box, an ellipse,
    /// a text box or a stamp. Any allowance an earlier drawing added round it
    /// goes, because it was an allowance for the old shape.
    pub fn set_box(&mut self, rect: [f64; 4]) -> &mut Markup {
        self.dict.set(
            Name::new("Rect"),
            Object::Array(rect.iter().map(|v| Object::real(*v)).collect()),
        );
        self.dict.remove("RD");
        self
    }

    /// The box the shape is drawn in, for the markups whose shape is a box.
    ///
    /// `/Rect` is the area a reader may draw in, which has to be bigger than
    /// the shape: half the line sticks out, a cloud's scallops stick out
    /// further, and a callout's leader can be anywhere. `/RD` says by how much
    /// on each side. Reading the shape as `/Rect` alone made every box a
    /// little bigger each time it was drawn again, and a cloud a lot bigger.
    pub fn drawn_box(&self) -> Option<[f64; 4]> {
        let r = self.dict.get("Rect").and_then(|o| o.as_rect())?;
        let d = self
            .dict
            .get("RD")
            .map(|o| o.numbers())
            .filter(|v| v.len() == 4 && v.iter().all(|x| x.is_finite() && *x >= 0.0))
            .unwrap_or_else(|| vec![0.0; 4]);
        let inner = [r[0] + d[0], r[1] + d[1], r[2] - d[2], r[3] - d[3]];
        if inner[2] > inner[0] && inner[3] > inner[1] {
            Some(inner)
        } else {
            Some(r)
        }
    }

    /// Sets the drawing area round a shape: `/Rect` to `outer`, and `/RD` to
    /// how far it reaches past `inner` on each side, so the shape can be read
    /// back exactly.
    pub fn set_drawing_area(&mut self, outer: [f64; 4], inner: [f64; 4]) -> &mut Markup {
        self.dict.set(
            Name::new("Rect"),
            Object::Array(outer.iter().map(|v| Object::real(*v)).collect()),
        );
        let d = [
            (inner[0] - outer[0]).max(0.0),
            (inner[1] - outer[1]).max(0.0),
            (outer[2] - inner[2]).max(0.0),
            (outer[3] - inner[3]).max(0.0),
        ];
        if d.iter().all(|v| *v == 0.0) {
            self.dict.remove("RD");
        } else {
            self.dict.set(
                Name::new("RD"),
                Object::Array(d.iter().map(|v| Object::real(*v)).collect()),
            );
        }
        self
    }


    /// Moves every point of a markup by a distance, wherever those points live.
    ///
    /// A markup keeps its geometry in a different key depending on what it is —
    /// `/L` for a line, `/InkList` for freehand, `/Rect` for a box, `/Vertices`
    /// for everything else — and every one of them has to move together or the
    /// markup comes apart. The caption and the measurement are untouched: a
    /// measured length that moves is the same length.
    pub fn move_by(&mut self, dx: f64, dy: f64) -> &mut Markup {
        use crate::Subtype;
        match self.subtype() {
            Subtype::Line => {
                let moved: Vec<f64> = self
                    .dict
                    .get("L")
                    .map(|o| o.numbers())
                    .unwrap_or_default()
                    .chunks(2)
                    .filter(|c| c.len() == 2)
                    .flat_map(|c| [c[0] + dx, c[1] + dy])
                    .collect();
                if !moved.is_empty() {
                    self.dict.set(
                        Name::new("L"),
                        Object::Array(moved.iter().map(|v| Object::real(*v)).collect()),
                    );
                }
            }
            Subtype::Ink => {
                let moved: Vec<Vec<[f64; 2]>> = self
                    .ink()
                    .into_iter()
                    .map(|stroke| {
                        stroke.into_iter().map(|p| [p[0] + dx, p[1] + dy]).collect()
                    })
                    .collect();
                if !moved.is_empty() {
                    self.set_ink(&moved);
                }
            }
            _ => {
                let moved: Vec<[f64; 2]> = self
                    .dict
                    .get("Vertices")
                    .map(|o| o.numbers())
                    .unwrap_or_default()
                    .chunks(2)
                    .filter(|c| c.len() == 2)
                    .map(|c| [c[0] + dx, c[1] + dy])
                    .collect();
                if !moved.is_empty() {
                    self.set_vertices(&moved);
                }
            }
        }
        // A callout's leader and a text markup's word boxes are points too.
        for key in ["CL", "QuadPoints"] {
            let moved: Vec<f64> = self
                .dict
                .get(key)
                .map(|o| o.numbers())
                .unwrap_or_default()
                .chunks(2)
                .filter(|c| c.len() == 2)
                .flat_map(|c| [c[0] + dx, c[1] + dy])
                .collect();
            if !moved.is_empty() {
                self.dict.set(
                    Name::new(key),
                    Object::Array(moved.iter().map(|v| Object::real(*v)).collect()),
                );
            }
        }
        // The box moves whatever the shape, because every markup has one and a
        // reader that trusts it will draw the markup in the wrong place if it
        // is left behind. `/RD` is a difference, so it moves with it as it is.
        if let Some(rect) = self.dict.get("Rect").and_then(|o| o.as_rect()) {
            let moved = [rect[0] + dx, rect[1] + dy, rect[2] + dx, rect[3] + dy];
            self.dict.set(
                Name::new("Rect"),
                Object::Array(moved.iter().map(|v| Object::real(*v)).collect()),
            );
        }
        // The drawn appearance is built from the geometry, so the old one is
        // now wrong. Dropping it makes it be rebuilt rather than showing the
        // markup at its old place — unless it is a picture that can't be
        // drawn again, which the new rectangle carries with it.
        if !self.keeps_its_appearance() {
            self.dict.remove("AP");
        }
        self
    }

    /// Every point this markup is drawn from, whatever shape it is.
    pub fn points(&self) -> Vec<[f64; 2]> {
        match self.subtype() {
            Subtype::Line => {
                let l = self.dict.get("L").map(|o| o.numbers()).unwrap_or_default();
                l.chunks(2).filter(|c| c.len() == 2).map(|c| [c[0], c[1]]).collect()
            }
            Subtype::Ink => self
                .ink()
                .into_iter()
                .flatten()
                .collect(),
            // A marked run of words is where its word boxes are. The
            // rectangle round them is only the area a reader may draw in.
            Subtype::Highlight | Subtype::Underline | Subtype::StrikeOut | Subtype::Squiggly
                if self.quad_bounds().is_some() =>
            {
                let r = self.quad_bounds().unwrap_or_default();
                vec![[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]]
            }
            // Everything whose shape is its rectangle: a box, an ellipse, a
            // stamp, a text box, a folded note, a link, a marked run of words.
            Subtype::Square
            | Subtype::Circle
            | Subtype::Stamp
            | Subtype::FreeText
            | Subtype::Text
            | Subtype::Link
            | Subtype::Highlight
            | Subtype::Underline
            | Subtype::StrikeOut
            | Subtype::Squiggly => {
                match self.drawn_box() {
                    Some(r) => vec![[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]],
                    None => Vec::new(),
                }
            }
            _ => {
                let v = self
                    .dict
                    .get("Vertices")
                    .map(|o| o.numbers())
                    .unwrap_or_default();
                if v.len() >= 4 {
                    return v
                        .chunks(2)
                        .filter(|c| c.len() == 2)
                        .map(|c| [c[0], c[1]])
                        .collect();
                }
                // Anything whose shape is only its rectangle: a form field, a
                // file attachment, whatever else a file carries that this
                // program did not draw. Without this they have no geometry at
                // all, so no appearance gets built and nothing is visible.
                match self.dict.get("Rect").and_then(|o| o.as_rect()) {
                    Some(r) => vec![[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]],
                    None => Vec::new(),
                }
            }
        }
    }

    /// The box round a text markup's word boxes, when it has any.
    fn quad_bounds(&self) -> Option<[f64; 4]> {
        let q = self.dict.get("QuadPoints").map(|o| o.numbers()).unwrap_or_default();
        if q.len() < 8 {
            return None;
        }
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for p in q.chunks(2).filter(|c| c.len() == 2) {
            b[0] = b[0].min(p[0]);
            b[1] = b[1].min(p[1]);
            b[2] = b[2].max(p[0]);
            b[3] = b[3].max(p[1]);
        }
        (b[2] > b[0] && b[3] > b[1]).then_some(b)
    }

    pub fn ink(&self) -> Vec<Vec<[f64; 2]>> {
        self.dict
            .get("InkList")
            .and_then(|o| o.as_array())
            .map(|list| {
                list.iter()
                    .map(|stroke| {
                        stroke
                            .numbers()
                            .chunks(2)
                            .filter(|c| c.len() == 2)
                            .map(|c| [c[0], c[1]])
                            .collect()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The smallest box holding the geometry, before any allowance for line
    /// width or a caption.
    pub fn bounds(&self) -> Option<[f64; 4]> {
        let points = self.points();
        let first = points.first()?;
        let mut b = [first[0], first[1], first[0], first[1]];
        for p in &points {
            b[0] = b[0].min(p[0]);
            b[1] = b[1].min(p[1]);
            b[2] = b[2].max(p[0]);
            b[3] = b[3].max(p[1]);
        }
        Some(b)
    }

    /// Fills in `/Rect` and `/AP` so the markup is visible in every reader.
    /// Returns the appearance stream and the resources it needs.
    pub fn finish(&mut self) -> Option<appearance::Appearance> {
        appearance::build(self)
    }

    /// The same, saying how much to draw. See [`appearance::Draw`].
    pub fn finish_how(&mut self, how: appearance::Draw) -> Option<appearance::Appearance> {
        appearance::build_with(self, how)
    }
}

fn colour_of(value: Option<&Object>) -> Option<[f32; 3]> {
    let values = value?.numbers();
    Some(match values.len() {
        1 => [values[0] as f32; 3],
        3 => [values[0] as f32, values[1] as f32, values[2] as f32],
        4 => {
            let k = values[3] as f32;
            [
                (1.0 - values[0] as f32) * (1.0 - k),
                (1.0 - values[1] as f32) * (1.0 - k),
                (1.0 - values[2] as f32) * (1.0 - k),
            ]
        }
        _ => return None,
    })
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The appearance stream plus the objects it refers to.
pub struct Written {
    pub stream: Stream,
    pub resources: Dict,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf::Reader;

    fn tool(text: &str) -> Dict {
        Reader::new(text.as_bytes())
            .object()
            .unwrap()
            .as_dict()
            .unwrap()
            .clone()
    }

    #[test]
    fn intent_decides_the_kind() {
        assert_eq!(Kind::read(&tool("<</IT/LineDimension>>")), Kind::Length);
        assert_eq!(Kind::read(&tool("<</IT/PolygonCount>>")), Kind::Count);
        assert_eq!(Kind::read(&tool("<</IT/PolygonVolume>>")), Kind::Volume);
        assert_eq!(Kind::read(&tool("<</IT/CircleDimension>>")), Kind::Diameter);
    }

    #[test]
    fn the_bit_field_answers_when_there_is_no_intent() {
        assert_eq!(Kind::read(&tool("<</MeasurementTypes 130>>")), Kind::Length);
        assert_eq!(Kind::read(&tool("<</MeasurementTypes 129>>")), Kind::Area);
        assert_eq!(Kind::read(&tool("<</MeasurementTypes 128>>")), Kind::Count);
        assert_eq!(Kind::read(&tool("<</MeasurementTypes 132>>")), Kind::Volume);
        assert_eq!(Kind::read(&tool("<</MeasurementTypes 384>>")), Kind::Diameter);
        assert_eq!(Kind::read(&tool("<</MeasurementTypes 1152>>")), Kind::Angle);
        assert_eq!(Kind::read(&tool("<</MeasurementTypes 2177>>")), Kind::Radius);
        assert_eq!(Kind::read(&tool("<</Subtype/Square>>")), Kind::Markup);
    }

    #[test]
    fn a_tool_keeps_everything_but_its_sample_geometry() {
        let t = tool(
            "<</Subtype/Line/IT/LineDimension/Subj(W12x26)/MeasurementTypes 130\
              /SlopeType 0/PitchRun 12/BSIColumnData[(26.00)]/L[1 2 3 4]/Rect[0 0 9 9]>>",
        );
        let m = Markup::from_tool(&t);
        assert_eq!(m.subject(), "W12x26");
        assert!(m.dict.has("SlopeType"), "the slope setting has to survive");
        assert!(m.dict.has("PitchRun"));
        assert!(m.dict.has("BSIColumnData"), "and so does the weight");
        assert!(!m.dict.has("L"), "but the sample geometry goes");
        assert!(!m.dict.has("Rect"));
    }

    #[test]
    fn a_tool_cannot_put_an_action_or_a_file_into_the_markups_made_with_it() {
        let t = tool(
            "<</Subtype/Link/IT/LineDimension/Subj(W12x26)/A<</S/JavaScript/JS(app.alert(1))>>\
              /AA<</O<</S/Launch/F(cmd.exe)>>>>/FS<</Type/Filespec/F(x.exe)>>/BSIColumnData[(26.00)]>>",
        ); // WHAT: a tool template carrying a script, an on-open launch and an attached file. WHY: what a hostile tool chest would put in every markup drawn with it.
        let m = Markup::from_tool(&t); // WHAT: a markup started from it. WHY: what drawing with the tool does.
        for key in ["A", "AA", "FS"] { // WHAT: each key that runs or carries something. WHY: none may reach a drawing that is saved and sent on.
            assert!(!m.dict.has(key), "{key} must not survive"); // WHAT: gone. WHY: the same.
        }
        assert_eq!(m.subject(), "W12x26"); // WHAT: the rest of the tool kept. WHY: only what runs or carries a file goes.
        assert!(m.dict.has("BSIColumnData")); // WHAT: the weight kept. WHY: the same.
    }

    #[test]
    fn geometry_reads_back_as_the_points_that_were_put_in() {
        let mut m = Markup::new(Subtype::Line);
        m.set_line([10.0, 20.0], [110.0, 20.0]);
        assert_eq!(m.points(), vec![[10.0, 20.0], [110.0, 20.0]]);
        assert_eq!(m.bounds(), Some([10.0, 20.0, 110.0, 20.0]));

        let mut p = Markup::new(Subtype::Polygon);
        p.set_vertices(&[[0.0, 0.0], [10.0, 0.0], [10.0, 5.0]]);
        assert_eq!(p.bounds(), Some([0.0, 0.0, 10.0, 5.0]));
    }

    #[test]
    fn a_colour_is_read_whether_it_was_written_grey_rgb_or_cmyk() {
        assert_eq!(colour_of(Some(&tool("<</C[0.5]>>").get("C").unwrap().clone())), Some([0.5; 3]));
        let cmyk = colour_of(Some(&tool("<</C[0 1 1 0]>>").get("C").unwrap().clone())).unwrap();
        assert!((cmyk[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_font_size_comes_out_of_bluebeams_style_string() {
        let mut m = Markup::new(Subtype::Line);
        assert_eq!(m.font_size(), 12.0, "a sensible default");
        m.set(
            "DS",
            Object::text("font: Helvetica 11pt; text-align:center; color:#FF0000"),
        );
        assert_eq!(m.font_size(), 11.0);
    }

    #[test]
    fn a_caption_is_written_in_both_the_plain_and_the_rich_form() {
        let mut m = Markup::new(Subtype::Line);
        m.set_colour([1.0, 0.0, 0.0]).set_contents("20'-6 1/2\"");
        assert_eq!(m.contents(), "20'-6 1/2\"");
        let rich = m.text_of("RC");
        assert!(rich.contains("<p>20'-6 1/2\"</p>"), "{rich}");
        assert!(rich.contains("color:#FF0000"), "{rich}");
    }

    #[test]
    fn a_caption_with_an_ampersand_cannot_break_the_rich_text() {
        let mut m = Markup::new(Subtype::Polygon);
        m.set_contents("Legends & Schedules");
        assert!(m.text_of("RC").contains("Legends &amp; Schedules"));
    }

    #[test]
    fn the_border_width_survives_a_round_trip() {
        let mut m = Markup::new(Subtype::Line);
        assert_eq!(m.width(), 1.0);
        m.set_width(4.0);
        assert_eq!(m.width(), 4.0);
        assert!(m.dict.get("BS").unwrap().as_dict().unwrap().has("S"));
    }
}

#[cfg(test)]
mod moving_tests {
    use super::*;

    fn a_line() -> Markup {
        let mut markup = Markup::new(Subtype::Line);
        markup.dict.set(
            Name::new("L"),
            Object::Array(vec![
                Object::real(10.0),
                Object::real(20.0),
                Object::real(110.0),
                Object::real(20.0),
            ]),
        );
        markup.set_box([10.0, 15.0, 110.0, 25.0]);
        markup
    }

    #[test]
    fn a_line_moves_end_to_end() {
        let mut line = a_line();
        line.move_by(50.0, -5.0);
        let points = line.points();
        assert_eq!(points, vec![[60.0, 15.0], [160.0, 15.0]]);
        // And it is the same length it was, because moving a measurement does
        // not change what it measured.
        let was = 110.0 - 10.0;
        let now = points[1][0] - points[0][0];
        assert_eq!(was, now);
    }

    #[test]
    fn the_box_moves_with_the_shape() {
        let mut line = a_line();
        line.move_by(50.0, -5.0);
        let rect = line.dict.get("Rect").and_then(|o| o.as_rect()).expect("a box");
        assert_eq!(rect, [60.0, 10.0, 160.0, 20.0]);
    }

    #[test]
    fn a_polygon_moves_every_vertex() {
        let mut area = Markup::new(Subtype::Polygon);
        area.set_vertices(&[[0.0, 0.0], [100.0, 0.0], [100.0, 50.0]]);
        area.set_box([0.0, 0.0, 100.0, 50.0]);
        area.move_by(-10.0, 10.0);
        assert_eq!(
            area.points(),
            vec![[-10.0, 10.0], [90.0, 10.0], [90.0, 60.0]]
        );
    }

    #[test]
    fn freehand_moves_every_stroke() {
        let mut ink = Markup::new(Subtype::Ink);
        ink.set_ink(&[vec![[0.0, 0.0], [10.0, 10.0]], vec![[20.0, 20.0]]]);
        ink.set_box([0.0, 0.0, 20.0, 20.0]);
        ink.move_by(5.0, 5.0);
        let strokes = ink.ink();
        assert_eq!(strokes[0], vec![[5.0, 5.0], [15.0, 15.0]]);
        assert_eq!(strokes[1], vec![[25.0, 25.0]]);
    }

    #[test]
    fn a_callout_moves_with_its_leader() {
        let mut callout = Markup::new(Subtype::FreeText);
        callout.set_box([100.0, 100.0, 200.0, 140.0]);
        callout.set("CL", Object::Array([20.0, 20.0, 60.0, 60.0, 100.0, 120.0].iter().map(|v| Object::real(*v)).collect()));
        callout.move_by(10.0, -5.0);
        assert_eq!(callout.dict.get("CL").unwrap().numbers(), vec![30.0, 15.0, 70.0, 55.0, 110.0, 115.0]);
        assert_eq!(callout.dict.get("Rect").and_then(|o| o.as_rect()), Some([110.0, 95.0, 210.0, 135.0]));
    }

    #[test]
    fn the_old_drawing_is_thrown_away_so_it_is_drawn_again_where_it_now_is() {
        let mut line = a_line();
        line.dict.set(Name::new("AP"), Object::Null);
        line.move_by(1.0, 1.0);
        assert!(line.dict.get("AP").is_none(), "a stale appearance draws it in its old place");
    }

    #[test]
    fn moving_by_nothing_changes_nothing() {
        let mut line = a_line();
        let before = line.points();
        line.move_by(0.0, 0.0);
        assert_eq!(line.points(), before);
    }
}
