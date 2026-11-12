//! Turning a markup into an appearance stream.
//!
//! This is the difference between a markup that exists and one that can be
//! seen. Pdfium draws nothing at all for an annotation with no `/AP`, and
//! Acrobat substitutes its own guess, so Hyperview draws every markup itself and
//! ships the drawing inside the file.

use pdf::{dict, Dict, Name, Object, Stream};

use crate::content::{width_of, Content};
use crate::markup::{Markup, Subtype};

pub struct Appearance {
    /// The form XObject to hang off `/AP /N`.
    pub stream: Stream,
    /// The rectangle the annotation should declare, which must contain
    /// everything drawn including the caption.
    pub rect: [f64; 4],
    /// Objects the appearance stream refers to by name and that have to be
    /// written beside it: the picture inside a stamp, and its transparency.
    /// Whoever writes the appearance into a file writes these first and puts
    /// their references into `/Resources /XObject`.
    pub extra: Vec<(String, Stream)>,
}

/// How much of a markup gets drawn into the file.
///
/// Revu draws only the geometry into its appearance streams and paints the
/// measurement caption live, out of `/RC`, in its own viewer. That means a
/// drawing marked up in Revu shows the lines but **not the numbers** in
/// Acrobat, in a browser, or on a general contractor's screen. Hyperview draws
/// the caption in as well, so the number is there wherever the file goes.
///
/// The switch exists because a file going back to somebody working in Revu is
/// the one case where Revu's own caption could land on top of the one in the
/// file. Nobody should have to find that out by surprise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draw {
    /// Draw the measurement caption into the appearance stream.
    pub captions: bool,
}

impl Default for Draw {
    fn default() -> Draw {
        // A number you cannot see is worse than no number at all.
        Draw { captions: true }
    }
}

impl Draw {
    /// What Revu itself writes: geometry only.
    pub fn like_revu() -> Draw {
        Draw { captions: false }
    }
}

const FONT: &str = "Helv";
const SOLID: &str = "GSa";
const FILL: &str = "GSf";
const MULTIPLY: &str = "GSm";
const PICTURE: &str = "Im0";
const MASK: &str = "Sm0";

pub fn build(markup: &mut Markup) -> Option<Appearance> {
    build_with(markup, Draw::default())
}

pub fn build_with(markup: &mut Markup, how: Draw) -> Option<Appearance> {
    let subtype = markup.subtype();
    let colour = markup.colour();
    let interior = markup.interior();
    let width = markup.width().max(0.0);
    let opacity = markup.opacity();
    let fill_opacity = markup.fill_opacity();

    let mut geometry = markup.bounds()?;
    // A callout's leader reaches outside the box its words are in. The
    // rectangle has to hold it, or every reader clips the line off and the
    // callout points at nothing.
    let leader: Vec<[f64; 2]> = markup
        .dict
        .get("CL")
        .map(|o| o.numbers())
        .unwrap_or_default()
        .chunks(2)
        .filter(|p| p.len() == 2)
        .map(|p| [p[0], p[1]])
        .collect();
    let box_only = geometry;
    for point in &leader {
        geometry[0] = geometry[0].min(point[0]);
        geometry[1] = geometry[1].min(point[1]);
        geometry[2] = geometry[2].max(point[0]);
        geometry[3] = geometry[3].max(point[1]);
    }
    let caption = if how.captions {
        caption_of(markup, geometry)
    } else {
        None
    };

    // A cloudy border replaces the shape's own outline: the scallops are the
    // outline. They stand outside the path, so the rectangle has to hold them
    // or the reader clips them off and a cloud arrives as a row of nubs.
    let cloud = cloud_radius_for(markup, geometry);

    // The rectangle has to hold the stroke, which straddles the path, plus
    // anything the caption or the scallops stick out by.
    let pad = (width / 2.0 + 2.0).max(2.0)
        + ending_reach(markup, width)
        + cloud.unwrap_or(0.0);
    let mut rect = [
        geometry[0] - pad,
        geometry[1] - pad,
        geometry[2] + pad,
        geometry[3] + pad,
    ];
    if let Some(c) = &caption {
        rect[0] = rect[0].min(c.box_at[0]);
        rect[1] = rect[1].min(c.box_at[1]);
        rect[2] = rect[2].max(c.box_at[2]);
        rect[3] = rect[3].max(c.box_at[3]);
    }
    // The shape's own box is kept beside the wider rectangle, so drawing it
    // again starts from the shape and not from the allowance round it.
    if matches!(
        subtype,
        Subtype::Square | Subtype::Circle | Subtype::FreeText | Subtype::Stamp | Subtype::Text | Subtype::Link
    ) {
        markup.set_drawing_area(rect, box_only);
    } else {
        markup.set_box(rect);
    }

    let mut c = Content::new();
    // Which fonts the stream ended up naming, so the resources carry them.
    let mut words: Vec<(String, Dict)> = Vec::new();
    c.save();
    // A highlighter always multiplies; anything else does when it is set to,
    // so a filled box lets the lines under it show.
    if subtype == Subtype::Highlight || markup.multiplies() {
        c.graphics_state(MULTIPLY);
    } else if opacity < 1.0 || fill_opacity < 1.0 {
        c.graphics_state(SOLID);
    }
    c.line_cap(0).line_join(0).line_width(width);
    c.stroke_colour(colour);
    if let Some(fill) = interior {
        c.fill_colour(fill);
    }
    apply_dash(markup, &mut c);

    // A picture takes the place of the shape: what a stamp or an image markup
    // is, is the picture, sat in the box somebody dragged out.
    if let Some(picture) = markup.picture.clone() {
        return picture_stream(&picture, geometry, opacity, fill_opacity, markup.multiplies());
    }

    let points = markup.points();
    let hatch = hatch_of(markup);
    if let Some(radius) = cloud {
        let path = match subtype {
            Subtype::Square | Subtype::Circle => corners_of(geometry),
            _ if points.len() >= 2 => points.clone(),
            _ => corners_of(geometry),
        };
        let closed = !matches!(subtype, Subtype::PolyLine | Subtype::Line);
        cloud_along(&mut c, &path, closed, radius);
        paint(&mut c, interior.is_some() && closed, width.max(1.0), fill_opacity, opacity);
        c.restore();
        if let Some(caption) = &caption {
            draw_caption(&mut c, caption);
        }
        return finish_stream(c, rect, opacity, fill_opacity, subtype);
    }
    match subtype {
        Subtype::Line => {
            if points.len() >= 2 {
                c.path(&points[..2], false);
                if width > 0.0 {
                    c.stroke();
                }
                draw_endings(markup, &mut c, points[0], points[1], colour, width);
            }
        }
        Subtype::PolyLine => {
            c.path(&points, false);
            if width > 0.0 {
                c.stroke();
            }
        }
        Subtype::Polygon => {
            if let Some(kind) = hatch {
                c.path(&points, true);
                draw_hatch(&mut c, kind, geometry, interior.unwrap_or(colour), width);
                c.path(&points, true);
                if width > 0.0 {
                    c.stroke();
                }
            } else {
                c.path(&points, true);
                paint(&mut c, interior.is_some(), width, fill_opacity, opacity);
            }
        }
        Subtype::Square => {
            let half = width / 2.0;
            let box_path = |c: &mut Content| {
                c.rectangle(
                    geometry[0] + half,
                    geometry[1] + half,
                    (geometry[2] - geometry[0] - width).max(0.0),
                    (geometry[3] - geometry[1] - width).max(0.0),
                );
            };
            if let Some(kind) = hatch {
                box_path(&mut c);
                draw_hatch(&mut c, kind, geometry, interior.unwrap_or(colour), width);
                box_path(&mut c);
                if width > 0.0 {
                    c.stroke();
                }
            } else {
                box_path(&mut c);
                paint(&mut c, interior.is_some(), width, fill_opacity, opacity);
            }
        }
        Subtype::Circle => {
            let half = width / 2.0;
            let ring = |c: &mut Content| {
                c.ellipse(
                    geometry[0] + half,
                    geometry[1] + half,
                    geometry[2] - half,
                    geometry[3] - half,
                );
            };
            if let Some(kind) = hatch {
                ring(&mut c);
                draw_hatch(&mut c, kind, geometry, interior.unwrap_or(colour), width);
                ring(&mut c);
                if width > 0.0 {
                    c.stroke();
                }
            } else {
                ring(&mut c);
                paint(&mut c, interior.is_some(), width, fill_opacity, opacity);
            }
        }
        Subtype::Ink => {
            c.line_cap(1).line_join(1);
            for stroke in markup.ink() {
                if stroke.len() == 1 {
                    // A single dab still has to leave a mark.
                    let p = stroke[0];
                    c.move_to(p).line_to([p[0] + 0.01, p[1]]);
                } else {
                    c.path(&stroke, false);
                }
                c.stroke();
            }
        }
        Subtype::Highlight | Subtype::Underline | Subtype::StrikeOut | Subtype::Squiggly => {
            draw_text_markup(markup, &mut c, subtype, colour, geometry);
        }
        Subtype::FreeText => {
            // The box first, then the words in it. A text box whose words are
            // not drawn in shows as an empty rectangle everywhere but here.
            // The box is where somebody put it, not the wider rectangle a
            // leader made necessary.
            let half = width / 2.0;
            if interior.is_some() || width > 0.0 {
                c.rectangle(
                    box_only[0] + half,
                    box_only[1] + half,
                    (box_only[2] - box_only[0] - width).max(0.0),
                    (box_only[3] - box_only[1] - width).max(0.0),
                );
                paint(&mut c, interior.is_some(), width, fill_opacity, opacity);
            }
            // A callout points at what it is about.
            draw_leader(markup, &mut c, colour, width);
            words = draw_words_turned(markup, &mut c, box_only, width);
        }
        _ => {
            // Anything else at least gets its outline, so nothing is invisible.
            c.path(&points, true);
            if width > 0.0 {
                c.stroke();
            }
        }
    }
    c.restore();

    if let Some(caption) = &caption {
        draw_caption(&mut c, caption);
    }

    finish_stream_with_fonts(c, rect, opacity, fill_opacity, subtype, Vec::new(), words)
}

/// Wraps a finished content stream up as the form XObject an `/AP` needs.
fn finish_stream(
    c: Content,
    rect: [f64; 4],
    opacity: f32,
    fill_opacity: f32,
    subtype: Subtype,
) -> Option<Appearance> {
    finish_stream_with(c, rect, opacity, fill_opacity, subtype, Vec::new())
}

fn finish_stream_with(
    c: Content,
    rect: [f64; 4],
    opacity: f32,
    fill_opacity: f32,
    subtype: Subtype,
    extra: Vec<(String, Stream)>,
) -> Option<Appearance> {
    finish_stream_with_fonts(c, rect, opacity, fill_opacity, subtype, extra, Vec::new())
}

fn finish_stream_with_fonts(
    c: Content,
    rect: [f64; 4],
    opacity: f32,
    fill_opacity: f32,
    subtype: Subtype,
    extra: Vec<(String, Stream)>,
    fonts: Vec<(String, Dict)>,
) -> Option<Appearance> {
    let _ = subtype;
    let mut resources = Dict::new();
    let mut font_table = dict! {
        FONT => Object::Dict(dict!{
            "Type" => Object::name("Font"),
            "Subtype" => Object::name("Type1"),
            "BaseFont" => Object::name("Helvetica"),
            "Encoding" => Object::name("WinAnsiEncoding"),
        })
    };
    for (name, font) in fonts {
        font_table.set(Name::new(&name), Object::Dict(font));
    }
    resources.set(Name::new("Font"), Object::Dict(font_table));
    let mut states = Dict::new();
    states.set(
        Name::new(SOLID),
        Object::Dict(dict! {
            "Type" => Object::name("ExtGState"),
            "CA" => Object::real(opacity as f64),
            "ca" => Object::real(fill_opacity as f64),
        }),
    );
    states.set(
        Name::new(FILL),
        Object::Dict(dict! {
            "Type" => Object::name("ExtGState"),
            "ca" => Object::real(fill_opacity as f64),
        }),
    );
    states.set(
        Name::new(MULTIPLY),
        Object::Dict(dict! {
            "Type" => Object::name("ExtGState"),
            "BM" => Object::name("Multiply"),
            "CA" => Object::real(opacity as f64),
            "ca" => Object::real(fill_opacity as f64),
        }),
    );
    resources.set(Name::new("ExtGState"), Object::Dict(states));
    resources.set(
        Name::new("ProcSet"),
        Object::Array(vec![Object::name("PDF"), Object::name("Text")]),
    );

    let dict = dict! {
        "Type" => Object::name("XObject"),
        "Subtype" => Object::name("Form"),
        "FormType" => Object::Int(1),
        // Drawing happens in page coordinates, so the box and the rectangle
        // are the same and the matrix is the identity.
        "BBox" => Object::Array(rect.iter().map(|v| Object::real(*v)).collect()),
        "Matrix" => Object::Array(vec![
            Object::Int(1), Object::Int(0), Object::Int(0),
            Object::Int(1), Object::Int(0), Object::Int(0),
        ]),
        "Resources" => Object::Dict(resources),
    };

    Some(Appearance {
        stream: Stream {
            dict,
            data: c.into_bytes(),
        },
        rect,
        extra,
    })
}


/// The appearance of a markup that shows a picture.
///
/// The picture is drawn to fill the box, keeping its proportions, so a stamp
/// dragged out square does not come out stretched. It goes in as its own
/// object under `/Resources /XObject`, which is what lets it be one copy of
/// the data however many times the stamp is used.
fn picture_stream(
    picture: &crate::markup::Picture,
    area: [f64; 4],
    opacity: f32,
    fill_opacity: f32,
    multiply: bool,
) -> Option<Appearance> {
    if picture.width == 0 || picture.height == 0 {
        return None;
    }
    let (bw, bh) = (area[2] - area[0], area[3] - area[1]);
    if bw <= 0.0 || bh <= 0.0 {
        return None;
    }
    // Fitted inside the box, centred, so nothing is stretched.
    let shape = picture.width as f64 / picture.height as f64;
    let (mut w, mut h) = (bw, bw / shape);
    if h > bh {
        h = bh;
        w = bh * shape;
    }
    let x = area[0] + (bw - w) * 0.5;
    let y = area[1] + (bh - h) * 0.5;

    let mut c = Content::new();
    c.save();
    // Multiplied, a scanned signature's white paper disappears and only the
    // ink lands on the sheet.
    if multiply {
        c.graphics_state(MULTIPLY);
    } else if opacity < 1.0 || fill_opacity < 1.0 {
        c.graphics_state(SOLID);
    }
    c.matrix(w, 0.0, 0.0, h, x, y);
    c.draw_object(PICTURE);
    c.restore();

    let mut extra = Vec::new();
    let mut dict = dict! {
        "Type" => Object::name("XObject"),
        "Subtype" => Object::name("Image"),
        "Width" => Object::Int(picture.width as i64),
        "Height" => Object::Int(picture.height as i64),
        "BitsPerComponent" => Object::Int(8),
        "ColorSpace" => Object::name(if picture.grey { "DeviceGray" } else { "DeviceRGB" }),
        "Filter" => Object::name(picture.filter),
    };
    if let Some(mask) = &picture.mask {
        let soft = Stream {
            dict: dict! {
                "Type" => Object::name("XObject"),
                "Subtype" => Object::name("Image"),
                "Width" => Object::Int(picture.width as i64),
                "Height" => Object::Int(picture.height as i64),
                "BitsPerComponent" => Object::Int(8),
                "ColorSpace" => Object::name("DeviceGray"),
                "Filter" => Object::name("FlateDecode"),
            },
            data: mask.clone(),
        };
        extra.push((MASK.to_string(), soft));
        dict.set(Name::new("SMask"), Object::name(MASK));
    }
    extra.push((
        PICTURE.to_string(),
        Stream {
            dict,
            data: picture.data.clone(),
        },
    ));

    finish_stream_with(c, area, opacity, fill_opacity, Subtype::Stamp, extra)
}

/// The words of a text box, turned to read upright on a sheet the PDF turns for showing. `/Rotate` on the text box
/// (as Acrobat and Revu write it) is how far its words turn counter-clockwise on the page; a sheet stored sideways and
/// turned 90 degrees to be shown needs its words turned the same 90, or they are shown sideways.
fn draw_words_turned(markup: &Markup, c: &mut Content, area: [f64; 4], border: f64) -> Vec<(String, Dict)> {
    let [x0, y0, x1, y1] = area; // WHAT: the box on the page. WHY: the words are laid out inside it
    let (w, h) = (x1 - x0, y1 - y0); // WHAT: its width and height on the page. WHY: swapped below for a quarter turn
    // WHAT: the turn's matrix, where the words start (the box's bottom left as the sheet is shown), and the box as the
    // words see it. WHY: the words are laid out in an upright box of their own, then turned into place
    let (m, origin, local) = match markup.turn() {
        90 => ([0.0, 1.0, -1.0, 0.0], [x1, y0], [0.0, 0.0, h, w]),
        180 => ([-1.0, 0.0, 0.0, -1.0], [x1, y1], [0.0, 0.0, w, h]),
        270 => ([0.0, -1.0, 1.0, 0.0], [x0, y1], [0.0, 0.0, h, w]),
        _ => return draw_words(markup, c, area, border), // WHAT: not turned. WHY: drawn exactly as before
    };
    c.save(); // WHAT: keep the page's own axes. WHY: only the words turn
    c.matrix(m[0], m[1], m[2], m[3], origin[0], origin[1]); // WHAT: turn about the box's shown bottom left. WHY: the words read left to right as the sheet is shown
    let words = draw_words(markup, c, local, border); // WHAT: the words in the turned box. WHY: the same wrapping, alignment and clipping as an upright box
    c.restore(); // WHAT: back to the page's axes. WHY: nothing after it turns
    words
}

/// Draws a text markup's own words into its box.
///
/// A text box whose words live only in `/Contents` is an empty rectangle in
/// Acrobat, in a browser, and on a general contractor's screen. Drawing them
/// in is the same decision as drawing a measurement's number in: what the
/// person who marked the drawing up saw is what everybody sees.
///
/// Comes back with the fonts the stream named, for the resources.
fn draw_words(
    markup: &Markup,
    c: &mut Content,
    area: [f64; 4],
    border: f64,
) -> Vec<(String, Dict)> {
    let text = markup.contents();
    if text.trim().is_empty() {
        return Vec::new();
    }
    let setting = crate::text::Setting::of(markup);
    let (name, font) = setting.font_dict();

    // A little room off the edges, and off the border if there is one.
    let pad = 2.0 + border;
    let width = (area[2] - area[0] - pad * 2.0).max(1.0);
    let height = (area[3] - area[1] - pad * 2.0).max(1.0);
    let lines = crate::text::wrap(&text, width, &setting);
    let leading = setting.size * 1.18;
    let block = leading * lines.len() as f64;

    // Down the box: from the top, which is what a note does, unless the
    // setting says otherwise.
    let top = match setting.down {
        crate::text::Down::Top => area[3] - pad,
        crate::text::Down::Middle => area[1] + (height + block) * 0.5 + pad,
        crate::text::Down::Bottom => area[1] + block + pad,
    };

    c.save();
    // Clipped to the box, so a note with more words than room does not spill
    // across the drawing.
    c.rectangle(area[0] + border * 0.5, area[1] + border * 0.5,
        (area[2] - area[0] - border).max(0.0), (area[3] - area[1] - border).max(0.0));
    c.clip();
    for (at, line) in lines.iter().enumerate() {
        let baseline = top - leading * (at as f64 + 1.0) + setting.size * 0.22;
        if baseline + setting.size < area[1] {
            break;
        }
        let run = crate::content::width_of_in(line, setting.size, setting.family, setting.bold); // WHAT: the line's width in its own family. WHY: centred and right-aligned Courier lines were placed by Helvetica's widths.
        let x = match setting.across {
            crate::text::Across::Left => area[0] + pad,
            crate::text::Across::Middle => area[0] + pad + (width - run) * 0.5,
            crate::text::Across::Right => area[2] - pad - run,
        };
        c.text(&name, setting.size, [x, baseline], setting.colour, line);
        // Underline and strike are drawn, because a PDF font has no such
        // thing: they are lines somebody expects to be there.
        if setting.underline || setting.strike {
            c.save();
            c.stroke_colour(setting.colour);
            c.line_width((setting.size * 0.06).max(0.4));
            if setting.underline {
                let y = baseline - setting.size * 0.14;
                c.move_to([x, y]).line_to([x + run, y]);
                c.stroke();
            }
            if setting.strike {
                let y = baseline + setting.size * 0.28;
                c.move_to([x, y]).line_to([x + run, y]);
                c.stroke();
            }
            c.restore();
        }
    }
    c.restore();
    vec![(name, font)]
}

/// Draws a callout's leader: the line from the words to the thing they are
/// about, with a head on the end that points at it.
fn draw_leader(markup: &Markup, c: &mut Content, colour: [f32; 3], width: f64) {
    let points: Vec<[f64; 2]> = markup
        .dict
        .get("CL")
        .map(|o| o.numbers())
        .unwrap_or_default()
        .chunks(2)
        .filter(|p| p.len() == 2)
        .map(|p| [p[0], p[1]])
        .collect();
    if points.len() < 2 {
        return;
    }
    c.save();
    c.stroke_colour(colour);
    c.line_width(width.max(1.0));
    c.path(&points, false);
    c.stroke();
    // A head on the end that points at what the callout is about.
    let (tip, from) = (points[0], points[1]);
    let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
    let run = (dx * dx + dy * dy).sqrt().max(1e-9);
    let (ux, uy) = (dx / run, dy / run);
    let reach = (width.max(1.0) * 3.5).max(6.0);
    c.move_to(tip)
        .line_to([
            tip[0] - ux * reach - uy * reach * 0.45,
            tip[1] - uy * reach + ux * reach * 0.45,
        ])
        .line_to([
            tip[0] - ux * reach + uy * reach * 0.45,
            tip[1] - uy * reach - ux * reach * 0.45,
        ])
        .close();
    c.fill_colour(colour);
    c.fill();
    c.restore();
}

fn paint(c: &mut Content, filled: bool, width: f64, fill_opacity: f32, opacity: f32) {
    let _ = (fill_opacity, opacity);
    match (filled, width > 0.0) {
        (true, true) => {
            c.fill_and_stroke();
        }
        (true, false) => {
            c.fill();
        }
        (false, _) => {
            c.stroke();
        }
    };
}

/// How a shape is filled, beyond solid.
///
/// A hatch is what tells one area takeoff from another on a plan with six of
/// them lying over each other, and it is what lets the drawing underneath
/// still be read. Drawn as lines clipped to the shape rather than as a tiling
/// pattern: a pattern is the tidier answer and the one more readers get
/// subtly wrong, and lines are the same picture everywhere.
fn hatch_of(markup: &Markup) -> Option<&'static str> {
    let name = markup
        .dict
        .get("BSIFillType")
        .and_then(|o| o.as_name())?
        .as_str()
        .to_string();
    match name.as_str() {
        "Diagonal" => Some("Diagonal"),
        "BackDiagonal" => Some("BackDiagonal"),
        "Cross" => Some("Cross"),
        "Horizontal" => Some("Horizontal"),
        "Vertical" => Some("Vertical"),
        "Dots" => Some("Dots"),
        _ => None,
    }
}

/// Draws a hatch inside whatever path is already on the stream.
///
/// The path is used as a clip, so the hatch stops at the shape's own edge
/// however concave it is — an L-shaped room hatches as an L.
fn draw_hatch(c: &mut Content, kind: &str, area: [f64; 4], colour: [f32; 3], width: f64) {
    let step = (width.max(1.0) * 5.0).clamp(6.0, 24.0);
    let (x0, y0, x1, y1) = (area[0], area[1], area[2], area[3]);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    c.save();
    c.clip();
    c.stroke_colour(colour);
    c.line_width((width * 0.5).clamp(0.4, 2.0));
    let across = x1 - x0;
    let down = y1 - y0;
    let line = |a: [f64; 2], b: [f64; 2], c: &mut Content| {
        c.move_to(a).line_to(b);
    };
    match kind {
        "Horizontal" => {
            let mut y = y0;
            while y <= y1 {
                line([x0, y], [x1, y], c);
                y += step;
            }
        }
        "Vertical" => {
            let mut x = x0;
            while x <= x1 {
                line([x, y0], [x, y1], c);
                x += step;
            }
        }
        "Diagonal" | "BackDiagonal" | "Cross" => {
            let reach = across + down;
            let mut at = -down;
            while at <= across + down {
                if kind != "BackDiagonal" {
                    line([x0 + at, y0], [x0 + at + down, y1], c);
                }
                if kind != "Diagonal" {
                    line([x0 + at, y1], [x0 + at + down, y0], c);
                }
                at += step;
            }
            let _ = reach;
        }
        "Dots" => {
            let mut y = y0 + step * 0.5;
            while y <= y1 {
                let mut x = x0 + step * 0.5;
                while x <= x1 {
                    // A dot is a very short line with a round cap, which is
                    // one operator rather than four.
                    line([x, y], [x + 0.01, y], c);
                    x += step;
                }
                y += step;
            }
        }
        _ => {}
    }
    c.line_cap(1);
    c.stroke();
    c.restore();
}

fn apply_dash(markup: &Markup, c: &mut Content) {
    let style = markup
        .dict
        .get("BS")
        .and_then(|o| o.as_dict())
        .and_then(|d| d.get("S"))
        .and_then(|o| o.as_name())
        .map(|n| n.as_str().to_string())
        .unwrap_or_else(|| "S".into());
    if style != "D" {
        return;
    }
    let pattern = markup
        .dict
        .get("BS")
        .and_then(|o| o.as_dict())
        .and_then(|d| d.get("D"))
        .map(|o| o.numbers())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![3.0]);
    c.dash(&pattern, 0.0);
}

/// How far a line ending sticks out past the point it sits on.
fn ending_reach(markup: &Markup, width: f64) -> f64 {
    let endings = markup
        .dict
        .get("LE")
        .and_then(|o| o.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    if endings == 0 {
        return 0.0;
    }
    (width.max(1.0) * 4.0).max(6.0)
}

fn draw_endings(
    markup: &Markup,
    c: &mut Content,
    a: [f64; 2],
    b: [f64; 2],
    colour: [f32; 3],
    width: f64,
) {
    let Some(endings) = markup.dict.get("LE").and_then(|o| o.as_array()) else {
        return;
    };
    let size = (width.max(1.0) * 4.0).max(6.0);
    for (i, ending) in endings.iter().take(2).enumerate() {
        let Some(name) = ending.as_name() else { continue };
        let (at, towards) = if i == 0 { (a, b) } else { (b, a) };
        let (dx, dy) = (towards[0] - at[0], towards[1] - at[1]);
        let length = (dx * dx + dy * dy).sqrt().max(1e-9);
        let dir = [dx / length, dy / length];
        draw_ending(c, name.as_str(), at, dir, size, colour);
    }
}

fn draw_ending(c: &mut Content, kind: &str, at: [f64; 2], dir: [f64; 2], size: f64, colour: [f32; 3]) {
    let side = [-dir[1], dir[0]];
    let tip = at;
    let back = [at[0] + dir[0] * size, at[1] + dir[1] * size];
    let half = size * 0.35;
    c.fill_colour(colour);
    match kind {
        "OpenArrow" => {
            c.move_to([back[0] + side[0] * half, back[1] + side[1] * half])
                .line_to(tip)
                .line_to([back[0] - side[0] * half, back[1] - side[1] * half])
                .stroke();
        }
        "ClosedArrow" => {
            c.move_to(tip)
                .line_to([back[0] + side[0] * half, back[1] + side[1] * half])
                .line_to([back[0] - side[0] * half, back[1] - side[1] * half])
                .close()
                .fill();
        }
        "Square" => {
            let h = size * 0.3;
            c.rectangle(at[0] - h, at[1] - h, h * 2.0, h * 2.0).fill();
        }
        "Circle" => {
            let h = size * 0.3;
            c.ellipse(at[0] - h, at[1] - h, at[0] + h, at[1] + h).fill();
        }
        "Diamond" => {
            let h = size * 0.35;
            c.move_to([at[0] - h, at[1]])
                .line_to([at[0], at[1] + h])
                .line_to([at[0] + h, at[1]])
                .line_to([at[0], at[1] - h])
                .close()
                .fill();
        }
        "Butt" => {
            let h = size * 0.35;
            c.move_to([at[0] + side[0] * h, at[1] + side[1] * h])
                .line_to([at[0] - side[0] * h, at[1] - side[1] * h])
                .stroke();
        }
        "Slash" => {
            let h = size * 0.35;
            let d = [
                (dir[0] + side[0]) * std::f64::consts::FRAC_1_SQRT_2, // WHAT: 1/sqrt(2), exactly. WHY: the constant clippy asks for; 0.7071 stopped the strict lint from reaching most of the program
                (dir[1] + side[1]) * std::f64::consts::FRAC_1_SQRT_2,
            ];
            c.move_to([at[0] + d[0] * h, at[1] + d[1] * h])
                .line_to([at[0] - d[0] * h, at[1] - d[1] * h])
                .stroke();
        }
        _ => {}
    }
}

fn draw_text_markup(
    markup: &Markup,
    c: &mut Content,
    subtype: Subtype,
    colour: [f32; 3],
    geometry: [f64; 4],
) {
    // Text markups carry their coverage as groups of four corners.
    let quads = markup
        .dict
        .get("QuadPoints")
        .map(|o| o.numbers())
        .unwrap_or_default();
    let boxes: Vec<[f64; 4]> = if quads.len() >= 8 {
        quads
            .chunks(8)
            .filter(|q| q.len() == 8)
            .map(|q| {
                let xs = [q[0], q[2], q[4], q[6]];
                let ys = [q[1], q[3], q[5], q[7]];
                [
                    xs.iter().cloned().fold(f64::MAX, f64::min),
                    ys.iter().cloned().fold(f64::MAX, f64::min),
                    xs.iter().cloned().fold(f64::MIN, f64::max),
                    ys.iter().cloned().fold(f64::MIN, f64::max),
                ]
            })
            .collect()
    } else {
        vec![geometry]
    };

    c.fill_colour(colour);
    for b in boxes {
        match subtype {
            Subtype::Highlight => {
                c.rectangle(b[0], b[1], b[2] - b[0], b[3] - b[1]).fill();
            }
            Subtype::Underline => {
                let y = b[1] + (b[3] - b[1]) * 0.08;
                c.line_width((b[3] - b[1]) * 0.07 + 0.5)
                    .move_to([b[0], y])
                    .line_to([b[2], y])
                    .stroke();
            }
            Subtype::StrikeOut => {
                let y = (b[1] + b[3]) / 2.0;
                c.line_width((b[3] - b[1]) * 0.07 + 0.5)
                    .move_to([b[0], y])
                    .line_to([b[2], y])
                    .stroke();
            }
            Subtype::Squiggly => {
                let y = b[1] + (b[3] - b[1]) * 0.1;
                let step = ((b[3] - b[1]) * 0.25).max(2.0);
                c.line_width(0.8).move_to([b[0], y]);
                let mut x = b[0];
                let mut up = true;
                while x < b[2] {
                    x = (x + step).min(b[2]);
                    c.line_to([x, if up { y + step } else { y }]);
                    up = !up;
                }
                c.stroke();
            }
            _ => {}
        }
    }
}

struct Caption {
    text: String,
    size: f64,
    colour: [f32; 3],
    /// Bottom left of the baseline.
    at: [f64; 2],
    /// Rotation, as cosine and sine.
    turn: [f64; 2],
    box_at: [f64; 4],
}

fn caption_of(markup: &Markup, geometry: [f64; 4]) -> Option<Caption> {
    let text = markup.contents();
    if text.is_empty() {
        return None;
    }
    // A note or a free text box paints its own words; this is only for the
    // labels that ride on a measurement.
    if matches!(
        markup.subtype(),
        Subtype::FreeText | Subtype::Text | Subtype::Stamp | Subtype::Link
    ) {
        return None;
    }
    let size = markup.font_size();
    let colour = markup.colour();
    let width = width_of(&text, size, false);
    let height = size;

    let points = markup.points();
    let (centre, turn) = match (markup.subtype(), points.len()) {
        (Subtype::Line, n) if n >= 2 => {
            let (a, b) = (points[0], points[1]);
            let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            if markup.along_segment() {
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let len = (dx * dx + dy * dy).sqrt().max(1e-9);
                // Keep the words the right way up rather than upside down.
                let (mut cx, mut sy) = (dx / len, dy / len);
                if cx < 0.0 {
                    cx = -cx;
                    sy = -sy;
                }
                (mid, [cx, sy])
            } else {
                (mid, [1.0, 0.0])
            }
        }
        _ => (
            [
                (geometry[0] + geometry[2]) / 2.0,
                (geometry[1] + geometry[3]) / 2.0,
            ],
            [1.0, 0.0],
        ),
    };

    // Centre the text on the point, along whatever direction it runs in.
    let half = width / 2.0;
    let drop = height * 0.35;
    let at = [
        centre[0] - turn[0] * half + turn[1] * drop,
        centre[1] - turn[1] * half - turn[0] * drop,
    ];
    let reach = half.max(height);
    let box_at = [
        centre[0] - reach - 2.0,
        centre[1] - reach - 2.0,
        centre[0] + reach + 2.0,
        centre[1] + reach + 2.0,
    ];
    Some(Caption {
        text,
        size,
        colour,
        at,
        turn,
        box_at,
    })
}

fn draw_caption(c: &mut Content, caption: &Caption) {
    c.save();
    c.text_turned(
        FONT,
        caption.size,
        caption.at,
        caption.turn,
        caption.colour,
        &caption.text,
    );
    c.restore();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream_text(a: &Appearance) -> String {
        String::from_utf8_lossy(&a.stream.data).into_owned()
    }

    fn cloudy(m: &mut Markup, intensity: f64) {
        let mut effect = Dict::new();
        effect.set("S", Object::name("C"));
        effect.set("I", Object::real(intensity));
        m.dict.set("BE", Object::Dict(effect));
    }

    #[test]
    fn a_text_box_draws_its_own_words_into_the_file() {
        // A text box whose words live only in /Contents is an empty rectangle
        // in Acrobat, in a browser, and on a general contractor's screen.
        let mut m = Markup::new(Subtype::FreeText);
        m.set_box([100.0, 100.0, 400.0, 200.0]);
        m.set_contents("FIELD VERIFY");
        let text = stream_text(&m.finish().unwrap());
        assert!(text.contains("BT"), "{text}");
        assert!(text.contains("(FIELD VERIFY)"), "{text}");
        assert!(text.contains("Tj"), "{text}");
    }

    #[test]
    fn a_text_box_with_nothing_in_it_draws_no_words() {
        let mut m = Markup::new(Subtype::FreeText);
        m.set_box([0.0, 0.0, 100.0, 40.0]);
        let text = stream_text(&m.finish().unwrap());
        assert!(!text.contains("Tj"), "{text}");
    }

    #[test]
    fn long_words_wrap_inside_the_box_rather_than_running_off_the_sheet() {
        let mut m = Markup::new(Subtype::FreeText);
        m.set_box([0.0, 0.0, 120.0, 200.0]);
        m.set_contents(
            "FIELD VERIFY ALL DIMENSIONS BEFORE FABRICATION AND REPORT ANY CONFLICT",
        );
        let text = stream_text(&m.finish().unwrap());
        assert!(text.matches("Tj").count() > 3, "it should be several lines: {text}");
        // And clipped, so a note with more words than room stays in its box.
        assert!(text.contains("W n"), "{text}");
    }

    #[test]
    fn the_font_a_markup_asks_for_is_in_its_resources() {
        // Naming a font the stream does not carry is how a note comes out in
        // whatever the other machine had instead.
        let mut m = Markup::new(Subtype::FreeText);
        m.set_box([0.0, 0.0, 200.0, 60.0]);
        m.set_contents("SEE DETAIL");
        crate::text::Setting {
            family: crate::text::Family::Times,
            bold: true,
            ..Default::default()
        }
        .onto(&mut m);
        let appearance = m.finish().unwrap();
        let text = stream_text(&appearance);
        assert!(text.contains("/TiRoB"), "{text}");
        let resources = appearance.stream.dict.get("Resources").unwrap().as_dict().unwrap();
        let fonts = resources.get("Font").unwrap().as_dict().unwrap();
        let font = fonts.get("TiRoB").unwrap().as_dict().unwrap();
        assert_eq!(
            font.get("BaseFont").and_then(|o| o.as_name()).map(|n| n.as_str().to_string()),
            Some("Times-Bold".to_string())
        );
    }

    #[test]
    fn a_callout_draws_the_line_that_points_at_what_it_is_about() {
        let mut m = Markup::new(Subtype::FreeText);
        m.set_box([200.0, 100.0, 400.0, 160.0]);
        m.set_contents("THIS ONE");
        m.set(
            "CL",
            Object::Array(
                [40.0f64, 40.0, 200.0, 100.0]
                    .iter()
                    .map(|v| Object::real(*v))
                    .collect(),
            ),
        );
        let text = stream_text(&m.finish().unwrap());
        assert!(text.contains("40 40 m"), "the leader starts at the thing: {text}");
    }

    #[test]
    fn a_cloud_is_drawn_as_scallops_and_not_as_a_box() {
        // An appearance stream wins over /BE in every reader. A revision cloud
        // whose appearance is a rectangle arrives at the general contractor as
        // a rectangle, and nobody reading the set would know it meant a
        // revision.
        let mut plain = Markup::new(Subtype::Square);
        plain.set_colour([1.0, 0.0, 0.0]).set_width(2.0);
        plain.set_box([100.0, 100.0, 300.0, 240.0]);
        let plain_text = stream_text(&plain.finish().unwrap());
        assert!(plain_text.contains(" re\n"), "a plain box is a rectangle: {plain_text}");

        let mut cloud = Markup::new(Subtype::Square);
        cloud.set_colour([1.0, 0.0, 0.0]).set_width(2.0);
        cloud.set_box([100.0, 100.0, 300.0, 240.0]);
        cloudy(&mut cloud, 2.0);
        let cloud_text = stream_text(&cloud.finish().unwrap());
        assert!(!cloud_text.contains(" re\n"), "a cloud is not a rectangle: {cloud_text}");
        assert!(cloud_text.contains(" c\n"), "a cloud is curves: {cloud_text}");
        // Enough of them to read as a cloud round a box that size.
        assert!(cloud_text.matches(" c\n").count() > 20, "{}", cloud_text.matches(" c\n").count());
        assert!(cloud_text.contains("h\n"), "and it closes: {cloud_text}");
    }

    #[test]
    fn a_deeper_cloud_scallops_further_out() {
        let make = |intensity: f64| {
            let mut m = Markup::new(Subtype::Square);
            m.set_width(1.0);
            m.set_box([100.0, 100.0, 300.0, 240.0]);
            cloudy(&mut m, intensity);
            m.finish().unwrap().rect
        };
        let shallow = make(1.0);
        let deep = make(3.0);
        let _ = (shallow, deep);
        assert!(cloud_radius(&{
            let mut m = Markup::new(Subtype::Square);
            cloudy(&mut m, 3.0);
            m
        })
        .unwrap()
            > cloud_radius(&{
                let mut m = Markup::new(Subtype::Square);
                cloudy(&mut m, 1.0);
                m
            })
            .unwrap());
    }

    #[test]
    fn a_border_effect_that_is_not_cloudy_leaves_the_shape_alone() {
        let mut m = Markup::new(Subtype::Square);
        m.set_width(2.0);
        m.set_box([0.0, 0.0, 100.0, 60.0]);
        let mut effect = Dict::new();
        effect.set("S", Object::name("S"));
        m.dict.set("BE", Object::Dict(effect));
        assert!(cloud_radius(&m).is_none());
        assert!(stream_text(&m.finish().unwrap()).contains(" re\n"));
    }

    #[test]
    fn a_cloud_with_no_depth_is_a_plain_shape() {
        // /I 0 means "cloudy, but not at all", which Revu writes when somebody
        // turns the effect off without removing it.
        let mut m = Markup::new(Subtype::Square);
        m.set_width(2.0);
        m.set_box([0.0, 0.0, 100.0, 60.0]);
        cloudy(&mut m, 0.0);
        assert!(cloud_radius(&m).is_none());
    }

    #[test]
    fn a_clouded_polygon_follows_the_points_it_was_drawn_round() {
        let mut m = Markup::new(Subtype::Polygon);
        m.set_width(2.0);
        m.set_vertices(&[[0.0, 0.0], [200.0, 0.0], [200.0, 120.0], [0.0, 120.0]]);
        cloudy(&mut m, 2.0);
        let text = stream_text(&m.finish().unwrap());
        assert!(text.contains(" c\n"));
        // The rectangle has to hold the scallops, which stand outside the
        // points themselves. A rectangle that only allows for the stroke
        // clips them off, and the cloud arrives as a row of nubs.
        let radius = cloud_radius(&m).unwrap();
        let rect = m.dict.get("Rect").unwrap().as_rect().unwrap();
        assert!(rect[0] <= -radius && rect[2] >= 200.0 + radius, "{rect:?}");
        assert!(rect[1] <= -radius && rect[3] >= 120.0 + radius, "{rect:?}");
    }

    #[test]
    fn a_line_produces_a_stroked_path_and_a_rectangle_that_holds_it() {
        let mut m = Markup::new(Subtype::Line);
        m.set_colour([1.0, 0.0, 0.0]).set_width(2.0);
        m.set_line([100.0, 100.0], [460.0, 100.0]);
        let a = m.finish().unwrap();
        let text = stream_text(&a);
        assert!(text.contains("1 0 0 RG"), "{text}");
        assert!(text.contains("100 100 m"), "{text}");
        assert!(text.contains("460 100 l"), "{text}");
        assert!(text.contains("S\n"));
        assert!(a.rect[0] < 100.0 && a.rect[2] > 460.0, "{:?}", a.rect);
        assert!(a.rect[1] < 100.0 && a.rect[3] > 100.0);
        // And the markup now carries that rectangle.
        assert_eq!(m.dict.get("Rect").unwrap().as_rect(), Some(a.rect));
    }

    #[test]
    fn a_caption_widens_the_rectangle_enough_to_hold_the_words() {
        let mut bare = Markup::new(Subtype::Line);
        bare.set_width(1.0).set_line([0.0, 0.0], [10.0, 0.0]);
        let narrow = bare.finish().unwrap().rect;

        let mut captioned = Markup::new(Subtype::Line);
        captioned.set_width(1.0).set_line([0.0, 0.0], [10.0, 0.0]);
        captioned.set_contents("128'-11 15/16\"");
        let wide = captioned.finish().unwrap().rect;

        assert!(wide[2] - wide[0] > narrow[2] - narrow[0]);
        assert!(stream_text(&captioned.finish().unwrap()).contains("Tj"));
    }

    #[test]
    fn an_area_is_filled_as_well_as_stroked_when_it_has_an_interior_colour() {
        let mut m = Markup::new(Subtype::Polygon);
        m.set_colour([1.0, 0.0, 0.0]).set_width(1.0);
        m.set("IC", Object::Array(vec![Object::real(1.0), Object::real(1.0), Object::real(0.5)]));
        m.set_vertices(&[[0.0, 0.0], [100.0, 0.0], [100.0, 50.0], [0.0, 50.0]]);
        let text = stream_text(&m.finish().unwrap());
        assert!(text.contains("1 1 0.5 rg"), "{text}");
        assert!(text.contains("h\nB\n"), "{text}");
    }

    #[test]
    fn an_area_with_no_interior_colour_is_only_outlined() {
        let mut m = Markup::new(Subtype::Polygon);
        m.set_colour([0.0, 0.0, 1.0]).set_width(1.0);
        m.set_vertices(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
        let text = stream_text(&m.finish().unwrap());
        assert!(text.contains("h\nS\n"), "{text}");
        assert!(!text.contains(" rg\n") || !text.contains("B\n"));
    }

    #[test]
    fn transparency_names_a_graphics_state_the_resources_actually_define() {
        let mut m = Markup::new(Subtype::Polygon);
        m.set_colour([1.0, 0.0, 0.0]).set_width(1.0);
        m.set("CA", Object::real(0.5));
        m.set_vertices(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
        let a = m.finish().unwrap();
        assert!(stream_text(&a).contains("/GSa gs"));
        let states = a
            .stream
            .dict
            .get("Resources")
            .and_then(|o| o.as_dict())
            .and_then(|d| d.get("ExtGState"))
            .and_then(|o| o.as_dict())
            .unwrap();
        assert!(states.has("GSa"));
    }

    #[test]
    fn an_empty_markup_makes_no_appearance_rather_than_a_broken_one() {
        let mut m = Markup::new(Subtype::Line);
        assert!(m.finish().is_none());
    }

    #[test]
    fn the_box_and_the_rectangle_agree_so_nothing_is_scaled() {
        let mut m = Markup::new(Subtype::Circle);
        m.set_colour([0.0, 0.0, 0.0]).set_width(1.0);
        m.set_box([10.0, 10.0, 60.0, 40.0]);
        let a = m.finish().unwrap();
        assert_eq!(
            a.stream.dict.get("BBox").unwrap().as_rect(),
            Some(a.rect),
            "a box that differs from the rectangle silently scales the drawing"
        );
    }

    #[test]
    fn a_pen_stroke_of_a_single_dab_still_leaves_a_mark() {
        let mut m = Markup::new(Subtype::Ink);
        m.set_colour([0.0, 0.0, 0.0]).set_width(2.0);
        m.set_ink(&[vec![[50.0, 50.0]]]);
        let text = stream_text(&m.finish().unwrap());
        assert!(text.contains("50 50 m"), "{text}");
        assert!(text.contains("S\n"));
    }

    /// Drawn, moved and drawn again, as happens every time a markup is moved
    /// and saved: the shape has to come back the size it was.
    fn redrawn(mut m: Markup) -> ([f64; 4], [f64; 4]) {
        m.finish().unwrap();
        let first = m.bounds().unwrap();
        for _ in 0..5 {
            m.move_by(10.0, -4.0);
            m.finish().unwrap();
        }
        let last = m.bounds().unwrap();
        (first, [last[0] - 50.0, last[1] + 20.0, last[2] - 50.0, last[3] + 20.0])
    }

    fn close(a: [f64; 4], b: [f64; 4]) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    #[test]
    fn drawing_a_shape_again_does_not_make_it_bigger() {
        let mut boxed = Markup::new(Subtype::Square);
        boxed.set_colour([1.0, 0.0, 0.0]).set_width(3.0);
        boxed.set_box([100.0, 100.0, 300.0, 200.0]);
        let (a, b) = redrawn(boxed);
        assert!(close(a, [100.0, 100.0, 300.0, 200.0]), "{a:?}");
        assert!(close(a, b), "{a:?} became {b:?}");

        let mut ring = Markup::new(Subtype::Circle);
        ring.set_colour([0.0, 0.0, 1.0]).set_width(2.0);
        ring.set_box([10.0, 10.0, 60.0, 40.0]);
        let (a, b) = redrawn(ring);
        assert!(close(a, b), "{a:?} became {b:?}");
    }

    #[test]
    fn a_cloud_stays_the_size_it_was_drawn() {
        // The scallops stick out a long way, so a cloud grew the most.
        let mut cloud = Markup::new(Subtype::Square);
        cloud.set_colour([1.0, 0.0, 0.0]).set_width(2.0);
        let mut effect = Dict::new();
        effect.set(Name::new("S"), Object::name("C"));
        effect.set(Name::new("I"), Object::Real(2.0));
        cloud.dict.set(Name::new("BE"), Object::Dict(effect));
        cloud.set_box([100.0, 100.0, 300.0, 240.0]);
        let (a, b) = redrawn(cloud);
        assert!(close(a, [100.0, 100.0, 300.0, 240.0]), "{a:?}");
        assert!(close(a, b), "{a:?} became {b:?}");
    }

    #[test]
    fn a_callouts_box_stays_where_it_was_and_the_same_size() {
        let mut m = Markup::new(Subtype::FreeText);
        m.set_colour([1.0, 0.0, 0.0]).set_width(1.0);
        m.set_box([200.0, 100.0, 400.0, 160.0]);
        m.set(
            "CL",
            Object::Array(vec![Object::real(50.0), Object::real(50.0), Object::real(200.0), Object::real(130.0)]),
        );
        let (a, b) = redrawn(m);
        assert!(close(a, [200.0, 100.0, 400.0, 160.0]), "the box is the box, not the leader: {a:?}");
        assert!(close(a, b), "{a:?} became {b:?}");
    }

    #[test]
    fn a_box_set_to_multiply_is_drawn_multiplied() {
        let mut m = Markup::new(Subtype::Square);
        m.set_colour([1.0, 0.0, 0.0]).set_width(2.0);
        m.set("IC", Object::Array(vec![Object::real(1.0), Object::real(1.0), Object::real(0.0)]));
        m.set_box([0.0, 0.0, 100.0, 60.0]);
        let plain = stream_text(&m.finish().unwrap());
        assert!(!plain.contains("/GSm gs"), "{plain}");
        m.set_multiply(true);
        let multiplied = stream_text(&m.finish().unwrap());
        assert!(multiplied.contains("/GSm gs"), "{multiplied}");
        assert!(m.multiplies());
    }

    #[test]
    fn a_highlight_is_where_its_words_are() {
        let mut m = Markup::new(Subtype::Highlight);
        m.set_colour([1.0, 1.0, 0.0]);
        m.set_box([10.0, 10.0, 110.0, 25.0]);
        m.set(
            "QuadPoints",
            Object::Array(
                [10.0, 25.0, 110.0, 25.0, 10.0, 10.0, 110.0, 10.0].iter().map(|v| Object::real(*v)).collect(),
            ),
        );
        let (a, b) = redrawn(m);
        assert!(close(a, b), "{a:?} became {b:?}");
    }
}

// ---- cloudy borders ---------------------------------------------------------

/// How deep the scallops are, from `/BE /I`, or `None` when the border is plain.
///
/// A cloud in a PDF is not a shape of its own: it is an ordinary Square,
/// Circle or Polygon carrying a border effect that says "draw this edge
/// cloudy". Revu writes it that way and so does Acrobat. Hyperview draws its
/// own appearance streams, and an appearance stream wins over `/BE` in every
/// reader — so if this is not drawn here, a revision cloud saved by Hyperview
/// would arrive at the general contractor as a plain rectangle. That is the
/// whole reason this function exists.
pub fn cloud_radius(markup: &Markup) -> Option<f64> {
    let effect = markup.dict.get("BE")?.as_dict()?;
    let style = effect.get("S").and_then(|o| o.as_name())?;
    if style.as_str() != "C" {
        return None;
    }
    let intensity = effect
        .get("I")
        .and_then(|o| o.as_f64())
        .unwrap_or(1.0)
        .clamp(0.0, 4.0);
    if intensity <= 0.0 {
        return None;
    }
    // Revu's own clouds at intensity 2 scallop about nine points deep.
    Some(3.0 + 3.0 * intensity)
}

/// The same, grown to suit the shape it goes round.
///
/// Nine points is right on a letter page and invisible on an ARCH-E sheet
/// three thousand points across — and a revision cloud nobody can see on the
/// plot is a revision nobody catches. So the scallops never go below a
/// hundredth of the way round the shape, which keeps a cloud looking like one
/// at whatever size it is drawn.
pub fn cloud_radius_for(markup: &Markup, area: [f64; 4]) -> Option<f64> {
    let base = cloud_radius(markup)?;
    let across = (area[2] - area[0]).abs();
    let down = (area[3] - area[1]).abs();
    let round_it = (across + down) * 2.0;
    Some(base.max(round_it / 100.0).min(48.0))
}

/// Lays a run of scalloped arcs along a path, bulging away from the middle.
///
/// Each scallop is a half circle whose diameter is the bit of edge it sits on,
/// which is what makes a cloud read as a cloud rather than as a row of spikes.
/// The edge is divided into a whole number of them so the last one lands
/// exactly on the corner.
fn cloud_along(c: &mut Content, points: &[[f64; 2]], closed: bool, radius: f64) {
    if points.len() < 2 || radius <= 0.0 {
        return;
    }
    // Which side is out. For a closed path, the side away from the middle of
    // the shape; for an open one, the left of travel, which is what a hand
    // drawing a cloud along a line does.
    let middle = {
        let n = points.len() as f64;
        let sx: f64 = points.iter().map(|p| p[0]).sum();
        let sy: f64 = points.iter().map(|p| p[1]).sum();
        [sx / n, sy / n]
    };

    // The circle drawn four ways, to the usual accuracy.
    const K: f64 = 0.5523;

    let mut first = true;
    let count = if closed { points.len() } else { points.len() - 1 };
    for i in 0..count {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let length = (dx * dx + dy * dy).sqrt();
        if length < 1e-6 {
            continue;
        }
        let bumps = ((length / (radius * 2.0)).round() as usize).max(1);
        let along = length / bumps as f64;
        let r = along * 0.5;
        let (ux, uy) = (dx / length, dy / length);
        let (mut nx, mut ny) = (-uy, ux);
        if closed {
            let mid = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
            let out = [mid[0] - middle[0], mid[1] - middle[1]];
            if nx * out[0] + ny * out[1] < 0.0 {
                nx = -nx;
                ny = -ny;
            }
        }

        for bump in 0..bumps {
            let from = [
                a[0] + ux * along * bump as f64,
                a[1] + uy * along * bump as f64,
            ];
            let to = [
                a[0] + ux * along * (bump + 1) as f64,
                a[1] + uy * along * (bump + 1) as f64,
            ];
            let centre = [(from[0] + to[0]) * 0.5, (from[1] + to[1]) * 0.5];
            let peak = [centre[0] + nx * r, centre[1] + ny * r];
            if first {
                c.move_to(from);
                first = false;
            }
            // Two quarter circles: up to the peak, then down to the far end.
            c.curve_to(
                [from[0] + nx * r * K, from[1] + ny * r * K],
                [peak[0] - ux * r * K, peak[1] - uy * r * K],
                peak,
            );
            c.curve_to(
                [peak[0] + ux * r * K, peak[1] + uy * r * K],
                [to[0] + nx * r * K, to[1] + ny * r * K],
                to,
            );
        }
    }
    if closed {
        c.close();
    }
}

/// The four corners of a rectangle, anticlockwise from the bottom left.
fn corners_of(area: [f64; 4]) -> Vec<[f64; 2]> {
    vec![
        [area[0], area[1]],
        [area[2], area[1]],
        [area[2], area[3]],
        [area[0], area[3]],
    ]
}

#[cfg(test)]
mod turned_words {
    // WHAT: text boxes on sheets a PDF turns for showing. WHY: 0.6.31 wrote their words along the stored page, so on a sheet stored sideways the words were shown sideways (Creede, 2026-11-11)
    use super::*;

    const WORDS: &str = "Welds were completed 6 in on the top of the bars and 1 1/2 in on the vertical sides"; // WHAT: a field note long enough to wrap. WHY: how far it wraps shows which way it reads

    fn tall_box(turn: Option<i64>) -> Markup {
        let mut m = Markup::new(Subtype::FreeText);
        m.set_box([100.0, 100.0, 200.0, 400.0]); // WHAT: 100 wide and 300 tall on the page. WHY: on a sheet turned 90 degrees it shows 300 wide and 100 tall
        m.set_contents(WORDS);
        if let Some(t) = turn {
            m.set("Rotate", Object::Int(t));
        } // WHAT: the box's own turn. WHY: each case below
        m
    }

    fn drawn(m: &mut Markup) -> String {
        String::from_utf8_lossy(&m.finish().unwrap().stream.data).into_owned() // WHAT: the appearance as text. WHY: its operators are read below
    }

    #[test]
    fn a_box_turned_a_quarter_writes_its_words_turned_from_its_shown_bottom_left() {
        let text = drawn(&mut tall_box(Some(90)));
        assert!(text.contains("0 1 -1 0 200 100 cm"), "words turned about the shown bottom left: {text}"); // WHAT: x runs up the page, starting at the right edge. WHY: that is left to right once the sheet is turned
        assert!(text.contains("(Welds were completed"), "the words are drawn: {text}");
    }

    #[test]
    fn a_turned_box_wraps_to_its_shown_width_not_its_stored_width() {
        let upright = drawn(&mut tall_box(None)).matches("Tj").count(); // WHAT: lines as 0.6.31 wrapped them, 96 points wide. WHY: the baseline
        let turned = drawn(&mut tall_box(Some(90))).matches("Tj").count(); // WHAT: lines wrapped 296 points wide. WHY: the width the sheet shows
        assert!(turned < upright, "fewer, longer lines: {turned} vs {upright}");
    }

    #[test]
    fn every_quarter_turn_starts_at_the_corner_shown_bottom_left() {
        for (turn, cm) in [(180, "-1 0 0 -1 200 400 cm"), (270, "0 -1 1 0 100 400 cm"), (-90, "0 -1 1 0 100 400 cm"), (450, "0 1 -1 0 200 100 cm")] {
            assert!(drawn(&mut tall_box(Some(turn))).contains(cm), "{turn}: {cm}");
        } // WHAT: each turn's matrix and corner. WHY: a wrong corner puts the words outside the box, clipped to nothing
    }

    #[test]
    fn an_unturned_box_is_drawn_exactly_as_before() {
        assert_eq!(drawn(&mut tall_box(None)), drawn(&mut tall_box(Some(0))), "a turn of 0 is no turn");
        assert!(!drawn(&mut tall_box(None)).contains(" cm"), "no matrix for an upright box"); // WHAT: unchanged output. WHY: every box on an upright sheet stays byte for byte what 0.6.31 wrote
    }

    #[test]
    fn a_text_box_takes_its_sheets_turn_once_and_nothing_else_does() {
        let mut m = tall_box(None);
        m.turn_with_sheet(90);
        assert_eq!(m.turn(), 90, "an unturned box on a turned sheet takes the turn");
        m.turn_with_sheet(180);
        assert_eq!(m.turn(), 90, "a box already turned keeps its turn"); // WHAT: kept. WHY: a box Revu turned stays as Revu turned it
        let mut upright = tall_box(None);
        upright.turn_with_sheet(0);
        assert!(!upright.dict.has("Rotate"), "an upright sheet writes nothing");
        let mut cloud = Markup::new(Subtype::Square);
        cloud.turn_with_sheet(90);
        assert!(!cloud.dict.has("Rotate"), "only a text box turns its words"); // WHAT: a cloud has no words. WHY: nothing else changes
        assert_eq!(tall_box(Some(-90)).turn(), 270, "a negative turn reads as its quarter");
    }
}
