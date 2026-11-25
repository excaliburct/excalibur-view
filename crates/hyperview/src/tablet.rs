//! The shop-and-field layout: the tablet's own screens (docs/TABLET-MANIFEST.md).
//!
//! One rule: two taps to anything, and no folders. A shop or field tablet
//! shows the jobs the office assigned to the person, then a job's home (the
//! "do this now" banner and its tiles), then the thing itself: the released
//! sheets, the loads and their tickets with the signing pad, the documents
//! the office pinned. Everything comes from the job's manifest, which the
//! office server cuts to what this person may reach; nothing here decides
//! who sees what.
//!
//! Drawn in Fabwire Modern's colours (`Excalibur.Theme/ExcaliburModernTheme.axaml`),
//! in Inter, with nothing smaller than 52 points to tap, dark indoors and a
//! bright look for the sun. The screens are drawn by [`Tablet::show`] from
//! plain data, so a test draws them too ([`crate::tablet`]'s tests, and the
//! screenshots); [`App`] only feeds it and carries out what it asks.

// WHAT: the lints this screen code is excused from, for this file only. WHY: screen positions are small
// numbers turned between f32 and counts (never near a precision limit); colours are written #RRGGBB as the
// theme file writes them; each screen is one long drawing function laid out top to bottom, as the mockups are;
// a status match names each case even when two look alike, so a new status is a choice, not a fall-through.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::unreadable_literal,
    clippy::too_many_lines,
    clippy::struct_excessive_bools,
    clippy::semicolon_if_nothing_returned,
    clippy::match_same_arms,
    clippy::single_match_else,
    clippy::type_complexity,
    clippy::many_single_char_names
)]

use std::collections::BTreeSet; // WHAT: the sheets opened here. WHY: the NEW badge, kept sorted for the prefs file.

use egui::{
    Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
}; // WHAT: egui's drawing parts. WHY: every screen is painted to the mockups' sizes.
use hub::tablet::{may_sign, Audience, Load, Sheet, TabletJob, TabletView, TicketBox}; // WHAT: the manifest's shapes and the signing rule. WHY: what the screens show, and who may sign which box.

use crate::app::{App, Tool}; // WHAT: the window and its tools. WHY: the glue at the bottom.

/// The smallest thing anyone taps, in points.
pub const SMALLEST: f32 = 52.0; // WHAT: 52 points. WHY: the design's glove rule; the tests hold every target to it.
/// The main action's height.
pub const MAIN: f32 = 76.0; // WHAT: 76 points. WHY: one big button a screen.
/// The section rail's width.
const RAIL: f32 = 112.0; // WHAT: the rail down the left. WHY: as the mockups.
/// The top bar's height.
const TOP: f32 = 68.0; // WHAT: the top bar. WHY: as the mockups.
/// Narrower than this is a phone: one column.
pub const PHONE: f32 = 700.0; // WHAT: the phone's break. WHY: the sign flow in one column (mockup j).

/// The colours of one look: Fabwire Modern's, dark or bright.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: Color32,                // WHAT: the window. WHY: behind everything.
    pub card: Color32,              // WHAT: cards. WHY: raised a step.
    pub card2: Color32,             // WHAT: buttons and inner cards. WHY: a step further.
    pub well: Color32,              // WHAT: search boxes and wells. WHY: sunk a step.
    pub line: Color32,              // WHAT: hairlines. WHY: card edges.
    pub edge: Color32,              // WHAT: control edges. WHY: a button's outline.
    pub ink: Color32,               // WHAT: words. WHY: read first.
    pub muted: Color32,             // WHAT: quieter words. WHY: read second.
    pub faint: Color32,             // WHAT: hints. WHY: read last.
    pub accent: Color32,            // WHAT: the one main action. WHY: one a screen.
    pub accent_ink: Color32,        // WHAT: words on the accent. WHY: readable on it.
    pub accent_soft: Color32, // WHAT: the accent's tint. WHY: the rail's chosen section, tile icons.
    pub gold: Color32,        // WHAT: job numbers. WHY: the company's gold.
    pub green: (Color32, Color32), // WHAT: done or delivered, and its tint. WHY: status colour with words.
    pub amber: (Color32, Color32), // WHAT: to sign. WHY: likewise.
    pub teal: (Color32, Color32),  // WHAT: in transit. WHY: likewise.
    pub purple: (Color32, Color32), // WHAT: revision or new. WHY: likewise.
    pub red: (Color32, Color32),   // WHAT: a problem. WHY: likewise.
    pub go_ink: Color32,           // WHAT: words on the green button. WHY: readable on it.
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8) // WHAT: #RRGGBB as a colour. WHY: the theme file writes them so.
}

/// Dark, the default indoors.
pub const DARK: Palette = Palette {
    bg: hex(0x0B0F19),
    card: hex(0x121826),
    card2: hex(0x161E2F),
    well: hex(0x0E1422),
    line: hex(0x2A3348),
    edge: hex(0x6B7894),
    ink: hex(0xE7EAF2),
    muted: hex(0xA3ACBF),
    faint: hex(0x7D879C),
    accent: hex(0x93A8FF),
    accent_ink: hex(0x0B0F19),
    accent_soft: hex(0x1E2A4D),
    gold: hex(0xD9B877),
    green: (hex(0x4ADE80), hex(0x0F2A1C)),
    amber: (hex(0xFBBF24), hex(0x2B2210)),
    teal: (hex(0x5EEAD4), hex(0x0D2A2A)),
    purple: (hex(0xC4B5FD), hex(0x221B3A)),
    red: (hex(0xF87171), hex(0x2E1416)),
    go_ink: hex(0x052012),
}; // WHAT: Fabwire Modern dark. WHY: the design's table (§5).

/// Bright, for the sun: the highest contrast.
pub const BRIGHT: Palette = Palette {
    bg: hex(0xF5F6F8),
    card: hex(0xFFFFFF),
    card2: hex(0xFFFFFF),
    well: hex(0xFFFFFF),
    line: hex(0xD9DDE5),
    edge: hex(0x78819A),
    ink: hex(0x0F172A),
    muted: hex(0x4B5563),
    faint: hex(0x5B6474),
    accent: hex(0x1F2D86),
    accent_ink: hex(0xFFFFFF),
    accent_soft: hex(0xE4E8F7),
    gold: hex(0x7A5F27),
    green: (hex(0x166534), hex(0xE7F6EC)),
    amber: (hex(0x854D0E), hex(0xFDF3D8)),
    teal: (hex(0x115E59), hex(0xE3F4F2)),
    purple: (hex(0x5B21B6), hex(0xEFEAFB)),
    red: (hex(0xB42318), hex(0xFDECEA)),
    go_ink: hex(0xFFFFFF),
}; // WHAT: Fabwire Modern light. WHY: likewise.

/// The type, Inter, at a size.
fn font(size: f32, bold: bool) -> FontId {
    FontId::new(
        size,
        FontFamily::Name(
            if bold {
                ui::chrome::TABLET_TYPE_BOLD
            } else {
                ui::chrome::TABLET_TYPE
            }
            .into(),
        ),
    ) // WHAT: Inter or Inter Bold. WHY: the company's type
}

/// Which screen is up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    Jobs,        // WHAT: the job list. WHY: the first screen.
    Home,        // WHAT: a job's home. WHY: the banner and the tiles.
    Drawings,    // WHAT: the released sheets. WHY: a section.
    Shipping,    // WHAT: the loads. WHY: a section.
    Ticket(u32), // WHAT: one load's ticket. WHY: the pieces and the signatures.
    Pad,         // WHAT: the signing pad. WHY: one signature.
    Done,        // WHAT: Delivered, or signed. WHY: what the server now has.
    Documents,   // WHAT: the office's pins. WHY: a section.
}

/// The pad, open on one box of one load.
#[derive(Clone, Debug, Default)]
pub struct Pad {
    pub load: u32,                   // WHAT: the load. WHY: its ticket and its words.
    pub which: String, // WHAT: loaded_by, driver or received_by. WHY: the words and the button.
    pub line: String,  // WHAT: the sign-off line's id. WHY: what is signed.
    pub name: String, // WHAT: the signer's name. WHY: empty for the receiver and the driver; theirs for their own box.
    pub strokes: Vec<Vec<[f64; 2]>>, // WHAT: the strokes so far, in the pad's units. WHY: what is sent.
    pub drawing: Vec<[f64; 2]>,      // WHAT: the stroke under the finger. WHY: added when it lifts.
    pub sending: bool,               // WHAT: sent, waiting for the server. WHY: once.
    pub said: Option<String>,        // WHAT: why it didn't go. WHY: said on the pad.
}

/// What was signed, for the Delivered screen: as the server has it.
#[derive(Clone, Debug, Default)]
pub struct Done {
    pub load: u32,                   // WHAT: the load. WHY: named.
    pub which: String,               // WHAT: the box. WHY: "Delivered" for Received by.
    pub signer: String,              // WHAT: the name the server kept. WHY: shown.
    pub signed: String,              // WHAT: when the server kept it. WHY: shown.
    pub strokes: Vec<Vec<[f64; 2]>>, // WHAT: the signature. WHY: drawn small beside the name.
}

/// What the screens ask the window to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Jobs,        // WHAT: read the job list again. WHY: opened, or pulled to refresh.
    Job(String), // WHAT: read one job. WHY: opened, or kept fresh.
    Open {
        set: String,
        name: String,
    }, // WHAT: open a set in View's viewer. WHY: a sheet, a ticket as printed, a pinned set.
    Sign {
        project: String,
        id: String,
        name: String,
        strokes: Vec<Vec<[f64; 2]>>,
    }, // WHAT: sign a box's line. WHY: the existing sign-off route.
    Bright,      // WHAT: the light look on or off. WHY: the top bar's button.
    Desktop,     // WHAT: back to the desktop layout. WHY: only offered on a desktop.
}

/// What the screens need to know about the world, from the window.
#[derive(Clone, Debug, Default)]
pub struct Env {
    pub person: String, // WHAT: who is signed in. WHY: greeted, and their own boxes prefilled.
    pub connected: bool, // WHAT: whether the office server answers. WHY: the pill and the offline words.
    pub bright: bool,    // WHAT: the light look. WHY: the palette.
    pub desktop: bool,   // WHAT: a desktop computer. WHY: offers going back to the desktop layout.
    pub today: String, // WHAT: today, YYYY-MM-DD, on the tablet's clock. WHY: Today / This week, and NEW.
    pub date_words: String, // WHAT: "Mon 11/23/2026 · 2:14 PM". WHY: the pad's date, set by the tablet.
    pub clock: String,      // WHAT: "10:42 AM". WHY: "Up to date with the office".
}

/// The tablet's screens and what they hold.
#[derive(Default)]
pub struct Tablet {
    pub unsupported: bool, // WHAT: the office server is older than 0.8.0. WHY: the desktop layout stays.
    pub desktop: bool, // WHAT: Desktop tapped this session. WHY: a desktop in the shop can still reach the desktop layout; the server's rules hold either way
    pub jobs: Option<Vec<TabletJob>>, // WHAT: the job list, once read. WHY: the first screen.
    pub jobs_at: String, // WHAT: when it was read. WHY: "Up to date with the office · 10:42 AM".
    pub search: String, // WHAT: the job search. WHY: one big box.
    pub job: Option<TabletView>, // WHAT: the open job's manifest. WHY: everything inside a job.
    pub screen: Option<Screen>, // WHAT: the screen up; none is the job list. WHY: one at a time.
    pub sheet_search: String, // WHAT: the sheet search. WHY: a number or a piece mark.
    pub filter: usize, // WHAT: All, New, Shop, Erection or Anchor bolts. WHY: the five buttons.
    pub ship: usize,   // WHAT: Today, This week or All loads. WHY: the three buttons.
    pub seen: BTreeSet<String>, // WHAT: sheets opened here. WHY: NEW until opened.
    pub pad: Option<Pad>, // WHAT: the pad, when somebody signs. WHY: one at a time.
    pub done: Option<Done>, // WHAT: the last signature, for its screen. WHY: Delivered ✓.
    pub said: Option<String>, // WHAT: the last trouble, in words. WHY: never failing quietly.
    pub problem: bool, // WHAT: Report a problem open. WHY: says who to call.
    pub viewing: Option<String>, // WHAT: the set opened in the viewer from here. WHY: its own bar and toolbar.
    pub more: bool, // WHAT: More opened in the viewer. WHY: the rest of the tool chest.
    pub targets: Vec<(Rect, String)>, // WHAT: every tap target drawn this frame. WHY: the tests hold each to 52 points.
    pub asked_at: Option<std::time::Instant>, // WHAT: when the manifest was last asked for. WHY: kept fresh every half minute.
}

/// A tap target: its rect claimed, remembered and asked about.
fn tap(ui: &egui::Ui, targets: &mut Vec<(Rect, String)>, rect: Rect, id: &str) -> egui::Response {
    targets.push((rect, id.to_string())); // WHAT: remembered. WHY: the 52-point rule is tested on what was drawn
    ui.interact(rect, ui.id().with(id), Sense::click()) // WHAT: a click or tap on it. WHY: answered by the caller
}

/// A card: rounded, filled, a hairline edge.
fn card(p: &egui::Painter, rect: Rect, fill: Color32, edge: Color32, width: f32) {
    p.rect(
        rect,
        CornerRadius::same(14),
        fill,
        Stroke::new(width, edge),
        StrokeKind::Inside,
    ); // WHAT: 14-point corners. WHY: Fabwire Modern's 10, scaled for touch
}

/// Words at a place.
fn words(
    p: &egui::Painter,
    at: Pos2,
    align: Align2,
    text: &str,
    size: f32,
    bold: bool,
    color: Color32,
) -> Rect {
    p.text(at, align, text, font(size, bold), color) // WHAT: drawn. WHY: one call for every line of words
}

/// Words wrapped to a width, from the top left: the height they took.
fn wrapped(
    p: &egui::Painter,
    at: Pos2,
    width: f32,
    text: &str,
    size: f32,
    bold: bool,
    color: Color32,
) -> f32 {
    let galley = p.layout(text.to_string(), font(size, bold), color, width); // WHAT: laid out. WHY: long names wrap
    let height = galley.size().y; // WHAT: how tall. WHY: the next line goes under it
    p.galley(at, galley, color); // WHAT: drawn. WHY: shown
    height
}

/// A status chip: words on a tint, never colour alone.
fn chip(p: &egui::Painter, at: Pos2, text: &str, (ink, tint): (Color32, Color32)) -> Rect {
    let galley = p.layout_no_wrap(text.to_string(), font(15.0, true), ink); // WHAT: its words. WHY: the chip fits them
    let rect = Rect::from_min_size(at, Vec2::new(galley.size().x + 24.0, 34.0)); // WHAT: 34 tall, 12 each side. WHY: the mockups' chip
    p.rect_filled(rect, CornerRadius::same(10), tint); // WHAT: the tint. WHY: the colour's meaning
    p.galley(
        Pos2::new(rect.min.x + 12.0, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    ); // WHAT: the words. WHY: the meaning itself
    rect
}

/// How a button looks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Plain, // WHAT: a card-coloured button with an edge. WHY: most buttons
    Main,  // WHAT: the accent. WHY: the one main action
    Go,    // WHAT: green. WHY: Sign and mark delivered
    Off,   // WHAT: turned off. WHY: not ready yet
}

/// A button: painted, tapped, at least 52 points.
fn button(
    ui: &egui::Ui,
    pal: &Palette,
    targets: &mut Vec<(Rect, String)>,
    rect: Rect,
    label: &str,
    look: Look,
    size: f32,
) -> bool {
    let response = tap(ui, targets, rect, label); // WHAT: the tap. WHY: below
    let p = ui.painter(); // WHAT: the painter. WHY: drawn by hand
    let (fill, ink, edge) = match look {
        Look::Plain => (pal.card2, pal.ink, pal.edge), // WHAT: plain. WHY: as the mockups' .btn
        Look::Main => (pal.accent, pal.accent_ink, pal.accent), // WHAT: the accent. WHY: .btn.primary
        Look::Go => (pal.green.0, pal.go_ink, pal.green.0),     // WHAT: green. WHY: .btn.go
        Look::Off => (pal.card2, pal.faint, pal.line),          // WHAT: greyed. WHY: not ready
    };
    let fill = if response.is_pointer_button_down_on() && look != Look::Off {
        fill.gamma_multiply(0.85)
    } else {
        fill
    }; // WHAT: darker while pressed. WHY: the finger sees it took
    p.rect(
        rect,
        CornerRadius::same(if rect.height() >= MAIN { 18 } else { 14 }),
        fill,
        Stroke::new(1.5_f32, edge),
        StrokeKind::Inside,
    ); // WHAT: the button. WHY: .btn and .btn.big
    words(
        p,
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        size,
        true,
        ink,
    ); // WHAT: its words. WHY: centred
    response.clicked() && look != Look::Off // WHAT: tapped, and on. WHY: an off button does nothing
}

/// A chevron, pointing right or left.
fn chevron(p: &egui::Painter, at: Pos2, right: bool, color: Color32) {
    let s = if right { 1.0 } else { -1.0 }; // WHAT: which way. WHY: back and forward
    p.line_segment(
        [
            Pos2::new(at.x - 5.0 * s, at.y - 9.0),
            Pos2::new(at.x + 4.0 * s, at.y),
        ],
        Stroke::new(2.5_f32, color),
    ); // WHAT: the upper stroke. WHY: a ‹ or ›
    p.line_segment(
        [
            Pos2::new(at.x + 4.0 * s, at.y),
            Pos2::new(at.x - 5.0 * s, at.y + 9.0),
        ],
        Stroke::new(2.5_f32, color),
    ); // WHAT: the lower stroke. WHY: likewise
}

/// A tick.
fn tick(p: &egui::Painter, at: Pos2, size: f32, color: Color32) {
    p.line_segment(
        [
            Pos2::new(at.x - size * 0.5, at.y),
            Pos2::new(at.x - size * 0.15, at.y + size * 0.35),
        ],
        Stroke::new(size * 0.14, color),
    ); // WHAT: the short stroke. WHY: ✓
    p.line_segment(
        [
            Pos2::new(at.x - size * 0.15, at.y + size * 0.35),
            Pos2::new(at.x + size * 0.5, at.y - size * 0.35),
        ],
        Stroke::new(size * 0.14, color),
    ); // WHAT: the long stroke. WHY: ✓
}

/// The five sections, their words, and how a tile and the rail name them.
pub const SECTIONS: [(&str, &str); 5] = [
    ("drawings", "Drawings"),
    ("shipping", "Shipping"),
    ("traveler", "Traveler"),
    ("loading", "Loading"),
    ("documents", "Documents"),
]; // WHAT: the order and the words. WHY: the tiles and the rail agree

/// A small line icon for a section, drawn in a square.
fn icon(p: &egui::Painter, section: &str, rect: Rect, color: Color32) {
    let s = Stroke::new(2.0_f32, color); // WHAT: the line. WHY: Lucide-like icons
    let r = rect.shrink(rect.width() * 0.22); // WHAT: the icon inside its square. WHY: room round it
    match section {
        "drawings" => {
            p.rect_stroke(
                r.shrink2(Vec2::new(r.width() * 0.12, 0.0)),
                CornerRadius::same(2),
                s,
                StrokeKind::Inside,
            ); // WHAT: a sheet. WHY: Drawings
            for k in 1..4 {
                let y = r.top() + r.height() * (0.25 + 0.18 * k as f32); // WHAT: a text line's height. WHY: lines on the sheet
                p.line_segment(
                    [
                        Pos2::new(r.left() + r.width() * 0.3, y),
                        Pos2::new(r.right() - r.width() * 0.3, y),
                    ],
                    s,
                ); // WHAT: a line. WHY: the sheet's words
            }
        }
        "shipping" => {
            p.rect_stroke(
                Rect::from_min_max(
                    Pos2::new(r.left(), r.top() + r.height() * 0.2),
                    Pos2::new(r.left() + r.width() * 0.62, r.bottom() - r.height() * 0.25),
                ),
                CornerRadius::same(2),
                s,
                StrokeKind::Inside,
            ); // WHAT: the trailer. WHY: a truck
            p.rect_stroke(
                Rect::from_min_max(
                    Pos2::new(r.left() + r.width() * 0.62, r.top() + r.height() * 0.4),
                    Pos2::new(r.right(), r.bottom() - r.height() * 0.25),
                ),
                CornerRadius::same(2),
                s,
                StrokeKind::Inside,
            ); // WHAT: the cab. WHY: likewise
            for x in [0.22, 0.8] {
                p.circle_stroke(
                    Pos2::new(r.left() + r.width() * x, r.bottom() - r.height() * 0.12),
                    r.width() * 0.1,
                    s,
                ); // WHAT: a wheel. WHY: likewise
            }
        }
        "traveler" => {
            p.rect_stroke(
                r.shrink2(Vec2::new(r.width() * 0.12, 0.0)),
                CornerRadius::same(3),
                s,
                StrokeKind::Inside,
            ); // WHAT: a clipboard. WHY: the traveler
            tick(
                p,
                r.center() + Vec2::new(0.0, r.height() * 0.05),
                r.width() * 0.45,
                color,
            ); // WHAT: a tick on it. WHY: stations signed
        }
        "loading" => {
            let c = r.center(); // WHAT: the middle. WHY: a box drawn round it
            let w = r.width() * 0.45; // WHAT: half its width. WHY: below
            let pts = [
                Pos2::new(c.x, c.y - w),
                Pos2::new(c.x + w, c.y - w * 0.5),
                Pos2::new(c.x + w, c.y + w * 0.5),
                Pos2::new(c.x, c.y + w),
                Pos2::new(c.x - w, c.y + w * 0.5),
                Pos2::new(c.x - w, c.y - w * 0.5),
                Pos2::new(c.x, c.y - w),
            ]; // WHAT: a hexagon. WHY: a crate seen from a corner
            p.line(pts.to_vec(), s); // WHAT: its outline. WHY: Loading
            p.line_segment([Pos2::new(c.x - w, c.y - w * 0.5), c], s); // WHAT: an inner edge. WHY: the box's corner
            p.line_segment([Pos2::new(c.x + w, c.y - w * 0.5), c], s); // WHAT: another. WHY: likewise
            p.line_segment([c, Pos2::new(c.x, c.y + w)], s); // WHAT: the last. WHY: likewise
        }
        _ => {
            let pts = vec![
                Pos2::new(r.left(), r.top() + r.height() * 0.15),
                Pos2::new(r.left() + r.width() * 0.38, r.top() + r.height() * 0.15),
                Pos2::new(r.left() + r.width() * 0.5, r.top() + r.height() * 0.3),
                Pos2::new(r.right(), r.top() + r.height() * 0.3),
                Pos2::new(r.right(), r.bottom() - r.height() * 0.1),
                Pos2::new(r.left(), r.bottom() - r.height() * 0.1),
                Pos2::new(r.left(), r.top() + r.height() * 0.15),
            ]; // WHAT: a folder's outline. WHY: Documents
            p.line(pts, s); // WHAT: drawn. WHY: likewise
        }
    }
}

/// "18,450": a number with thousands marked.
#[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
pub fn thousands(n: u64) -> String {
    let digits = n.to_string(); // WHAT: the digits. WHY: grouped below
    let mut out = String::new(); // WHAT: the result. WHY: built below
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(','); // WHAT: a comma every three from the right. WHY: how a weight is read
        }
        out.push(c); // WHAT: the digit. WHY: kept
    }
    out
}

/// "11/20" from "2026-11-20".
#[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
pub fn short_day(day: &str) -> String {
    if hub::tablet::is_day(day) {
        format!("{}/{}", &day[5..7], &day[8..10])
    } else {
        day.to_string()
    } // WHAT: month/day. WHY: as the shop writes it
}

/// A load's status in words and its colour.
fn status(pal: &Palette, load: &Load) -> (&'static str, (Color32, Color32)) {
    match load.status.as_str() {
        "loading" => ("Loading", pal.purple), // WHAT: still loading. WHY: words and colour
        "loaded" => ("Loaded", pal.amber),    // WHAT: ready to go. WHY: likewise
        "in_transit" => ("In transit", pal.teal), // WHAT: on the road. WHY: likewise
        "delivered" => ("Delivered", pal.green), // WHAT: there. WHY: likewise
        _ => ("Returned", pal.red),           // WHAT: came back. WHY: likewise
    }
}

/// A box's words.
#[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
pub fn box_words(which: &str) -> &'static str {
    match which {
        "loaded_by" => "Loaded by", // WHAT: the shop's box. WHY: the form's words
        "driver" => "Driver",       // WHAT: the driver's. WHY: likewise
        _ => "Received by",         // WHAT: the receiver's. WHY: likewise
    }
}

/// Whether a box is waiting for a signature on this tablet: its line is there and open.
#[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
pub fn open_box(b: &TicketBox, who: Audience) -> bool {
    b.line.as_ref().is_some_and(|l| l.status == "open") && may_sign(&b.which, who) // WHAT: a live open line, and a box this tablet's account may sign. WHY: only a line the server has can be signed, and only by the accounts the owner's rule allows (the server says no to the rest)
}

/// Who is holding this tablet: the shop, the field, or both (the office).
#[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
pub fn holder(job: &TabletView) -> Audience {
    Audience { shop: job.shop, field: job.field } // WHAT: the account's audience as the server named it. WHY: the same rule the server signs by
}

/// Whether a box is signed, in the office or here, and by whom.
fn signed_by(b: &TicketBox) -> Option<(String, String)> {
    match &b.line {
        Some(l) if l.status == "signed" => Some((
            l.signer.clone(),
            l.signed.get(..10).unwrap_or("").to_string(),
        )), // WHAT: signed here, as the server has it. WHY: newer than Fabwire's last pass
        _ if !b.signer.is_empty() => Some((b.signer.clone(), b.signed.clone())), // WHAT: filled in the office. WHY: shown signed
        _ => None, // WHAT: not signed. WHY: shown waiting
    }
}

/// The box this person's big button signs on a load: the field's receiver first, the shop's own boxes first.
#[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
pub fn next_box(load: &Load, field_first: bool, who: Audience) -> Option<&TicketBox> {
    let order: [&str; 3] = if field_first {
        ["received_by", "driver", "loaded_by"]
    } else {
        ["loaded_by", "driver", "received_by"]
    }; // WHAT: who signs first here. WHY: a field tablet is at the receiving end
    order
        .iter()
        .find_map(|w| load.boxes.iter().find(|b| b.which == *w && open_box(b, who))) // WHAT: the first open one in that order. WHY: the one big button
}

/// The sheets a filter and a search leave, by sheet number.
#[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
pub fn sheets_shown<'a>(
    sheets: &'a [Sheet],
    filter: usize,
    search: &str,
    seen: &BTreeSet<String>,
) -> Vec<&'a Sheet> {
    let wanted = search.trim().to_lowercase(); // WHAT: what was typed. WHY: compared case aside
    let mut shown: Vec<&Sheet> = sheets
        .iter()
        .filter(|s| match filter {
            1 => !seen.contains(&s.set), // WHAT: New: not opened here. WHY: the NEW badge
            2 => matches!(s.discipline.as_str(), "structural" | "misc" | "joist_deck"), // WHAT: Shop. WHY: the fabrication sheets
            3 => s.discipline == "erection", // WHAT: Erection. WHY: the field's sheets
            4 => s.discipline == "anchor_bolts", // WHAT: Anchor bolts. WHY: likewise
            _ => true,                       // WHAT: All. WHY: everything released
        })
        .filter(|s| {
            wanted.is_empty()
                || s.sheet.to_lowercase().contains(&wanted)
                || s.title.to_lowercase().contains(&wanted)
                || s.marks.iter().any(|m| m.to_lowercase() == wanted)
        }) // WHAT: the number, the title, or a piece mark on it. WHY: "B12" finds its sheets
        .collect();
    shown.sort_by_cached_key(|s| natural(&s.sheet)); // WHAT: by sheet number, S-2 before S-10. WHY: the list is by number
    shown
}

/// A sort key that reads numbers as numbers.
fn natural(text: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new(); // WHAT: runs of letters, each with the number after it. WHY: compared in order
    let mut letters = String::new(); // WHAT: the letters so far. WHY: below
    let mut number: Option<u64> = None; // WHAT: the number so far. WHY: below
    for c in text.chars() {
        match (c.to_digit(10), number) {
            (Some(d), n) => {
                number = Some(
                    n.unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(u64::from(d)),
                )
            } // WHAT: a digit added. WHY: the number grows
            (None, Some(n)) => {
                out.push((std::mem::take(&mut letters), n)); // WHAT: a run finished. WHY: kept
                number = None; // WHAT: none now. WHY: the next run
                letters.push(c.to_ascii_lowercase()); // WHAT: this letter. WHY: starts the next run
            }
            (None, None) => letters.push(c.to_ascii_lowercase()), // WHAT: a letter. WHY: kept
        }
    }
    out.push((letters, number.unwrap_or(0))); // WHAT: the last run. WHY: kept
    out
}

/// Whether a load ships today, this week, or at all, by the three buttons.
fn load_shown(load: &Load, which: usize, today: &str, who: Audience) -> bool {
    match which {
        0 => {
            load.ships == today
                || load.status == "in_transit"
                || (load.boxes.iter().any(|b| open_box(b, who)) && load.status != "loading")
        } // WHAT: Today: shipping today, on the road, or waiting to be signed. WHY: what needs doing now
        1 => load.ships.is_empty() || within_week(&load.ships, today), // WHAT: This week. WHY: the manifest holds this week's
        _ => true, // WHAT: All loads. WHY: everything sent
    }
}

/// Whether `day` is within seven days of `today`, either way (both YYYY-MM-DD).
fn within_week(day: &str, today: &str) -> bool {
    let n = |d: &str| -> Option<i64> {
        let date = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()?; // WHAT: read. WHY: compared as days
        Some(date.num_days_from_ce().into()) // WHAT: its day number. WHY: subtracted below
    };
    matches!((n(day), n(today)), (Some(a), Some(b)) if (a - b).abs() <= 7) // WHAT: a week either side. WHY: "This week"
}

use chrono::Datelike as _; // WHAT: a date's day number. WHY: within_week

impl Tablet {
    /// The job list, as the server answered.
    pub fn take_jobs(&mut self, jobs: Vec<TabletJob>) {
        self.jobs = Some(jobs); // WHAT: kept. WHY: drawn
        self.jobs_at = now_clock(); // WHAT: when. WHY: "Up to date with the office"
        self.said = None; // WHAT: no trouble now. WHY: it answered
        self.unsupported = false; // WHAT: it has the routes. WHY: it answered them
    }

    /// One job, as the server answered.
    pub fn take_job(&mut self, view: TabletView) {
        if self.screen.is_none() {
            self.screen = Some(Screen::Home); // WHAT: its home. WHY: the job was just opened
        }
        self.job = Some(view); // WHAT: kept. WHY: drawn
        self.said = None; // WHAT: no trouble now. WHY: it answered
    }

    /// Asking didn't work: said.
    pub fn trouble(&mut self, why: String) {
        self.said = Some(why); // WHAT: kept. WHY: shown in a band
    }

    /// A box signed, the job read again: the Delivered screen when the server has it.
    pub fn signed(&mut self, view: TabletView) {
        let pad = self.pad.clone().unwrap_or_default(); // WHAT: what was being signed. WHY: found in the new manifest
        let line = view
            .manifest
            .loads
            .iter()
            .find(|l| l.number == pad.load)
            .and_then(|l| l.boxes.iter().find(|b| b.which == pad.which))
            .and_then(|b| b.line.clone()); // WHAT: its line now. WHY: signed or not, as the server says
        self.job = Some(view); // WHAT: the job as it is now. WHY: every screen agrees
        match line {
            Some(l) if l.status == "signed" => {
                self.done = Some(Done {
                    load: pad.load,
                    which: pad.which,
                    signer: l.signer,
                    signed: l.signed,
                    strokes: pad.strokes,
                }); // WHAT: what the server kept. WHY: the Delivered screen shows the server's, not the pad's
                self.pad = None; // WHAT: the pad put away. WHY: done
                self.screen = Some(Screen::Done); // WHAT: Delivered ✓. WHY: final once the server has it
            }
            _ => self.sign_failed("The office server didn't keep the signature. Try again.".into()), // WHAT: not signed after all. WHY: never a tick the server can't back
        }
    }

    /// Signing didn't go through: said on the pad, which stays open.
    pub fn sign_failed(&mut self, why: String) {
        if let Some(pad) = self.pad.as_mut() {
            pad.sending = false; // WHAT: ready again. WHY: they can try again
            pad.said = Some(why); // WHAT: the reason. WHY: said plainly
        }
    }

    /// Opens the pad on a box.
    fn open_pad(&mut self, load: u32, b: &TicketBox, person: &str) {
        let own = b.which == "loaded_by"; // WHAT: the shop's own box. WHY: their own name; a receiver or driver types theirs
        self.pad = Some(Pad {
            load,
            which: b.which.clone(),
            line: b.line.as_ref().map(|l| l.id.clone()).unwrap_or_default(),
            name: if own {
                person.to_string()
            } else {
                String::new()
            },
            ..Pad::default()
        }); // WHAT: a clean pad. WHY: the name starts empty for Received by and Driver
        self.screen = Some(Screen::Pad); // WHAT: the pad. WHY: signing
    }

    /// Draws the screens, from the job list to Delivered. What the window should do comes back.
    pub fn show(&mut self, ctx: &egui::Context, env: &Env) -> Vec<Action> {
        self.targets.clear(); // WHAT: this frame's targets. WHY: tested afresh
        let pal = if env.bright { BRIGHT } else { DARK }; // WHAT: the look. WHY: dark or bright
        let mut actions = Vec::new(); // WHAT: what to do. WHY: returned
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(pal.bg))
            .show(ctx, |ui| {
                let full = ui.max_rect(); // WHAT: the whole window. WHY: laid out by hand
                let phone = full.width() < PHONE; // WHAT: a phone. WHY: one column
                let body = Rect::from_min_max(Pos2::new(full.left(), full.top() + TOP), full.max); // WHAT: under the top bar. WHY: the screen's own room
                match self.screen.clone() {
                    None | Some(Screen::Jobs) => {
                        self.top_bar(ui, &pal, env, full, None, "", "", &mut actions); // WHAT: the brand bar. WHY: the first screen
                        self.jobs_screen(ui, &pal, env, body, phone, &mut actions);
                        // WHAT: the list. WHY: the first screen
                    }
                    Some(Screen::Pad) => self.pad_screen(ui, &pal, env, full, phone, &mut actions), // WHAT: the pad, its own bar. WHY: nothing else on screen while signing
                    Some(screen) => {
                        let Some(job) = self.job.clone() else {
                            self.screen = None; // WHAT: no job any more. WHY: back to the list
                            return;
                        };
                        let m = &job.manifest; // WHAT: the job's manifest. WHY: every section reads it
                        let (back, kicker, title) = match &screen {
                            Screen::Home => {
                                ("Jobs", format!("JOB {}", m.job.number), m.job.name.clone())
                            } // WHAT: the job's home bar. WHY: as mockup b
                            Screen::Ticket(n) => (
                                "Shipping",
                                format!(
                                    "{} · {} · SHIPPING TICKET {}",
                                    m.job.number,
                                    m.job.name.to_uppercase(),
                                    m.loads
                                        .iter()
                                        .find(|l| l.number == *n)
                                        .map(|l| l.ticket.clone())
                                        .unwrap_or_default()
                                ),
                                self.load_title(*n),
                            ), // WHAT: the ticket's bar. WHY: as mockup f
                            Screen::Done => (
                                "Shipping",
                                format!("{} · {}", m.job.number, m.job.name.to_uppercase()),
                                "Signed".to_string(),
                            ), // WHAT: the Delivered bar. WHY: as mockup h
                            other => (
                                m.job.number.as_str(),
                                m.job.name.to_uppercase(),
                                SECTIONS
                                    .iter()
                                    .find(|(k, _)| section_of(other) == *k)
                                    .map_or("", |(_, w)| *w)
                                    .to_string(),
                            ), // WHAT: a section's bar. WHY: as mockup c
                        };
                        let back = back.to_string(); // WHAT: owned. WHY: m is borrowed below
                        self.top_bar(
                            ui,
                            &pal,
                            env,
                            full,
                            Some(back.as_str()),
                            &kicker,
                            &title,
                            &mut actions,
                        ); // WHAT: the bar. WHY: back, where, and who
                        let main = if matches!(
                            screen,
                            Screen::Drawings | Screen::Shipping | Screen::Documents
                        ) && !phone
                        {
                            self.rail(ui, &pal, &job, body, &screen); // WHAT: the sections down the left. WHY: one tap between sections
                            Rect::from_min_max(Pos2::new(body.left() + RAIL, body.top()), body.max)
                        // WHAT: the room beside it. WHY: the section's own
                        } else {
                            body // WHAT: the whole body. WHY: home, the ticket and Delivered have no rail
                        };
                        match screen {
                            Screen::Home => {
                                self.home_screen(ui, &pal, &job, main, phone, &mut actions)
                            } // WHAT: the banner and the tiles. WHY: mockup b
                            Screen::Drawings => {
                                self.drawings_screen(ui, &pal, &job, main, phone, &mut actions)
                            } // WHAT: the sheets. WHY: mockup c
                            Screen::Shipping => {
                                self.shipping_screen(ui, &pal, env, &job, main, phone)
                            } // WHAT: the loads. WHY: mockup e
                            Screen::Ticket(n) => self.ticket_screen(
                                ui,
                                &pal,
                                env,
                                &job,
                                n,
                                main,
                                phone,
                                &mut actions,
                            ), // WHAT: the ticket. WHY: mockup f
                            Screen::Done => self.done_screen(ui, &pal, &job, main), // WHAT: Delivered ✓. WHY: mockup h
                            Screen::Documents => {
                                self.documents_screen(ui, &pal, &job, main, &mut actions)
                            } // WHAT: the pins. WHY: design §2
                            Screen::Jobs | Screen::Pad => {} // WHAT: drawn above. WHY: never here
                        }
                    }
                }
                if let Some(said) = self.said.clone() {
                    let band = Rect::from_min_size(
                        Pos2::new(full.left() + 20.0, full.bottom() - 76.0),
                        Vec2::new(full.width() - 40.0, 60.0),
                    ); // WHAT: a band at the bottom. WHY: seen, out of the way
                    card(ui.painter(), band, pal.red.1, pal.red.0, 1.5); // WHAT: red. WHY: a problem
                    wrapped(
                        ui.painter(),
                        band.min + Vec2::new(18.0, 10.0),
                        band.width() - 150.0,
                        &said,
                        16.0,
                        true,
                        pal.red.0,
                    ); // WHAT: the words. WHY: never failing quietly
                    if button(
                        ui,
                        &pal,
                        &mut self.targets,
                        Rect::from_min_size(
                            Pos2::new(band.right() - 124.0, band.top() + 4.0),
                            Vec2::new(116.0, SMALLEST),
                        ),
                        "Try again",
                        Look::Plain,
                        16.0,
                    ) {
                        self.said = None; // WHAT: cleared. WHY: asked again below
                        actions.push(match &self.job {
                            Some(j) if self.screen.is_some() => Action::Job(j.project.clone()),
                            _ => Action::Jobs,
                        }); // WHAT: asked again. WHY: the connection may be back
                    }
                }
            });
        actions
    }

    /// "Load 2 · 18,450 lb · Flatbed 48'".
    fn load_title(&self, n: u32) -> String {
        let load = self
            .job
            .as_ref()
            .and_then(|j| j.manifest.loads.iter().find(|l| l.number == n)); // WHAT: the load. WHY: its words
        load.map(|l| {
            format!(
                "Load {} · {} lb{}",
                l.number,
                thousands(l.weight_lb),
                if l.trailer.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", l.trailer)
                }
            )
        })
        .unwrap_or_default() // WHAT: number, weight, trailer. WHY: the ticket's title
    }

    /// The top bar: back or the brand, where, the connection, Bright, and who.
    #[allow(clippy::too_many_arguments)] // WHAT: the bar's parts each passed. WHY: one bar, every screen
    fn top_bar(
        &mut self,
        ui: &egui::Ui,
        pal: &Palette,
        env: &Env,
        full: Rect,
        back: Option<&str>,
        kicker: &str,
        title: &str,
        actions: &mut Vec<Action>,
    ) {
        let p = ui.painter().clone(); // WHAT: the painter. WHY: drawn by hand
        let bar = Rect::from_min_size(full.min, Vec2::new(full.width(), TOP)); // WHAT: the bar. WHY: 68 tall
        p.rect_filled(bar, CornerRadius::ZERO, pal.bg); // WHAT: its fill. WHY: as the window
        p.line_segment(
            [bar.left_bottom(), bar.right_bottom()],
            Stroke::new(1.0_f32, pal.line),
        ); // WHAT: its edge. WHY: a hairline
        let mut x = bar.left() + 20.0; // WHAT: where the next part goes. WHY: left to right
        let phone = full.width() < PHONE; // WHAT: a phone. WHY: the bar sheds words
        if let Some(back) = back {
            let w = (back.chars().count() as f32 * 11.0 + 56.0).max(SMALLEST); // WHAT: as wide as its words. WHY: at least 52
            let rect = Rect::from_min_size(Pos2::new(x, bar.top() + 8.0), Vec2::new(w, SMALLEST)); // WHAT: the back button. WHY: 52 tall
            let response = tap(ui, &mut self.targets, rect, "back"); // WHAT: tapped. WHY: one level up
            card(&p, rect, pal.card, pal.line, 1.0); // WHAT: its card. WHY: as .back
            chevron(
                &p,
                Pos2::new(rect.left() + 22.0, rect.center().y),
                false,
                pal.accent,
            ); // WHAT: ‹. WHY: back
            words(
                &p,
                Pos2::new(rect.left() + 38.0, rect.center().y),
                Align2::LEFT_CENTER,
                back,
                18.0,
                true,
                pal.accent,
            ); // WHAT: where it goes. WHY: plain
            if response.clicked() {
                self.back(actions); // WHAT: up one level. WHY: back
            }
            x = rect.right() + 16.0; // WHAT: past it. WHY: the title next
            words(
                &p,
                Pos2::new(x, bar.top() + 14.0),
                Align2::LEFT_TOP,
                kicker,
                12.0,
                true,
                pal.gold,
            ); // WHAT: the kicker, gold. WHY: which job
            let room = (full.right() - 260.0 - x).max(80.0); // WHAT: what's left for the title. WHY: never under the pills
            let galley = p.layout(title.to_string(), font(22.0, true), pal.ink, room); // WHAT: the title. WHY: wrapped if it must
            p.galley(Pos2::new(x, bar.top() + 30.0), galley, pal.ink); // WHAT: drawn. WHY: where you are
        } else {
            words(
                &p,
                Pos2::new(x, bar.top() + 12.0),
                Align2::LEFT_TOP,
                "Excalibur View",
                20.0,
                true,
                pal.ink,
            ); // WHAT: the brand. WHY: the first screen
            words(
                &p,
                Pos2::new(x, bar.top() + 40.0),
                Align2::LEFT_TOP,
                "SHOP & FIELD",
                10.0,
                true,
                pal.gold,
            ); // WHAT: the mode. WHY: says what this is
        }
        let mut right = bar.right() - 20.0; // WHAT: from the right. WHY: who, Bright, the connection
        let me = Rect::from_min_size(
            Pos2::new(right - SMALLEST, bar.top() + 8.0),
            Vec2::splat(SMALLEST),
        ); // WHAT: the person's initials. WHY: who is signed in
        p.circle_filled(me.center(), 24.0, pal.accent_soft); // WHAT: a round badge. WHY: as .me
        words(
            &p,
            me.center(),
            Align2::CENTER_CENTER,
            &initials(&env.person),
            17.0,
            true,
            pal.accent,
        ); // WHAT: their initials. WHY: at a glance
        right = me.left() - 12.0; // WHAT: past it. WHY: Bright next
        let bright = Rect::from_min_size(
            Pos2::new(right - 96.0, bar.top() + 8.0),
            Vec2::new(96.0, SMALLEST),
        ); // WHAT: the Bright button. WHY: one button switches the look
        if button(
            ui,
            pal,
            &mut self.targets,
            bright,
            if env.bright { "Dark" } else { "Bright" },
            Look::Plain,
            16.0,
        ) {
            actions.push(Action::Bright); // WHAT: switched. WHY: kept in prefs by the window
        }
        right = bright.left() - 12.0; // WHAT: past it. WHY: the connection next
        if env.desktop && back.is_none() {
            let desk = Rect::from_min_size(
                Pos2::new(right - 110.0, bar.top() + 8.0),
                Vec2::new(110.0, SMALLEST),
            ); // WHAT: back to the desktop layout. WHY: only on a desktop, only on the first screen
            if button(
                ui,
                pal,
                &mut self.targets,
                desk,
                "Desktop",
                Look::Plain,
                16.0,
            ) {
                actions.push(Action::Desktop); // WHAT: asked. WHY: the window turns tablet mode off
            }
            right = desk.left() - 12.0; // WHAT: past it. WHY: the connection next
        }
        if !phone {
            let (ink, tint, label) = if env.connected {
                (pal.green.0, pal.green.1, "Connected")
            } else {
                (pal.red.0, pal.red.1, "No connection")
            }; // WHAT: the connection in words and colour. WHY: never colour alone
            let pill = Rect::from_min_size(
                Pos2::new(right - 150.0, bar.top() + 14.0),
                Vec2::new(150.0, 40.0),
            ); // WHAT: the pill. WHY: as .status
            p.rect_filled(pill, CornerRadius::same(20), tint); // WHAT: its tint. WHY: the colour
            p.circle_filled(Pos2::new(pill.left() + 20.0, pill.center().y), 4.5, ink); // WHAT: the dot. WHY: as .status i
            words(
                &p,
                Pos2::new(pill.left() + 32.0, pill.center().y),
                Align2::LEFT_CENTER,
                label,
                15.0,
                true,
                ink,
            ); // WHAT: the words. WHY: never colour alone
        }
    }

    /// One level up.
    fn back(&mut self, actions: &mut Vec<Action>) {
        self.screen = match self.screen.take() {
            Some(Screen::Ticket(_) | Screen::Done) => Some(Screen::Shipping), // WHAT: back to the loads. WHY: the ticket's parent
            None | Some(Screen::Home | Screen::Jobs) => {
                self.job = None; // WHAT: the job closed. WHY: back to the list
                actions.push(Action::Jobs); // WHAT: the list read again. WHY: what needs doing may have changed
                None
            }
            Some(Screen::Pad) => self.pad.as_ref().map(|p| Screen::Ticket(p.load)), // WHAT: back to its ticket. WHY: cancelled
            Some(_) => Some(Screen::Home), // WHAT: a section's parent. WHY: the job's home
        };
        if !matches!(self.screen, Some(Screen::Pad)) {
            self.pad = None; // WHAT: the pad put away. WHY: unsigned
        }
    }

    /// The job list: mockup a.
    fn jobs_screen(
        &mut self,
        ui: &mut egui::Ui,
        pal: &Palette,
        env: &Env,
        body: Rect,
        phone: bool,
        actions: &mut Vec<Action>,
    ) {
        let p = ui.painter().clone(); // WHAT: the painter. WHY: drawn by hand
        let pad = if phone { 16.0 } else { 30.0 }; // WHAT: the margin. WHY: a phone has less room
        let mut y = body.top() + 26.0; // WHAT: where the next part goes. WHY: top to bottom
        words(
            &p,
            Pos2::new(body.left() + pad, y),
            Align2::LEFT_TOP,
            "Your jobs",
            30.0,
            true,
            pal.ink,
        ); // WHAT: the title. WHY: as mockup a
        y += 42.0; // WHAT: under it. WHY: the greeting next
        let jobs = self.jobs.clone().unwrap_or_default(); // WHAT: the list. WHY: drawn below
        let first = env.person.split_whitespace().next().unwrap_or(""); // WHAT: their first name. WHY: greeted by it
        let greeting = match (self.jobs.is_some(), jobs.len()) {
            (false, _) => "Asking the office for your jobs…".to_string(), // WHAT: not answered yet. WHY: said
            (true, 1) => format!("{} {first}. 1 job is assigned to you.", greet()), // WHAT: one. WHY: plain
            (true, n) => format!("{} {first}. {n} jobs are assigned to you.", greet()), // WHAT: several. WHY: plain
        };
        words(
            &p,
            Pos2::new(body.left() + pad, y),
            Align2::LEFT_TOP,
            &greeting,
            17.0,
            false,
            pal.muted,
        ); // WHAT: the greeting. WHY: as mockup a
        y += 36.0; // WHAT: under it. WHY: the search next
        let search = Rect::from_min_size(
            Pos2::new(body.left() + pad, y),
            Vec2::new(body.width() - pad * 2.0, 56.0),
        ); // WHAT: the search box. WHY: one big box, never a drop-down
        self.search_box(ui, pal, search, "jobs", "Find a job by number or name"); // WHAT: drawn. WHY: typed into
        y = search.bottom() + 16.0; // WHAT: under it. WHY: the cards next
        let wanted = self.search.trim().to_lowercase(); // WHAT: what was typed. WHY: compared case aside
        let shown: Vec<&TabletJob> = jobs
            .iter()
            .filter(|j| {
                wanted.is_empty()
                    || j.number.to_lowercase().contains(&wanted)
                    || j.name.to_lowercase().contains(&wanted)
            })
            .collect(); // WHAT: the jobs it matches. WHY: by number or name
        if self.jobs.is_some() && jobs.is_empty() {
            wrapped(&p, Pos2::new(body.left() + pad, y + 20.0), body.width() - pad * 2.0, "No jobs on your tablet yet. The office puts jobs on the tablets in Fabwire, on each job's Tablets tab.", 18.0, false, pal.muted);
            // WHAT: nothing assigned. WHY: said plainly, with where it is done
        }
        let columns = if phone { 1 } else { 2 }; // WHAT: one or two columns. WHY: as the mockups
        let gap = 16.0; // WHAT: between cards. WHY: room for gloves
        let w = (body.width() - pad * 2.0 - gap * (columns as f32 - 1.0)) / columns as f32; // WHAT: a card's width. WHY: fills the row
        let h = 200.0; // WHAT: a card's height. WHY: as mockup a
        let list = Rect::from_min_max(
            Pos2::new(body.left(), y),
            Pos2::new(body.right(), body.bottom() - 40.0),
        ); // WHAT: the cards' room. WHY: scrolls if long
        ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("tablet-jobs")
                .show(ui, |ui| {
                    let rows = shown.len().div_ceil(columns); // WHAT: how many rows. WHY: the scroll's height
                    let (area, _) = ui.allocate_exact_size(
                        Vec2::new(list.width(), rows as f32 * (h + gap)),
                        Sense::hover(),
                    ); // WHAT: room for every card. WHY: scrolled
                    for (i, job) in shown.iter().enumerate() {
                        let at = Pos2::new(
                            area.left() + pad + (i % columns) as f32 * (w + gap),
                            area.top() + (i / columns) as f32 * (h + gap),
                        ); // WHAT: its place. WHY: in the grid
                        let rect = Rect::from_min_size(at, Vec2::new(w, h)); // WHAT: the card. WHY: one tap opens the job
                        let response =
                            tap(ui, &mut self.targets, rect, &format!("job-{}", job.project)); // WHAT: tapped. WHY: opens it
                        let painter = ui.painter(); // WHAT: inside the scroll. WHY: clipped to it
                        card(
                            painter,
                            rect,
                            pal.card,
                            if response.hovered() {
                                pal.accent
                            } else {
                                pal.line
                            },
                            if response.hovered() { 2.0 } else { 1.0 },
                        ); // WHAT: the card. WHY: as .card
                        words(
                            painter,
                            rect.min + Vec2::new(22.0, 22.0),
                            Align2::LEFT_TOP,
                            &job.number,
                            16.0,
                            true,
                            pal.gold,
                        ); // WHAT: the number in gold. WHY: as mockup a
                        chevron(
                            painter,
                            Pos2::new(rect.right() - 34.0, rect.top() + 32.0),
                            true,
                            pal.faint,
                        ); // WHAT: ›. WHY: it opens
                        let name_h = wrapped(
                            painter,
                            rect.min + Vec2::new(22.0, 50.0),
                            w - 60.0,
                            &job.name,
                            24.0,
                            true,
                            pal.ink,
                        ); // WHAT: its name. WHY: the card's title
                        let who = match (job.shop, job.field) {
                            (true, true) => "Field + Shop",
                            (true, false) => "Shop",
                            (false, true) => "Field",
                            _ => "Office",
                        }; // WHAT: shop, field or both. WHY: on the card
                        let place = if job.town.is_empty() {
                            who.to_string()
                        } else {
                            format!("{} · {who}", job.town)
                        }; // WHAT: the town and who. WHY: as mockup a
                        words(
                            painter,
                            rect.min + Vec2::new(22.0, 56.0 + name_h),
                            Align2::LEFT_TOP,
                            &place,
                            16.0,
                            false,
                            pal.muted,
                        ); // WHAT: drawn. WHY: as mockup a
                        let mut cx = rect.left() + 22.0; // WHAT: where the next chip goes. WHY: left to right
                        let cy = rect.bottom() - 54.0; // WHAT: the chips' row. WHY: at the card's foot
                        if job.todo.is_empty() {
                            chip(painter, Pos2::new(cx, cy), "✓ Nothing to do", pal.green);
                            // WHAT: green. WHY: nothing waits
                        }
                        for todo in job.todo.iter().take(2) {
                            let r = chip(painter, Pos2::new(cx, cy), todo, pal.amber); // WHAT: what needs doing, in words. WHY: as mockup a
                            cx = r.right() + 10.0; // WHAT: past it. WHY: the next
                        }
                        if response.clicked() {
                            self.screen = None; // WHAT: the home comes up once it answers. WHY: take_job
                            self.job = None; // WHAT: no stale job. WHY: another was open
                            actions.push(Action::Job(job.project.clone())); // WHAT: read it. WHY: its home
                        }
                    }
                });
        });
        let foot = body.bottom() - 26.0; // WHAT: the footer's line. WHY: at the bottom
        if self.jobs.is_some() {
            tick(
                &p,
                Pos2::new(body.left() + pad + 8.0, foot),
                16.0,
                pal.green.0,
            ); // WHAT: a tick. WHY: up to date
            words(
                &p,
                Pos2::new(body.left() + pad + 26.0, foot),
                Align2::LEFT_CENTER,
                &format!("Up to date with the office · {}", self.jobs_at),
                14.0,
                false,
                pal.faint,
            ); // WHAT: when. WHY: as mockup a
        }
        if !phone {
            words(
                &p,
                Pos2::new(body.right() - pad, foot),
                Align2::RIGHT_CENTER,
                "Only jobs the office assigned to you are shown.",
                14.0,
                false,
                pal.faint,
            ); // WHAT: why only these. WHY: as mockup a
        }
    }

    /// A search box: one big field.
    fn search_box(
        &mut self,
        ui: &mut egui::Ui,
        pal: &Palette,
        rect: Rect,
        which: &str,
        hint: &str,
    ) {
        let p = ui.painter().clone(); // WHAT: the painter. WHY: drawn by hand
        p.rect(
            rect,
            CornerRadius::same(14),
            pal.well,
            Stroke::new(1.5_f32, pal.edge),
            StrokeKind::Inside,
        ); // WHAT: the well. WHY: as .search
        p.circle_stroke(
            Pos2::new(rect.left() + 28.0, rect.center().y - 2.0),
            8.0,
            Stroke::new(2.0_f32, pal.faint),
        ); // WHAT: a lens. WHY: a search
        p.line_segment(
            [
                Pos2::new(rect.left() + 34.0, rect.center().y + 4.0),
                Pos2::new(rect.left() + 40.0, rect.center().y + 10.0),
            ],
            Stroke::new(2.0_f32, pal.faint),
        ); // WHAT: its handle. WHY: likewise
        self.targets.push((rect, format!("search-{which}"))); // WHAT: remembered. WHY: 56 tall, tested
        let text = if which == "jobs" {
            &mut self.search
        } else {
            &mut self.sheet_search
        }; // WHAT: which search. WHY: two boxes
        let edit = egui::TextEdit::singleline(text)
            .font(font(18.0, false))
            .hint_text(
                egui::RichText::new(hint)
                    .color(pal.faint)
                    .font(font(18.0, false)),
            )
            .text_color(pal.ink)
            .frame(false); // WHAT: the field itself, unframed. WHY: drawn inside the well
        ui.put(
            Rect::from_min_max(
                Pos2::new(rect.left() + 56.0, rect.top() + 6.0),
                Pos2::new(rect.right() - 16.0, rect.bottom() - 6.0),
            ),
            edit,
        ); // WHAT: placed in the well. WHY: typed into
    }

    /// The sections down the left, each 88 tall, with a count of what waits.
    fn rail(
        &mut self,
        ui: &egui::Ui,
        pal: &Palette,
        job: &TabletView,
        body: Rect,
        screen: &Screen,
    ) {
        let p = ui.painter().clone(); // WHAT: the painter. WHY: drawn by hand
        p.line_segment(
            [
                Pos2::new(body.left() + RAIL, body.top()),
                Pos2::new(body.left() + RAIL, body.bottom()),
            ],
            Stroke::new(1.0_f32, pal.line),
        ); // WHAT: its edge. WHY: a hairline
        let mut y = body.top() + 14.0; // WHAT: the first button. WHY: top to bottom
        for (key, label) in on_sections(job) {
            let rect = Rect::from_min_size(
                Pos2::new(body.left() + 10.0, y),
                Vec2::new(RAIL - 20.0, 88.0),
            ); // WHAT: the button. WHY: 88 tall
            let response = tap(ui, &mut self.targets, rect, &format!("rail-{key}")); // WHAT: tapped. WHY: opens the section
            let on = section_of(screen) == key; // WHAT: the section up. WHY: marked
            if on {
                p.rect_filled(rect, CornerRadius::same(14), pal.accent_soft); // WHAT: the tint. WHY: where you are
            }
            let color = if on { pal.accent } else { pal.muted }; // WHAT: its colour. WHY: as .rail a
            icon(
                &p,
                key,
                Rect::from_center_size(
                    Pos2::new(rect.center().x, rect.top() + 32.0),
                    Vec2::splat(36.0),
                ),
                color,
            ); // WHAT: its icon. WHY: found by eye
            words(
                &p,
                Pos2::new(rect.center().x, rect.bottom() - 20.0),
                Align2::CENTER_CENTER,
                label,
                14.0,
                true,
                color,
            ); // WHAT: its name. WHY: never an icon alone
            let waiting = waiting(job, key); // WHAT: what waits in it. WHY: the amber dot
            if waiting > 0 {
                let dot = Pos2::new(rect.right() - 20.0, rect.top() + 18.0); // WHAT: the dot's place. WHY: top right
                p.circle_filled(dot, 11.0, hex(0xFBBF24)); // WHAT: amber. WHY: to sign
                words(
                    &p,
                    dot,
                    Align2::CENTER_CENTER,
                    &waiting.to_string(),
                    12.0,
                    true,
                    hex(0x1B1405),
                ); // WHAT: the count. WHY: how many
            }
            if response.clicked() {
                self.open_section(key); // WHAT: there. WHY: one tap between sections
            }
            y += 96.0; // WHAT: the next. WHY: 8 between
        }
    }

    /// Goes to a section.
    fn open_section(&mut self, key: &str) {
        self.screen = Some(match key {
            "drawings" => Screen::Drawings,   // WHAT: the sheets. WHY: the tile
            "shipping" => Screen::Shipping,   // WHAT: the loads. WHY: the tile
            "documents" => Screen::Documents, // WHAT: the pins. WHY: the tile
            _ => Screen::Home, // WHAT: Traveler and Loading open their form. WHY: their own screens are phase 2
        });
    }

    /// The job's home: the banner and the tiles (mockup b).
    fn home_screen(
        &mut self,
        ui: &egui::Ui,
        pal: &Palette,
        job: &TabletView,
        body: Rect,
        phone: bool,
        actions: &mut Vec<Action>,
    ) {
        let p = ui.painter().clone(); // WHAT: the painter. WHY: drawn by hand
        let m = &job.manifest; // WHAT: the manifest. WHY: every tile reads it
        let pad = if phone { 16.0 } else { 30.0 }; // WHAT: the margin. WHY: a phone has less room
        let mut y = body.top() + 22.0; // WHAT: the banner's top. WHY: top to bottom
        if let Some(todo) = m.todo.first() {
            let banner = Rect::from_min_size(
                Pos2::new(body.left() + pad, y),
                Vec2::new(body.width() - pad * 2.0, if phone { 170.0 } else { 100.0 }),
            ); // WHAT: the banner. WHY: the most urgent thing
            card(&p, banner, pal.amber.1, pal.amber.0, 1.5); // WHAT: amber. WHY: something to sign
            p.rect_filled(
                Rect::from_min_size(banner.min + Vec2::new(22.0, 22.0), Vec2::splat(56.0)),
                CornerRadius::same(14),
                pal.amber.0,
            ); // WHAT: the pencil's square. WHY: as mockup b
            p.line_segment(
                [
                    banner.min + Vec2::new(38.0, 62.0),
                    banner.min + Vec2::new(62.0, 38.0),
                ],
                Stroke::new(4.0_f32, hex(0x1B1405)),
            ); // WHAT: a pencil. WHY: to sign
            let room = if phone {
                banner.width() - 110.0
            } else {
                banner.width() - 330.0
            }; // WHAT: room for the words. WHY: beside the button
            let h = wrapped(
                &p,
                banner.min + Vec2::new(96.0, 20.0),
                room,
                &todo.words,
                24.0,
                true,
                pal.ink,
            ); // WHAT: what to do. WHY: in words
            wrapped(
                &p,
                banner.min + Vec2::new(96.0, 26.0 + h),
                room,
                &todo.detail,
                16.0,
                false,
                pal.muted,
            ); // WHAT: one more line. WHY: as mockup b
            let label = if todo.load.is_some() {
                "Open ticket"
            } else {
                "Open"
            }; // WHAT: the button's words. WHY: what it opens
            let rect = if phone {
                Rect::from_min_size(
                    Pos2::new(banner.left() + 22.0, banner.bottom() - 70.0),
                    Vec2::new(banner.width() - 44.0, 56.0),
                )
            } else {
                Rect::from_min_size(
                    Pos2::new(banner.right() - 214.0, banner.top() + 18.0),
                    Vec2::new(192.0, 64.0),
                )
            }; // WHAT: the button. WHY: one tap to it
            if button(ui, pal, &mut self.targets, rect, label, Look::Main, 20.0) {
                match todo.load {
                    Some(n) => self.screen = Some(Screen::Ticket(n)), // WHAT: the ticket. WHY: "Open ticket"
                    None => self.open_section(&todo.section), // WHAT: its section. WHY: "Open"
                }
            }
            y = banner.bottom() + 20.0; // WHAT: under it. WHY: the tiles next
        }
        let columns = if phone { 1 } else { 3 }; // WHAT: three tiles a row. WHY: as mockup b
        let gap = 16.0; // WHAT: between tiles. WHY: room for gloves
        let w = (body.width() - pad * 2.0 - gap * (columns as f32 - 1.0)) / columns as f32; // WHAT: a tile's width. WHY: fills the row
        let h = if phone {
            150.0
        } else {
            ((body.bottom() - y - 20.0 - gap) / 2.0).clamp(150.0, 260.0)
        }; // WHAT: a tile's height. WHY: two rows fill the screen
        let mut tiles: Vec<(&str, &str)> = on_sections(job); // WHAT: the tiles on for them. WHY: only theirs
        tiles.push(("job", "")); // WHAT: the "This job" card last. WHY: as mockup b
        for (i, (key, label)) in tiles.iter().enumerate() {
            let at = Pos2::new(
                body.left() + pad + (i % columns) as f32 * (w + gap),
                y + (i / columns) as f32 * (h + gap),
            ); // WHAT: its place. WHY: in the grid
            let rect = Rect::from_min_size(at, Vec2::new(w, h)); // WHAT: the tile. WHY: one tap opens it
            if *key == "job" {
                p.rect(
                    rect,
                    CornerRadius::same(14),
                    Color32::TRANSPARENT,
                    Stroke::new(1.0_f32, pal.line),
                    StrokeKind::Inside,
                ); // WHAT: an outline only. WHY: words, not a button
                words(
                    &p,
                    rect.min + Vec2::new(22.0, 22.0),
                    Align2::LEFT_TOP,
                    "THIS JOB",
                    13.0,
                    true,
                    pal.muted,
                ); // WHAT: its label. WHY: as mockup b
                let mut ly = rect.top() + 50.0; // WHAT: the first line. WHY: top to bottom
                for (k, v) in [
                    ("GC", &m.job.gc),
                    ("Site", &m.job.site),
                    ("Office", &m.job.office),
                ] {
                    if !v.is_empty() {
                        ly += wrapped(
                            &p,
                            Pos2::new(rect.left() + 22.0, ly),
                            w - 44.0,
                            &format!("{k}: {v}"),
                            17.0,
                            false,
                            pal.ink,
                        ) + 6.0; // WHAT: one line. WHY: who to ask, where to go
                    }
                }
                chip(
                    &p,
                    Pos2::new(rect.left() + 22.0, rect.bottom() - 54.0),
                    "✓ Released sheets only",
                    pal.green,
                ); // WHAT: the AISC promise. WHY: as mockup b
                continue; // WHAT: no tap. WHY: words only
            }
            let response = tap(ui, &mut self.targets, rect, &format!("tile-{key}")); // WHAT: tapped. WHY: opens the section
            card(
                &p,
                rect,
                pal.card,
                if response.hovered() {
                    pal.accent
                } else {
                    pal.line
                },
                1.0,
            ); // WHAT: the tile. WHY: as .card
            let square = Rect::from_min_size(rect.min + Vec2::new(22.0, 22.0), Vec2::splat(56.0)); // WHAT: the icon's square. WHY: as mockup b
            p.rect_filled(square, CornerRadius::same(14), pal.accent_soft); // WHAT: its tint. WHY: likewise
            icon(&p, key, square, pal.accent); // WHAT: the icon. WHY: found by eye
            chevron(
                &p,
                Pos2::new(rect.right() - 34.0, rect.top() + 40.0),
                true,
                pal.faint,
            ); // WHAT: ›. WHY: it opens
            words(
                &p,
                rect.min + Vec2::new(22.0, 92.0),
                Align2::LEFT_TOP,
                label,
                24.0,
                true,
                pal.ink,
            ); // WHAT: the section's name. WHY: as mockup b
            let (line, badge) = tile_words(job, key, pal, &self.seen); // WHAT: what it holds and what waits. WHY: counts on every tile
            wrapped(
                &p,
                rect.min + Vec2::new(22.0, 126.0),
                w - 44.0,
                &line,
                16.0,
                false,
                pal.muted,
            ); // WHAT: what it holds. WHY: as mockup b
            if let Some((text, colors)) = badge {
                if h > 200.0 {
                    chip(
                        &p,
                        Pos2::new(rect.left() + 22.0, rect.bottom() - 54.0),
                        &text,
                        colors,
                    ); // WHAT: what waits. WHY: as mockup b
                }
            }
            if response.clicked() {
                match *key {
                    "traveler" => {
                        if let Some(t) = &m.traveler {
                            actions.push(Action::Open {
                                set: t.set.clone(),
                                name: "Traveler".into(),
                            }); // WHAT: the traveler form, with its sign-off list. WHY: its own screen is phase 2
                        }
                    }
                    "loading" => {
                        if let Some(l) = &m.loading {
                            actions.push(Action::Open {
                                set: l.set.clone(),
                                name: "Loading log".into(),
                            }); // WHAT: the loading log. WHY: likewise
                        }
                    }
                    other => self.open_section(other), // WHAT: the section. WHY: tapped
                }
            }
        }
    }

    /// The released sheets: mockup c.
    fn drawings_screen(
        &mut self,
        ui: &mut egui::Ui,
        pal: &Palette,
        job: &TabletView,
        body: Rect,
        phone: bool,
        actions: &mut Vec<Action>,
    ) {
        let p = ui.painter().clone(); // WHAT: the painter. WHY: drawn by hand
        let pad = if phone { 16.0 } else { 26.0 }; // WHAT: the margin. WHY: as .main
        let sheets = &job.manifest.sheets; // WHAT: the released sheets. WHY: the only drawings here
        let search = Rect::from_min_size(
            Pos2::new(body.left() + pad, body.top() + 22.0),
            Vec2::new(body.width() - pad * 2.0, 56.0),
        ); // WHAT: the search. WHY: number or piece mark
        self.search_box(
            ui,
            pal,
            search,
            "sheets",
            "Sheet or piece mark (S-101, B12)",
        ); // WHAT: drawn. WHY: typed into
        let labels = ["All", "New", "Shop", "Erection", "Anchor bolts"]; // WHAT: the five filters. WHY: from the disciplines
        let mut x = body.left() + pad; // WHAT: where the next goes. WHY: left to right
        let y = search.bottom() + 16.0; // WHAT: their row. WHY: under the search
        for (i, label) in labels.iter().enumerate() {
            let count = sheets_shown(sheets, i, "", &self.seen).len(); // WHAT: how many it holds. WHY: shown on it
            let text = format!("{label}  {count}"); // WHAT: its words. WHY: as .seg span b
            let w = text.chars().count() as f32 * 10.0 + 40.0; // WHAT: its width. WHY: fits the words
            let rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, SMALLEST)); // WHAT: the button. WHY: 52 tall
            if phone && rect.right() > body.right() {
                break; // WHAT: no room. WHY: a phone shows the first few
            }
            if button(
                ui,
                pal,
                &mut self.targets,
                rect,
                &text,
                if self.filter == i {
                    Look::Main
                } else {
                    Look::Plain
                },
                17.0,
            ) {
                self.filter = i; // WHAT: chosen. WHY: the list follows
            }
            x = rect.right() + 8.0; // WHAT: past it. WHY: the next
        }
        let shown = sheets_shown(sheets, self.filter, &self.sheet_search, &self.seen); // WHAT: what the list shows. WHY: filtered and searched
        let list = Rect::from_min_max(
            Pos2::new(body.left(), y + SMALLEST + 16.0),
            Pos2::new(body.right(), body.bottom() - 44.0),
        ); // WHAT: the list's room. WHY: scrolls
        let columns = if phone { 1 } else { 2 }; // WHAT: two columns. WHY: as mockup c
        let gap = 12.0; // WHAT: between rows. WHY: room for gloves
        let w = (body.width() - pad * 2.0 - gap * (columns as f32 - 1.0)) / columns as f32; // WHAT: a row's width. WHY: fills
        let h = 92.0; // WHAT: a row's height. WHY: 88 to 92
        let mut open = None; // WHAT: a sheet tapped. WHY: opened after drawing
        ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("tablet-sheets")
                .show(ui, |ui| {
                    let rows = shown.len().div_ceil(columns); // WHAT: rows. WHY: the scroll's height
                    let (area, _) = ui.allocate_exact_size(
                        Vec2::new(list.width(), rows as f32 * (h + gap)),
                        Sense::hover(),
                    ); // WHAT: room for them. WHY: scrolled
                    for (i, sheet) in shown.iter().enumerate() {
                        let rect = Rect::from_min_size(
                            Pos2::new(
                                area.left() + pad + (i % columns) as f32 * (w + gap),
                                area.top() + (i / columns) as f32 * (h + gap),
                            ),
                            Vec2::new(w, h),
                        ); // WHAT: the row. WHY: in the grid
                        let response =
                            tap(ui, &mut self.targets, rect, &format!("sheet-{}", sheet.set)); // WHAT: tapped. WHY: opens it
                        let painter = ui.painter(); // WHAT: inside the scroll. WHY: clipped
                        let new = !self.seen.contains(&sheet.set); // WHAT: not opened here. WHY: NEW
                        card(
                            painter,
                            rect,
                            pal.card,
                            if new {
                                pal.purple.0.gamma_multiply(0.6)
                            } else {
                                pal.line
                            },
                            1.0,
                        ); // WHAT: the row. WHY: a new one edged purple
                        let thumb = Rect::from_min_size(
                            rect.min + Vec2::new(14.0, 12.0),
                            Vec2::new(88.0, 66.0),
                        ); // WHAT: a sheet's picture. WHY: as mockup c
                        painter.rect(
                            thumb,
                            CornerRadius::same(6),
                            hex(0xF3F4F6),
                            Stroke::new(1.0_f32, pal.line),
                            StrokeKind::Inside,
                        ); // WHAT: paper. WHY: a drawing
                        for k in 1..5 {
                            let fx = thumb.left() + thumb.width() * (0.12 + 0.13 * k as f32); // WHAT: a grid line across. WHY: looks like a plan
                            painter.line_segment(
                                [
                                    Pos2::new(fx, thumb.top() + 10.0),
                                    Pos2::new(fx, thumb.bottom() - 10.0),
                                ],
                                Stroke::new(1.2_f32, hex(0x374151)),
                            ); // WHAT: drawn. WHY: likewise
                            let fy = thumb.top() + thumb.height() * (0.12 + 0.17 * k as f32); // WHAT: a grid line down. WHY: likewise
                            painter.line_segment(
                                [
                                    Pos2::new(thumb.left() + 10.0, fy),
                                    Pos2::new(thumb.right() - 30.0, fy),
                                ],
                                Stroke::new(1.2_f32, hex(0x374151)),
                            ); // WHAT: drawn. WHY: likewise
                        }
                        let num = words(
                            painter,
                            rect.min + Vec2::new(120.0, 12.0),
                            Align2::LEFT_TOP,
                            &sheet.sheet,
                            22.0,
                            true,
                            pal.ink,
                        ); // WHAT: the sheet number, large. WHY: the list is by number
                        if new {
                            let badge = Rect::from_min_size(
                                Pos2::new(num.right() + 10.0, num.top() + 1.0),
                                Vec2::new(50.0, 26.0),
                            ); // WHAT: the NEW badge. WHY: until opened here
                            painter.rect_filled(badge, CornerRadius::same(8), pal.purple.0); // WHAT: purple. WHY: new or revised
                            words(
                                painter,
                                badge.center(),
                                Align2::CENTER_CENTER,
                                "NEW",
                                13.0,
                                true,
                                if pal.bg == BRIGHT.bg {
                                    Color32::WHITE
                                } else {
                                    hex(0x160F2E)
                                },
                            ); // WHAT: the word. WHY: never colour alone
                        }
                        let title = painter.layout(
                            sheet.title.clone(),
                            font(16.0, false),
                            pal.muted,
                            w - 170.0,
                        ); // WHAT: its title, one line. WHY: under the number
                        painter.galley(rect.min + Vec2::new(120.0, 40.0), title, pal.muted); // WHAT: drawn. WHY: likewise
                        let rev = words(
                            painter,
                            rect.min + Vec2::new(120.0, 64.0),
                            Align2::LEFT_TOP,
                            &format!("Rev {}", sheet.rev),
                            15.0,
                            true,
                            pal.purple.0,
                        ); // WHAT: its revision. WHY: as mockup c
                        words(
                            painter,
                            Pos2::new(rev.right() + 6.0, rev.top()),
                            Align2::LEFT_TOP,
                            &format!("· released {}", short_day(&sheet.released)),
                            15.0,
                            false,
                            pal.muted,
                        ); // WHAT: when released. WHY: likewise
                        chevron(
                            painter,
                            Pos2::new(rect.right() - 26.0, rect.center().y),
                            true,
                            pal.faint,
                        ); // WHAT: ›. WHY: it opens
                        if response.clicked() {
                            open = Some(sheet.set.clone()); // WHAT: this one. WHY: opened below
                        }
                    }
                });
        });
        if let Some(set) = open {
            self.open_sheet(&set, actions); // WHAT: opened in the viewer. WHY: one tap
        }
        let foot = body.bottom() - 24.0; // WHAT: the footer's line. WHY: at the bottom
        words(&p, Pos2::new(body.left() + pad, foot), Align2::LEFT_CENTER, "Only sheets released for fabrication are shown. When a sheet is revised, the old one leaves this list by itself.", 14.0, false, pal.faint);
        // WHAT: the rule, said. WHY: as mockup c
    }

    /// Opens a released sheet in View's viewer, and marks it seen.
    pub fn open_sheet(&mut self, set: &str, actions: &mut Vec<Action>) {
        let name = self
            .job
            .as_ref()
            .and_then(|j| j.manifest.sheets.iter().find(|s| s.set == set))
            .map_or_else(
                || "Drawing".into(),
                |s| format!("{} Rev {}", s.sheet, s.rev),
            ); // WHAT: its name. WHY: the download is named by it
        self.seen.insert(set.to_string()); // WHAT: seen. WHY: NEW goes once opened
        self.viewing = Some(set.to_string()); // WHAT: the viewer's sheet. WHY: its bar and flipping
        actions.push(Action::Open {
            set: set.to_string(),
            name,
        }); // WHAT: opened. WHY: View's own viewer, unchanged
    }

    /// The loads: mockup e.
    fn shipping_screen(
        &mut self,
        ui: &mut egui::Ui,
        pal: &Palette,
        env: &Env,
        job: &TabletView,
        body: Rect,
        phone: bool,
    ) {
        let p = ui.painter().clone(); // WHAT: the painter. WHY: drawn by hand
        let pad = if phone { 16.0 } else { 26.0 }; // WHAT: the margin. WHY: as .main
        let mut x = body.left() + pad; // WHAT: where the next goes. WHY: left to right
        for (i, label) in ["Today", "This week", "All loads"].iter().enumerate() {
            let rect = Rect::from_min_size(
                Pos2::new(x, body.top() + 22.0),
                Vec2::new(label.chars().count() as f32 * 10.0 + 48.0, SMALLEST),
            ); // WHAT: the button. WHY: 52 tall
            if button(
                ui,
                pal,
                &mut self.targets,
                rect,
                label,
                if self.ship == i {
                    Look::Main
                } else {
                    Look::Plain
                },
                17.0,
            ) {
                self.ship = i; // WHAT: chosen. WHY: the list follows
            }
            x = rect.right() + 8.0; // WHAT: past it. WHY: the next
        }
        let loads: Vec<&Load> = job
            .manifest
            .loads
            .iter()
            .filter(|l| load_shown(l, self.ship, &env.today, holder(job)))
            .collect(); // WHAT: the loads it leaves. WHY: Today, This week, All
        let list = Rect::from_min_max(
            Pos2::new(body.left(), body.top() + 22.0 + SMALLEST + 16.0),
            Pos2::new(body.right(), body.bottom() - 10.0),
        ); // WHAT: the list's room. WHY: scrolls
        if loads.is_empty() {
            words(
                &p,
                list.min + Vec2::new(pad, 20.0),
                Align2::LEFT_TOP,
                "No loads here. Try This week or All loads.",
                18.0,
                false,
                pal.muted,
            ); // WHAT: none. WHY: said
        }
        let field_first = job.field; // WHAT: a field tablet. WHY: the receiver's box first
        let mut open = None; // WHAT: a load tapped. WHY: its ticket after drawing
        ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("tablet-loads")
                .show(ui, |ui| {
                    let h = if phone { 250.0 } else { 170.0 }; // WHAT: a card's height. WHY: room for the boxes and the button
                    let (area, _) = ui.allocate_exact_size(
                        Vec2::new(list.width(), loads.len() as f32 * (h + 14.0)),
                        Sense::hover(),
                    ); // WHAT: room for every card. WHY: scrolled
                    for (i, load) in loads.iter().enumerate() {
                        let rect = Rect::from_min_size(
                            Pos2::new(area.left() + pad, area.top() + i as f32 * (h + 14.0)),
                            Vec2::new(list.width() - pad * 2.0, h),
                        ); // WHAT: the card. WHY: one per load
                        let painter = ui.painter().clone(); // WHAT: inside the scroll. WHY: clipped
                        let waiting = load.boxes.iter().any(|b| open_box(b, holder(job))); // WHAT: a box to sign. WHY: edged amber
                        card(
                            &painter,
                            rect,
                            pal.card,
                            if waiting { pal.amber.0 } else { pal.line },
                            if waiting { 1.5 } else { 1.0 },
                        ); // WHAT: the card. WHY: as mockup e
                        let pieces: u32 = load.pieces.iter().map(|p| p.qty).sum(); // WHAT: pieces on it. WHY: said
                        let title = format!(
                            "Load {} · {} lb{}",
                            load.number,
                            thousands(load.weight_lb),
                            if load.trailer.is_empty() {
                                String::new()
                            } else {
                                format!(" · {}", load.trailer)
                            }
                        ); // WHAT: number, weight, trailer. WHY: as mockup e
                        words(
                            &painter,
                            rect.min + Vec2::new(22.0, 18.0),
                            Align2::LEFT_TOP,
                            &title,
                            22.0,
                            true,
                            pal.ink,
                        ); // WHAT: drawn. WHY: as mockup e
                        let ships = if load.ships == env.today {
                            "today".to_string()
                        } else {
                            short_day(&load.ships)
                        }; // WHAT: when it ships. WHY: in words
                        words(
                            &painter,
                            rect.min + Vec2::new(22.0, 50.0),
                            Align2::LEFT_TOP,
                            &format!(
                                "{pieces} pieces · {} · ships {ships}",
                                if load.destination.is_empty() {
                                    "the job"
                                } else {
                                    &load.destination
                                }
                            ),
                            16.0,
                            false,
                            pal.muted,
                        ); // WHAT: pieces, where, when. WHY: as mockup e
                        let (state, colors) = status(pal, load); // WHAT: its status. WHY: words and colour
                        chip(
                            &painter,
                            Pos2::new(rect.right() - 160.0, rect.top() + 16.0),
                            state,
                            colors,
                        ); // WHAT: drawn. WHY: never colour alone
                        let mut cx = rect.left() + 22.0; // WHAT: the boxes' row. WHY: left to right
                        for b in &load.boxes {
                            let label = match signed_by(b) {
                                Some(_) => (format!("✓ {}", box_words(&b.which)), pal.green), // WHAT: signed, green. WHY: done
                                None if open_box(b, holder(job)) => {
                                    (format!("{} to sign", box_words(&b.which)), pal.amber)
                                } // WHAT: to sign, amber. WHY: waiting here
                                None => (
                                    format!("{}: not asked yet", box_words(&b.which)),
                                    (pal.muted, pal.card2),
                                ), // WHAT: no line yet. WHY: Fabwire asks it on its next pass
                            };
                            let r = chip(
                                &painter,
                                Pos2::new(cx, rect.top() + 84.0),
                                &label.0,
                                label.1,
                            ); // WHAT: the box's chip. WHY: as mockup e
                            cx = r.right() + 8.0; // WHAT: past it. WHY: the next
                            if phone && cx > rect.right() - 120.0 {
                                break; // WHAT: no more room. WHY: the ticket shows them all
                            }
                        }
                        let label = match next_box(load, field_first, holder(job)) {
                            Some(b) => format!("Sign: {}", box_words(&b.which).to_lowercase()),
                            None => "Open ticket".to_string(),
                        }; // WHAT: the one big button's words. WHY: what to do
                        let br = Rect::from_min_size(
                            Pos2::new(rect.right() - 262.0, rect.bottom() - 66.0),
                            Vec2::new(240.0, 56.0),
                        ); // WHAT: its place. WHY: bottom right
                        if button(
                            ui,
                            pal,
                            &mut self.targets,
                            br,
                            &label,
                            if waiting { Look::Main } else { Look::Plain },
                            18.0,
                        ) {
                            open = Some(load.number); // WHAT: this load. WHY: its ticket
                        }
                    }
                });
        });
        if let Some(n) = open {
            self.screen = Some(Screen::Ticket(n)); // WHAT: the ticket. WHY: two taps to sign
        }
    }

    /// A ticket: mockup f.
    #[allow(clippy::too_many_arguments)] // WHAT: the screen's parts each passed. WHY: one screen
    fn ticket_screen(
        &mut self,
        ui: &mut egui::Ui,
        pal: &Palette,
        env: &Env,
        job: &TabletView,
        n: u32,
        body: Rect,
        phone: bool,
        actions: &mut Vec<Action>,
    ) {
        let p = ui.painter().clone(); // WHAT: the painter. WHY: drawn by hand
        let Some(load) = job.manifest.loads.iter().find(|l| l.number == n) else {
            words(
                &p,
                body.min + Vec2::new(26.0, 26.0),
                Align2::LEFT_TOP,
                "This load isn't on the tablet any more. Check with the office.",
                18.0,
                false,
                pal.muted,
            ); // WHAT: gone. WHY: said, never a blank screen
            return;
        };
        let pad = if phone { 16.0 } else { 24.0 }; // WHAT: the margin. WHY: as mockup f
        let bottom = body.bottom() - pad - MAIN; // WHAT: the big buttons' top. WHY: they sit at the foot
        let (left, right) = if phone {
            (
                Rect::from_min_max(
                    body.min + Vec2::new(pad, 18.0),
                    Pos2::new(body.right() - pad, body.top() + 240.0),
                ),
                Rect::from_min_max(
                    Pos2::new(body.left() + pad, body.top() + 254.0),
                    Pos2::new(body.right() - pad, bottom - 14.0),
                ),
            ) // WHAT: one column. WHY: a phone
        } else {
            let split = body.left() + body.width() * 0.62; // WHAT: the pieces and the signatures side by side. WHY: as mockup f
            (
                Rect::from_min_max(
                    body.min + Vec2::new(pad, 18.0),
                    Pos2::new(split, bottom - 14.0),
                ),
                Rect::from_min_max(
                    Pos2::new(split + 18.0, body.top() + 18.0),
                    Pos2::new(body.right() - pad, bottom - 14.0),
                ),
            ) // WHAT: left and right. WHY: likewise
        };
        card(&p, left, pal.card, pal.line, 1.0); // WHAT: the pieces' card. WHY: as mockup f
        let cols = [
            ("PC MARK", 0.0),
            ("QTY", 0.17),
            ("DESCRIPTION", 0.27),
            ("LENGTH", 0.72),
            ("WEIGHT", 0.86),
        ]; // WHAT: the columns and where each starts. WHY: as the ticket
        for (label, at) in cols {
            words(
                &p,
                Pos2::new(
                    left.left() + 18.0 + (left.width() - 36.0) * at,
                    left.top() + 18.0,
                ),
                Align2::LEFT_TOP,
                label,
                13.0,
                true,
                pal.muted,
            ); // WHAT: a heading. WHY: as mockup f
        }
        let rows = Rect::from_min_max(
            Pos2::new(left.left(), left.top() + 44.0),
            Pos2::new(left.right(), left.bottom() - 56.0),
        ); // WHAT: the rows' room. WHY: scrolls
        ui.scope_builder(egui::UiBuilder::new().max_rect(rows), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("tablet-pieces")
                .show(ui, |ui| {
                    let (area, _) = ui.allocate_exact_size(
                        Vec2::new(rows.width(), load.pieces.len() as f32 * 48.0),
                        Sense::hover(),
                    ); // WHAT: room for every line. WHY: scrolled
                    let painter = ui.painter(); // WHAT: inside the scroll. WHY: clipped
                    for (i, piece) in load.pieces.iter().enumerate() {
                        let y = area.top() + i as f32 * 48.0; // WHAT: the row's top. WHY: 48 a row
                        painter.line_segment(
                            [
                                Pos2::new(area.left() + 8.0, y),
                                Pos2::new(area.right() - 8.0, y),
                            ],
                            Stroke::new(1.0_f32, pal.line),
                        ); // WHAT: a rule. WHY: rows read apart
                        let cell = |at: f32| {
                            Pos2::new(area.left() + 18.0 + (area.width() - 36.0) * at, y + 24.0)
                        }; // WHAT: a cell's left middle. WHY: by column
                        words(
                            painter,
                            cell(0.0),
                            Align2::LEFT_CENTER,
                            &piece.mark,
                            18.0,
                            true,
                            pal.ink,
                        ); // WHAT: the mark. WHY: first
                        words(
                            painter,
                            cell(0.17),
                            Align2::LEFT_CENTER,
                            &piece.qty.to_string(),
                            17.0,
                            false,
                            pal.ink,
                        ); // WHAT: how many. WHY: counted
                        let d = painter.layout(
                            piece.description.clone(),
                            font(16.0, false),
                            pal.ink,
                            (area.width() - 36.0) * 0.44,
                        ); // WHAT: what it is, cut to its column. WHY: fits
                        painter.galley(cell(0.27) - Vec2::new(0.0, d.size().y / 2.0), d, pal.ink); // WHAT: drawn. WHY: likewise
                        words(
                            painter,
                            cell(0.72),
                            Align2::LEFT_CENTER,
                            &piece.length,
                            16.0,
                            false,
                            pal.ink,
                        ); // WHAT: its length. WHY: as written
                        words(
                            painter,
                            Pos2::new(area.right() - 18.0, y + 24.0),
                            Align2::RIGHT_CENTER,
                            &format!("{} lb", thousands(piece.weight_lb)),
                            16.0,
                            false,
                            pal.ink,
                        ); // WHAT: its weight. WHY: as written
                    }
                });
        });
        let count: u32 = load.pieces.iter().map(|p| p.qty).sum(); // WHAT: pieces. WHY: the footer
        p.line_segment(
            [
                Pos2::new(left.left() + 8.0, left.bottom() - 56.0),
                Pos2::new(left.right() - 8.0, left.bottom() - 56.0),
            ],
            Stroke::new(1.0_f32, pal.line),
        ); // WHAT: the footer's rule. WHY: as mockup f
        words(
            &p,
            Pos2::new(left.left() + 22.0, left.bottom() - 28.0),
            Align2::LEFT_CENTER,
            &format!("{} lines · {count} pieces", load.pieces.len()),
            18.0,
            true,
            pal.ink,
        ); // WHAT: the totals. WHY: as mockup f
        words(
            &p,
            Pos2::new(left.right() - 22.0, left.bottom() - 28.0),
            Align2::RIGHT_CENTER,
            &format!("{} lb", thousands(load.weight_lb)),
            18.0,
            true,
            pal.ink,
        ); // WHAT: the weight. WHY: likewise
        words(
            &p,
            right.min,
            Align2::LEFT_TOP,
            "SIGNATURES",
            13.0,
            true,
            pal.muted,
        ); // WHAT: the panel's label. WHY: as mockup f
        let mut y = right.top() + 26.0; // WHAT: the first box. WHY: top to bottom
        let mut sign = None; // WHAT: a box tapped. WHY: the pad after drawing
        for b in &load.boxes {
            let rect =
                Rect::from_min_size(Pos2::new(right.left(), y), Vec2::new(right.width(), 70.0)); // WHAT: the box. WHY: 70 tall
            let open = open_box(b, holder(job)); // WHAT: to sign here. WHY: amber and tappable
            let (edge, fill) = match (signed_by(b), open) {
                (Some(_), _) => (pal.line, pal.card), // WHAT: signed. WHY: plain
                (None, true) => (pal.amber.0, pal.amber.1), // WHAT: to sign. WHY: amber
                (None, false) => (pal.line, pal.card), // WHAT: not asked yet. WHY: plain
            };
            card(&p, rect, fill, edge, if open { 2.0 } else { 1.0 }); // WHAT: the box. WHY: as mockup f
            let circle = Pos2::new(rect.left() + 36.0, rect.center().y); // WHAT: its mark's place. WHY: left
            words(
                &p,
                Pos2::new(rect.left() + 70.0, rect.top() + 12.0),
                Align2::LEFT_TOP,
                &box_words(&b.which).to_uppercase(),
                13.0,
                true,
                if open { pal.amber.0 } else { pal.muted },
            ); // WHAT: the box's name. WHY: as the form
            match signed_by(b) {
                Some((who, day)) => {
                    p.circle_filled(circle, 20.0, pal.green.1); // WHAT: green. WHY: signed
                    tick(&p, circle, 18.0, pal.green.0); // WHAT: a tick. WHY: never colour alone
                    words(
                        &p,
                        Pos2::new(rect.left() + 70.0, rect.top() + 32.0),
                        Align2::LEFT_TOP,
                        &format!("{who} · {}", short_day(&day)),
                        18.0,
                        true,
                        pal.ink,
                    ); // WHAT: who and when. WHY: as mockup f
                }
                None => {
                    p.circle_filled(circle, 20.0, if open { pal.amber.0 } else { pal.card2 }); // WHAT: amber when waiting. WHY: to sign
                    let waiting_for = if open {
                        format!(
                            "Waiting for the {}",
                            if b.which == "received_by" {
                                "receiver"
                            } else if b.which == "driver" {
                                "driver"
                            } else {
                                "shop"
                            }
                        )
                    } else {
                        "Not asked yet: the office sends it shortly".to_string()
                    }; // WHAT: what it waits for. WHY: in words
                    words(
                        &p,
                        Pos2::new(rect.left() + 70.0, rect.top() + 32.0),
                        Align2::LEFT_TOP,
                        &waiting_for,
                        17.0,
                        true,
                        pal.ink,
                    ); // WHAT: drawn. WHY: as mockup f
                }
            }
            if open && tap(ui, &mut self.targets, rect, &format!("box-{}", b.which)).clicked() {
                sign = Some(b.clone()); // WHAT: this box. WHY: the pad opens on it
            }
            y = rect.bottom() + 12.0; // WHAT: the next. WHY: 12 between
        }
        let deliver = Rect::from_min_size(
            Pos2::new(right.left(), right.bottom() - 156.0),
            Vec2::new(right.width(), 86.0),
        ); // WHAT: where it goes. WHY: as mockup f
        if !phone && deliver.top() > y {
            card(&p, deliver, pal.card, pal.line, 1.0); // WHAT: its card. WHY: as mockup f
            words(
                &p,
                deliver.min + Vec2::new(22.0, 14.0),
                Align2::LEFT_TOP,
                "DELIVER TO",
                13.0,
                true,
                pal.muted,
            ); // WHAT: the label. WHY: likewise
            wrapped(
                &p,
                deliver.min + Vec2::new(22.0, 34.0),
                deliver.width() - 44.0,
                if load.destination.is_empty() {
                    &job.manifest.job.site
                } else {
                    &load.destination
                },
                17.0,
                true,
                pal.ink,
            ); // WHAT: where. WHY: likewise
        }
        let printed = Rect::from_min_size(
            Pos2::new(right.left(), right.bottom() - SMALLEST - 4.0),
            Vec2::new(right.width(), SMALLEST + 4.0),
        ); // WHAT: "See the ticket as printed". WHY: the PDF for anyone who wants it
        if button(
            ui,
            pal,
            &mut self.targets,
            printed,
            "See the ticket as printed",
            Look::Plain,
            18.0,
        ) {
            actions.push(Action::Open {
                set: load.set.clone(),
                name: format!("Shipping ticket {}", load.ticket),
            }); // WHAT: the ticket's PDF in the viewer. WHY: with its sign-off list, as before
        }
        let problem = Rect::from_min_size(
            Pos2::new(body.left() + pad, bottom),
            Vec2::new(
                if phone {
                    (body.width() - pad * 2.0) * 0.4
                } else {
                    300.0
                },
                MAIN,
            ),
        ); // WHAT: Report a problem. WHY: a short or damaged load
        if button(
            ui,
            pal,
            &mut self.targets,
            problem,
            if phone { "Problem" } else { "Report a problem" },
            Look::Plain,
            20.0,
        ) {
            self.problem = !self.problem; // WHAT: opened. WHY: who to call
        }
        let next = next_box(load, job.field, holder(job)).cloned(); // WHAT: the box the big button signs. WHY: one main action
        let main = Rect::from_min_max(
            Pos2::new(problem.right() + 14.0, bottom),
            Pos2::new(body.right() - pad, bottom + MAIN),
        ); // WHAT: the big button. WHY: 76 tall
        let label = next.as_ref().map_or("Nothing to sign".to_string(), |b| {
            format!("Sign: {}", box_words(&b.which).to_lowercase())
        }); // WHAT: its words. WHY: what it signs
        if button(
            ui,
            pal,
            &mut self.targets,
            main,
            &label,
            if next.is_some() && env.connected {
                Look::Main
            } else {
                Look::Off
            },
            24.0,
        ) {
            sign.clone_from(&next); // WHAT: that box. WHY: the pad opens on it
        }
        if !env.connected && next.is_some() {
            words(&p, Pos2::new(main.center().x, main.top() - 12.0), Align2::CENTER_BOTTOM, "No connection: signing needs the office. Try again near the trailer or office Wi-Fi.", 15.0, true, pal.red.0);
            // WHAT: offline, said plainly. WHY: signing needs the server until phase 3
        }
        if self.problem {
            let note = Rect::from_min_size(
                Pos2::new(body.left() + pad, bottom - 90.0),
                Vec2::new(body.width() - pad * 2.0, 76.0),
            ); // WHAT: the note. WHY: over the foot
            card(&p, note, pal.red.1, pal.red.0, 1.5); // WHAT: red. WHY: a problem
            wrapped(&p, note.min + Vec2::new(18.0, 12.0), note.width() - 36.0, &format!("Short or damaged? Don't sign for what isn't there. Call the office{}: they note it on the load and change the ticket before it is signed.", if job.manifest.job.office.is_empty() { String::new() } else { format!(" ({})", job.manifest.job.office) }), 16.0, true, pal.red.0);
            // WHAT: what to do now. WHY: the note written from the tablet comes in phase 2
        }
        if let Some(b) = sign {
            self.problem = false; // WHAT: the note put away. WHY: signing
            self.open_pad(n, &b, &env.person); // WHAT: the pad. WHY: on that box
        }
    }

    /// The pad: mockup g (and j on a phone).
    fn pad_screen(
        &mut self,
        ui: &mut egui::Ui,
        pal: &Palette,
        env: &Env,
        full: Rect,
        phone: bool,
        actions: &mut Vec<Action>,
    ) {
        let Some(mut pad) = self.pad.take() else {
            self.screen = Some(Screen::Shipping); // WHAT: no pad. WHY: back to the loads
            return;
        };
        let p = ui.painter().clone(); // WHAT: the painter. WHY: drawn by hand
        let job = self.job.clone(); // WHAT: the job. WHY: the load's words
        let load = job
            .as_ref()
            .and_then(|j| j.manifest.loads.iter().find(|l| l.number == pad.load))
            .cloned(); // WHAT: the load. WHY: what they sign for
        let receiver = pad.which != "loaded_by"; // WHAT: somebody else signs. WHY: their own name, typed
        let who = if pad.which == "driver" {
            "driver"
        } else if receiver {
            "receiver"
        } else {
            "loader"
        }; // WHAT: the signer's part. WHY: the words
        p.line_segment(
            [
                Pos2::new(full.left(), full.top() + TOP),
                Pos2::new(full.right(), full.top() + TOP),
            ],
            Stroke::new(1.0_f32, pal.line),
        ); // WHAT: the bar's edge. WHY: as the others
        let cancel = Rect::from_min_size(
            full.min + Vec2::new(20.0, 8.0),
            Vec2::new(if phone { 110.0 } else { 140.0 }, SMALLEST),
        ); // WHAT: Cancel. WHY: unsigned, back to the ticket
        if button(
            ui,
            pal,
            &mut self.targets,
            cancel,
            "✗ Cancel",
            if pad.sending { Look::Off } else { Look::Plain },
            18.0,
        ) {
            self.screen = Some(Screen::Ticket(pad.load)); // WHAT: back. WHY: cancelled
            return; // WHAT: the pad dropped. WHY: unsigned
        }
        let ticket = load
            .as_ref()
            .map(|l| format!("SHIPPING TICKET {} · LOAD {}", l.ticket, l.number))
            .unwrap_or_default(); // WHAT: which ticket. WHY: the kicker
        words(
            &p,
            if phone {
                Pos2::new(cancel.right() + 14.0, full.top() + 14.0)
            } else {
                Pos2::new(full.center().x, full.top() + 14.0)
            }, // WHAT: beside Cancel on a phone, centred on a tablet. WHY: never under the button
            if phone {
                Align2::LEFT_TOP
            } else {
                Align2::CENTER_TOP
            },
            &ticket,
            12.0,
            true,
            pal.gold,
        ); // WHAT: drawn. WHY: as mockup g
        words(
            &p,
            if phone {
                Pos2::new(cancel.right() + 14.0, full.top() + 32.0)
            } else {
                Pos2::new(full.center().x, full.top() + 32.0)
            }, // WHAT: likewise. WHY: likewise
            if phone {
                Align2::LEFT_TOP
            } else {
                Align2::CENTER_TOP
            },
            box_words(&pad.which),
            22.0,
            true,
            pal.ink,
        ); // WHAT: the box. WHY: what is signed
        let named = !pad.name.trim().is_empty(); // WHAT: a name typed. WHY: step 2 done
        let drawn = !pad.strokes.is_empty(); // WHAT: a signature drawn. WHY: step 1 done
        if !phone {
            let mut sx = full.right() - 380.0; // WHAT: the steps' place. WHY: top right
            for (i, (label, done)) in [("Sign", drawn), ("Name", named), ("Done", false)]
                .iter()
                .enumerate()
            {
                let c = Pos2::new(sx + 18.0, full.top() + 34.0); // WHAT: the step's circle. WHY: numbered
                p.circle_filled(
                    c,
                    18.0,
                    if *done || i == 0 && !drawn || i == 1 && drawn && !named {
                        pal.accent
                    } else {
                        pal.card2
                    },
                ); // WHAT: lit when done or current. WHY: where they are
                words(
                    &p,
                    c,
                    Align2::CENTER_CENTER,
                    &(i + 1).to_string(),
                    15.0,
                    true,
                    if *done { pal.accent_ink } else { pal.ink },
                ); // WHAT: its number. WHY: 1 2 3
                let r = words(
                    &p,
                    Pos2::new(c.x + 26.0, c.y),
                    Align2::LEFT_CENTER,
                    label,
                    17.0,
                    true,
                    pal.ink,
                ); // WHAT: its word. WHY: never a number alone
                sx = r.right() + 26.0; // WHAT: the next. WHY: left to right
            }
        }
        let body = Rect::from_min_max(
            Pos2::new(full.left() + 24.0, full.top() + TOP + 20.0),
            Pos2::new(full.right() - 24.0, full.bottom() - 24.0),
        ); // WHAT: under the bar. WHY: the pad's room
        let foot = body.bottom() - MAIN - 30.0; // WHAT: the buttons' top. WHY: at the foot, with room for the hint
        let (pad_rect, side) = if phone {
            (
                Rect::from_min_size(
                    body.min,
                    Vec2::new(body.width(), (body.width() * 0.62).min(260.0)),
                ),
                Rect::from_min_max(
                    Pos2::new(
                        body.left(),
                        body.top() + (body.width() * 0.62).min(260.0) + 16.0,
                    ),
                    Pos2::new(body.right(), foot - 10.0),
                ),
            ) // WHAT: the pad over the name. WHY: one column
        } else {
            let split = body.left() + body.width() * 0.62; // WHAT: the pad and the details side by side. WHY: as mockup g
            (
                Rect::from_min_max(body.min, Pos2::new(split, foot - 14.0)),
                Rect::from_min_max(
                    Pos2::new(split + 20.0, body.top()),
                    Pos2::new(body.right(), foot - 14.0),
                ),
            ) // WHAT: left and right. WHY: likewise
        };
        p.rect(
            pad_rect,
            CornerRadius::same(18),
            Color32::WHITE,
            Stroke::new(3.0_f32, hex(0xD9DDE5)),
            StrokeKind::Inside,
        ); // WHAT: white paper, always. WHY: like paper, in either look
        words(
            &p,
            pad_rect.min + Vec2::new(24.0, 22.0),
            Align2::LEFT_TOP,
            &format!("{}'S SIGNATURE", who.to_uppercase()),
            14.0,
            true,
            hex(0x374151),
        ); // WHAT: whose. WHY: as mockup g
        let line_y = pad_rect.bottom() - pad_rect.height() * 0.22; // WHAT: the line to sign on. WHY: near the foot
        p.line_segment(
            [
                Pos2::new(pad_rect.left() + 50.0, line_y),
                Pos2::new(pad_rect.right() - 50.0, line_y),
            ],
            Stroke::new(2.0_f32, hex(0x9CA3AF)),
        ); // WHAT: the line. WHY: people sign on a line
        let x = Pos2::new(pad_rect.left() + 62.0, line_y - 22.0); // WHAT: the X. WHY: where to start
        p.line_segment(
            [x - Vec2::splat(9.0), x + Vec2::splat(9.0)],
            Stroke::new(2.5_f32, hex(0x6B7280)),
        ); // WHAT: one stroke of the X. WHY: as on paper
        p.line_segment(
            [x + Vec2::new(-9.0, 9.0), x + Vec2::new(9.0, -9.0)],
            Stroke::new(2.5_f32, hex(0x6B7280)),
        ); // WHAT: the other. WHY: likewise
        words(
            &p,
            Pos2::new(pad_rect.center().x, line_y + 24.0),
            Align2::CENTER_TOP,
            "Sign above the line with a finger or a pen",
            16.0,
            false,
            hex(0x6B7280),
        ); // WHAT: the hint. WHY: as mockup g
        let ink = Rect::from_min_max(
            pad_rect.min + Vec2::new(10.0, 50.0),
            Pos2::new(pad_rect.right() - 10.0, line_y + 16.0),
        ); // WHAT: where ink goes. WHY: the pad's own shape below
        self.targets.push((ink, "pad".into())); // WHAT: remembered. WHY: tested with the rest
        let response = ui.interact(ink, ui.id().with("tablet-pad"), Sense::drag()); // WHAT: a finger or pen dragging. WHY: drawn by dragging
        if !pad.sending {
            if let Some(at) = response.interact_pointer_pos() {
                if response.dragged() || response.drag_started() {
                    let point = crate::signoffs::on_pad(at, ink); // WHAT: on the pad, in its units. WHY: the server's units
                    if pad.drawing.last() != Some(&point) {
                        pad.drawing.push(point); // WHAT: added. WHY: the line follows the finger
                    }
                }
            }
            if response.drag_stopped() && !pad.drawing.is_empty() {
                pad.strokes.push(std::mem::take(&mut pad.drawing)); // WHAT: the stroke finished. WHY: the next touch starts another
            }
        }
        draw_strokes(
            &p,
            ink,
            pad.strokes.iter().chain(std::iter::once(&pad.drawing)),
            3.5,
        ); // WHAT: the ink. WHY: shown as drawn
        let mut y = side.top(); // WHAT: down the side. WHY: the name first
        words(
            &p,
            Pos2::new(side.left(), y),
            Align2::LEFT_TOP,
            &format!("{}'S NAME", who.to_uppercase()),
            13.0,
            true,
            pal.muted,
        ); // WHAT: the label. WHY: as mockup g
        y += 24.0; // WHAT: under it. WHY: the box
        let name_rect =
            Rect::from_min_size(Pos2::new(side.left(), y), Vec2::new(side.width(), 62.0)); // WHAT: the name box. WHY: big
        p.rect(
            name_rect,
            CornerRadius::same(14),
            pal.well,
            Stroke::new(2.0_f32, pal.accent),
            StrokeKind::Inside,
        ); // WHAT: its well, edged in the accent. WHY: as mockup g
        self.targets.push((name_rect, "name".into())); // WHAT: remembered. WHY: tested
        let edit = egui::TextEdit::singleline(&mut pad.name)
            .font(font(22.0, false))
            .hint_text(
                egui::RichText::new("Type their name")
                    .font(font(22.0, false))
                    .color(pal.faint),
            )
            .text_color(pal.ink)
            .frame(false)
            .interactive(!pad.sending); // WHAT: the name, starting empty for a receiver. WHY: free text; the account that collects it is kept by the server
        ui.put(name_rect.shrink2(Vec2::new(18.0, 12.0)), edit); // WHAT: placed. WHY: typed into
        y = name_rect.bottom() + 16.0; // WHAT: under it. WHY: the date
        let info = |p: &egui::Painter, y: f32, label: &str, big: &str, small: &str| -> f32 {
            let rect =
                Rect::from_min_size(Pos2::new(side.left(), y), Vec2::new(side.width(), 96.0)); // WHAT: a card. WHY: as mockup g
            card(p, rect, pal.card, pal.line, 1.0); // WHAT: drawn. WHY: likewise
            words(
                p,
                rect.min + Vec2::new(20.0, 14.0),
                Align2::LEFT_TOP,
                label,
                13.0,
                true,
                pal.muted,
            ); // WHAT: its label. WHY: likewise
            words(
                p,
                rect.min + Vec2::new(20.0, 34.0),
                Align2::LEFT_TOP,
                big,
                21.0,
                true,
                pal.ink,
            ); // WHAT: the main words. WHY: likewise
            words(
                p,
                rect.min + Vec2::new(20.0, 64.0),
                Align2::LEFT_TOP,
                small,
                15.0,
                false,
                pal.muted,
            ); // WHAT: the small words. WHY: likewise
            rect.bottom() + 14.0
        };
        if !phone || side.height() > 240.0 {
            y = info(&p, y, "DATE", &env.date_words, "Set by the tablet"); // WHAT: the date, set by the tablet. WHY: nobody types a date
            if let Some(l) = &load {
                let count: u32 = l.pieces.iter().map(|p| p.qty).sum(); // WHAT: pieces. WHY: what they sign for
                info(
                    &p,
                    y,
                    "THEY ARE SIGNING FOR",
                    &format!("{count} pieces · {} lb", thousands(l.weight_lb)),
                    &format!("{} lines on Load {}", l.pieces.len(), l.number),
                ); // WHAT: what they sign for. WHY: as mockup g
            }
        }
        words(
            &p,
            Pos2::new(side.left(), side.bottom() - 4.0),
            Align2::LEFT_BOTTOM,
            "Once signed, it can't be changed.",
            16.0,
            false,
            pal.muted,
        ); // WHAT: said before signing. WHY: a signature is final
        let clear = Rect::from_min_size(
            Pos2::new(body.left(), foot),
            Vec2::new(if phone { 120.0 } else { 200.0 }, MAIN),
        ); // WHAT: Clear. WHY: a smudge
        if button(
            ui,
            pal,
            &mut self.targets,
            clear,
            "Clear",
            if pad.sending { Look::Off } else { Look::Plain },
            22.0,
        ) {
            pad.strokes.clear(); // WHAT: rubbed out. WHY: start again
            pad.drawing.clear(); // WHAT: and the stroke under way. WHY: likewise
        }
        let ready = drawn && named && !pad.sending && env.connected; // WHAT: a signature, a name, not sent, connected. WHY: nothing half-done goes
        let label = if pad.sending {
            "Signing…".to_string()
        } else if pad.which == "received_by" && phone {
            "✓ Sign: delivered".to_string() // WHAT: the short words. WHY: fits a phone's button
        } else if pad.which == "received_by" {
            "✓ Sign and mark delivered".to_string()
        } else {
            "✓ Sign".to_string()
        }; // WHAT: the button's words. WHY: says what happens
        let go = Rect::from_min_max(
            Pos2::new(
                if phone {
                    clear.right() + 12.0
                } else {
                    body.left() + body.width() * 0.45
                },
                foot,
            ),
            Pos2::new(body.right(), foot + MAIN),
        ); // WHAT: the green button. WHY: 76 tall
        if button(
            ui,
            pal,
            &mut self.targets,
            go,
            &label,
            if ready { Look::Go } else { Look::Off },
            if phone { 20.0 } else { 24.0 }, // WHAT: smaller on a phone. WHY: the words fit the button
        ) {
            pad.sending = true; // WHAT: sent once. WHY: no double signature
            pad.said = None; // WHAT: the last reason cleared. WHY: a new try
            actions.push(Action::Sign {
                project: job.as_ref().map(|j| j.project.clone()).unwrap_or_default(),
                id: pad.line.clone(),
                name: pad.name.trim().to_string(),
                strokes: pad.strokes.clone(),
            }); // WHAT: the signature to the office server. WHY: the existing sign-off line, final there
        }
        let hint = match (&pad.said, env.connected, drawn, named) {
            (Some(said), _, _, _) => (said.clone(), pal.red.0), // WHAT: why it didn't go. WHY: said
            (None, false, _, _) => ("No connection: signing needs the office. Try again near the trailer or office Wi-Fi.".to_string(), pal.red.0), // WHAT: offline. WHY: said plainly, never failing quietly
            (None, true, false, _) => ("Sign on the pad to finish".to_string(), pal.amber.0), // WHAT: no signature yet. WHY: what's missing
            (None, true, true, false) => (format!("Type the {who}'s name to finish"), pal.amber.0), // WHAT: no name yet. WHY: likewise
            _ => (if pad.sending { "Sending to the office. Final once the office server has it.".to_string() } else { String::new() }, pal.muted), // WHAT: ready or sending. WHY: said
        };
        words(
            &p,
            Pos2::new(body.right(), foot + MAIN + 8.0),
            Align2::RIGHT_TOP,
            &hint.0,
            16.0,
            true,
            hint.1,
        ); // WHAT: drawn under the button. WHY: as mockup g
        self.pad = Some(pad); // WHAT: kept. WHY: still signing or sending
    }

    /// Delivered ✓: mockup h.
    fn done_screen(&mut self, ui: &egui::Ui, pal: &Palette, job: &TabletView, body: Rect) {
        let p = ui.painter().clone(); // WHAT: the painter. WHY: drawn by hand
        let Some(done) = self.done.clone() else {
            return;
        }; // WHAT: what was signed. WHY: shown
        let c = Pos2::new(body.center().x, body.top() + 110.0); // WHAT: the tick's place. WHY: top middle
        p.circle_filled(c, 64.0, pal.green.1); // WHAT: the green circle. WHY: done
        tick(&p, c, 64.0, pal.green.0); // WHAT: the big tick. WHY: never colour alone
        let delivered = done.which == "received_by"; // WHAT: the receiver signed. WHY: Delivered
        words(
            &p,
            Pos2::new(c.x, c.y + 86.0),
            Align2::CENTER_TOP,
            if delivered { "Delivered" } else { "Signed" },
            34.0,
            true,
            pal.ink,
        ); // WHAT: what happened. WHY: as mockup h
        let load = job.manifest.loads.iter().find(|l| l.number == done.load); // WHAT: the load. WHY: named
        if let Some(l) = load {
            let count: u32 = l.pieces.iter().map(|p| p.qty).sum(); // WHAT: pieces. WHY: said
            words(
                &p,
                Pos2::new(c.x, c.y + 132.0),
                Align2::CENTER_TOP,
                &format!(
                    "Load {} · {count} pieces · {} lb",
                    l.number,
                    thousands(l.weight_lb)
                ),
                18.0,
                false,
                pal.muted,
            ); // WHAT: the load. WHY: as mockup h
        }
        let rect = Rect::from_center_size(
            Pos2::new(c.x, c.y + 240.0),
            Vec2::new(560.0_f32.min(body.width() - 40.0), 120.0),
        ); // WHAT: who signed. WHY: the name with the signature
        card(&p, rect, pal.card, pal.line, 1.0); // WHAT: its card. WHY: as mockup h
        let sig = Rect::from_min_size(rect.min + Vec2::new(16.0, 14.0), Vec2::new(200.0, 70.0)); // WHAT: the signature, small. WHY: shown beside the name
        p.rect_filled(sig, CornerRadius::same(8), Color32::WHITE); // WHAT: paper. WHY: as on the pad
        draw_strokes(&p, sig, done.strokes.iter(), 2.0); // WHAT: the signature. WHY: what was signed
        words(
            &p,
            Pos2::new(sig.right() + 18.0, rect.top() + 22.0),
            Align2::LEFT_TOP,
            &format!("{}: {}", box_words(&done.which).to_uppercase(), done.signer),
            18.0,
            true,
            pal.ink,
        ); // WHAT: the name the server kept. WHY: as mockup h
        words(
            &p,
            Pos2::new(sig.right() + 18.0, rect.top() + 52.0),
            Align2::LEFT_TOP,
            &signed_words(&done.signed),
            16.0,
            false,
            pal.muted,
        ); // WHAT: when the server kept it. WHY: likewise
        let note = if delivered {
            "Sent to the office · the signed ticket files itself in Fabwire. The load shows Delivered there within a minute."
        } else {
            "Sent to the office · Fabwire writes it on the ticket within a minute."
        }; // WHAT: what happens next. WHY: the tick means the server has it
        wrapped(
            &p,
            Pos2::new(rect.left(), rect.bottom() + 20.0),
            rect.width(),
            note,
            16.0,
            false,
            pal.muted,
        ); // WHAT: drawn. WHY: as mockup h
        let ok = Rect::from_center_size(
            Pos2::new(c.x, rect.bottom() + 120.0),
            Vec2::new(280.0, MAIN),
        ); // WHAT: Done. WHY: back to the loads
        if button(ui, pal, &mut self.targets, ok, "Done", Look::Main, 22.0) {
            self.screen = Some(Screen::Shipping); // WHAT: back. WHY: finished
            self.done = None; // WHAT: put away. WHY: shown once
        }
    }

    /// The office's pins: a flat list.
    fn documents_screen(
        &mut self,
        ui: &egui::Ui,
        pal: &Palette,
        job: &TabletView,
        body: Rect,
        actions: &mut Vec<Action>,
    ) {
        let p = ui.painter().clone(); // WHAT: the painter. WHY: drawn by hand
        let docs = &job.manifest.documents; // WHAT: the pins. WHY: only what the office pinned
        if docs.is_empty() {
            words(
                &p,
                body.min + Vec2::new(26.0, 26.0),
                Align2::LEFT_TOP,
                "Nothing pinned for this job yet.",
                18.0,
                false,
                pal.muted,
            ); // WHAT: none. WHY: said
        }
        for (i, doc) in docs.iter().enumerate() {
            let rect = Rect::from_min_size(
                Pos2::new(body.left() + 26.0, body.top() + 22.0 + i as f32 * 100.0),
                Vec2::new(body.width() - 52.0, 88.0),
            ); // WHAT: a row. WHY: 88 tall
            let response = tap(ui, &mut self.targets, rect, &format!("doc-{}", doc.id)); // WHAT: tapped. WHY: opens it
            card(&p, rect, pal.card, pal.line, 1.0); // WHAT: the row. WHY: big rows, no folders
            icon(
                &p,
                if doc.kind == "set" {
                    "drawings"
                } else {
                    "documents"
                },
                Rect::from_min_size(rect.min + Vec2::new(16.0, 16.0), Vec2::splat(56.0)),
                pal.accent,
            ); // WHAT: what kind. WHY: found by eye
            words(
                &p,
                rect.min + Vec2::new(88.0, 20.0),
                Align2::LEFT_TOP,
                &doc.name,
                20.0,
                true,
                pal.ink,
            ); // WHAT: its name. WHY: the row
            words(
                &p,
                rect.min + Vec2::new(88.0, 52.0),
                Align2::LEFT_TOP,
                if doc.kind == "set" {
                    "From the office · opens here"
                } else {
                    "From the office · opens with its own program"
                },
                15.0,
                false,
                pal.muted,
            ); // WHAT: what tapping does. WHY: said
            chevron(
                &p,
                Pos2::new(rect.right() - 26.0, rect.center().y),
                true,
                pal.faint,
            ); // WHAT: ›. WHY: it opens
            if response.clicked() {
                actions.push(Action::Open {
                    set: doc.id.clone(),
                    name: doc.name.clone(),
                }); // WHAT: opened. WHY: a set in the viewer; a file by the window's file route
            }
        }
    }

    /// The bar over a sheet open from the tablet, the flip buttons, and the minimal toolbar (mockup d).
    pub fn viewer_chrome(
        &mut self,
        ctx: &egui::Context,
        env: &Env,
        tool: Tool,
        color: [u8; 4],
    ) -> Vec<ViewerAction> {
        let pal = if env.bright { BRIGHT } else { DARK }; // WHAT: the look. WHY: as the other screens
        let mut out = Vec::new(); // WHAT: what to do. WHY: returned
        let Some(job) = self.job.clone() else {
            return out;
        }; // WHAT: the job. WHY: the sheet's words
        let set = self.viewing.clone().unwrap_or_default(); // WHAT: the open sheet. WHY: its words and place in the list
        let sheets = sheets_shown(&job.manifest.sheets, 0, "", &self.seen); // WHAT: every released sheet by number. WHY: "3 of 42" and flipping
        let at = sheets.iter().position(|s| s.set == set); // WHAT: where this one is. WHY: flipping
        let sheet = at.map(|i| sheets[i]); // WHAT: the sheet itself, when it is one. WHY: the ticket or a pin is not
        egui::TopBottomPanel::top("tablet-viewer-bar")
            .exact_height(TOP)
            .frame(egui::Frame::NONE.fill(pal.bg))
            .show(ctx, |ui| {
                let bar = ui.max_rect(); // WHAT: the bar. WHY: laid out by hand
                let p = ui.painter().clone(); // WHAT: the painter. WHY: drawn by hand
                let back =
                    Rect::from_min_size(bar.min + Vec2::new(20.0, 8.0), Vec2::new(150.0, SMALLEST)); // WHAT: back. WHY: to where it was opened from
                if button(
                    ui,
                    &pal,
                    &mut self.targets,
                    back,
                    "‹ Back",
                    Look::Plain,
                    18.0,
                ) {
                    out.push(ViewerAction::Close); // WHAT: closed. WHY: back to the tablet's screens
                }
                words(
                    &p,
                    Pos2::new(back.right() + 16.0, bar.top() + 14.0),
                    Align2::LEFT_TOP,
                    &format!(
                        "{} · {}",
                        job.manifest.job.number,
                        job.manifest.job.name.to_uppercase()
                    ),
                    12.0,
                    true,
                    pal.gold,
                ); // WHAT: the job. WHY: the kicker
                let title = sheet.map_or_else(
                    || "Document".to_string(),
                    |s| format!("{} · {}", s.sheet, s.title),
                ); // WHAT: the sheet and its title. WHY: as mockup d
                let galley = p.layout(
                    title,
                    font(22.0, true),
                    pal.ink,
                    (bar.width() - 700.0).max(120.0),
                ); // WHAT: laid out. WHY: room for the chips
                p.galley(
                    Pos2::new(back.right() + 16.0, bar.top() + 30.0),
                    galley,
                    pal.ink,
                ); // WHAT: drawn. WHY: where you are
                if let (Some(s), Some(i)) = (sheet, at) {
                    let count = Rect::from_min_size(
                        Pos2::new(bar.right() - 160.0, bar.top() + 8.0),
                        Vec2::new(140.0, SMALLEST),
                    ); // WHAT: "3 of 42". WHY: as mockup d
                    card(&p, count, pal.card, pal.edge, 1.5); // WHAT: its card. WHY: likewise
                    words(
                        &p,
                        count.center(),
                        Align2::CENTER_CENTER,
                        &format!("{} of {}", i + 1, sheets.len()),
                        20.0,
                        true,
                        pal.ink,
                    ); // WHAT: where in the set. WHY: likewise
                    let released = chip(
                        &p,
                        Pos2::new(count.left() - 190.0, bar.top() + 17.0),
                        &format!("✓ Released {}", short_day(&s.released)),
                        pal.green,
                    ); // WHAT: the green chip. WHY: released, at a glance
                    chip(
                        &p,
                        Pos2::new(released.left() - 90.0, bar.top() + 17.0),
                        &format!("Rev {}", s.rev),
                        pal.purple,
                    ); // WHAT: the Rev chip. WHY: as mockup d
                }
            });
        egui::Area::new(egui::Id::new("tablet-viewer-tools"))
            .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -16.0))
            .show(ctx, |ui| {
                let tools: [(&str, Option<Tool>); 9] = [
                    ("Move", Some(Tool::Pan)),
                    ("Pen", Some(Tool::Ink)),
                    ("Highlight", Some(Tool::Highlight)),
                    ("Cloud", Some(Tool::Cloud)),
                    ("Note", Some(Tool::Text)),
                    ("Measure", Some(Tool::Length)),
                    ("Colour", None),
                    ("Undo", None),
                    ("More", None),
                ]; // WHAT: the nine buttons. WHY: everything else is under More
                let (rect, _) = ui.allocate_exact_size(
                    Vec2::new(tools.len() as f32 * 76.0 + 20.0, 84.0),
                    Sense::hover(),
                ); // WHAT: the toolbar. WHY: one floating bar
                let p = ui.painter().clone(); // WHAT: the painter. WHY: drawn by hand
                p.rect(
                    rect,
                    CornerRadius::same(22),
                    pal.card,
                    Stroke::new(1.0_f32, pal.line),
                    StrokeKind::Inside,
                ); // WHAT: its card. WHY: as mockup d
                for (i, (label, t)) in tools.iter().enumerate() {
                    let b = Rect::from_min_size(
                        Pos2::new(rect.left() + 10.0 + i as f32 * 76.0, rect.top() + 10.0),
                        Vec2::new(70.0, 64.0),
                    ); // WHAT: a 64-point button. WHY: the design's size
                    let on = t.is_some_and(|t| t == tool) || (*label == "More" && self.more); // WHAT: the tool in hand. WHY: lit
                    let response = tap(ui, &mut self.targets, b, &format!("tool-{label}")); // WHAT: tapped. WHY: below
                    if on {
                        p.rect_filled(b, CornerRadius::same(14), pal.accent); // WHAT: lit. WHY: the tool in hand
                    }
                    let ink = if on { pal.accent_ink } else { pal.ink }; // WHAT: its colour. WHY: readable either way
                    if *label == "Colour" {
                        p.circle_filled(
                            Pos2::new(b.center().x, b.top() + 22.0),
                            13.0,
                            Color32::from_rgba_unmultiplied(color[0], color[1], color[2], 255),
                        ); // WHAT: the pen's colour. WHY: tapped to change it
                    } else {
                        words(
                            &p,
                            Pos2::new(b.center().x, b.top() + 22.0),
                            Align2::CENTER_CENTER,
                            &label[..1],
                            22.0,
                            true,
                            ink,
                        ); // WHAT: its letter. WHY: a glyph above the word
                    }
                    words(
                        &p,
                        Pos2::new(b.center().x, b.bottom() - 12.0),
                        Align2::CENTER_CENTER,
                        label,
                        13.0,
                        true,
                        ink,
                    ); // WHAT: its word. WHY: never an icon alone
                    if response.clicked() {
                        out.push(match (label, t) {
                            (_, Some(t)) => ViewerAction::Tool(*t), // WHAT: a tool. WHY: in hand
                            (&"Colour", None) => ViewerAction::Colour, // WHAT: the next colour. WHY: one tap
                            (&"Undo", None) => ViewerAction::Undo,     // WHAT: undo. WHY: one tap
                            _ => ViewerAction::More, // WHAT: the rest of the tool chest. WHY: More
                        });
                    }
                }
            });
        if let Some(i) = at {
            for (k, side) in [(-1i32, Align2::LEFT_CENTER), (1, Align2::RIGHT_CENTER)] {
                let to = i as i32 + k; // WHAT: the sheet that way. WHY: flipping
                if to < 0 || to as usize >= sheets.len() {
                    continue; // WHAT: none that way. WHY: no button
                }
                egui::Area::new(egui::Id::new(("tablet-flip", k)))
                    .anchor(side, Vec2::new(-16.0 * k as f32, 0.0))
                    .show(ctx, |ui| {
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::splat(64.0), Sense::click()); // WHAT: a round 64-point button. WHY: big ‹ › at the sides
                        self.targets.push((rect, format!("flip{k}"))); // WHAT: remembered. WHY: tested
                        ui.painter().circle_filled(rect.center(), 32.0, pal.card); // WHAT: its circle. WHY: as mockup d
                        chevron(ui.painter(), rect.center(), k > 0, pal.ink); // WHAT: ‹ or ›. WHY: which way
                        if response.clicked() {
                            out.push(ViewerAction::Flip(sheets[to as usize].set.clone()));
                            // WHAT: that sheet. WHY: flipped
                        }
                    });
            }
        }
        out
    }
}

/// What the viewer's tablet chrome asks of the window.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewerAction {
    Close,        // WHAT: back to the tablet's screens. WHY: the bar's back button
    Tool(Tool),   // WHAT: a tool in hand. WHY: the toolbar
    Colour,       // WHAT: the next pen colour. WHY: the toolbar
    Undo,         // WHAT: undo. WHY: the toolbar
    More,         // WHAT: the rest of the tool chest. WHY: More
    Flip(String), // WHAT: open that sheet instead. WHY: ‹ ›
}

/// Strokes in the pad's units drawn into a rect.
fn draw_strokes<'a>(
    p: &egui::Painter,
    rect: Rect,
    strokes: impl Iterator<Item = &'a Vec<[f64; 2]>>,
    width: f32,
) {
    for stroke in strokes {
        let points: Vec<Pos2> = stroke
            .iter()
            .map(|q| {
                Pos2::new(
                    rect.left() + (q[0] / hub::SIGN_PAD.0) as f32 * rect.width(),
                    rect.top() + (q[1] / hub::SIGN_PAD.1) as f32 * rect.height(),
                )
            })
            .collect(); // WHAT: on the screen. WHY: drawn where made
        p.add(egui::Shape::line(points, Stroke::new(width, hex(0x111827)))); // WHAT: dark ink. WHY: as the printed picture
    }
}

/// The sections on for this person, in order.
fn on_sections(job: &TabletView) -> Vec<(&'static str, &'static str)> {
    let m = &job.manifest; // WHAT: the manifest. WHY: its sections
    SECTIONS
        .iter()
        .copied()
        .filter(|(k, _)| {
            *k == "drawings"
                || m.sections
                    .shop
                    .iter()
                    .chain(&m.sections.field)
                    .any(|s| s == k)
        })
        .collect() // WHAT: drawings always, the rest as the server cut them. WHY: only theirs
}

/// The section a screen belongs to.
fn section_of(screen: &Screen) -> &'static str {
    match screen {
        Screen::Drawings => "drawings", // WHAT: Drawings. WHY: the rail
        Screen::Shipping | Screen::Ticket(_) | Screen::Pad | Screen::Done => "shipping", // WHAT: Shipping. WHY: likewise
        Screen::Documents => "documents", // WHAT: Documents. WHY: likewise
        _ => "",                          // WHAT: none. WHY: home and the list
    }
}

/// How many things wait in a section, for the rail's dot.
fn waiting(job: &TabletView, key: &str) -> usize {
    match key {
        "shipping" => job
            .manifest
            .loads
            .iter()
            .filter(|l| l.boxes.iter().any(|b| open_box(b, holder(job))))
            .count(), // WHAT: tickets to sign. WHY: the amber dot
        "traveler" => job
            .manifest
            .traveler
            .as_ref()
            .map_or(0, |t| t.open_stations as usize), // WHAT: stations to sign. WHY: likewise
        _ => 0, // WHAT: nothing waits. WHY: no dot
    }
}

/// A tile's line and its chip.
fn tile_words(
    job: &TabletView,
    key: &str,
    pal: &Palette,
    seen: &BTreeSet<String>,
) -> (String, Option<(String, (Color32, Color32))>) {
    let m = &job.manifest; // WHAT: the manifest. WHY: counted
    match key {
        "drawings" => {
            let new = m.sheets.iter().filter(|s| !seen.contains(&s.set)).count(); // WHAT: not opened here. WHY: the chip
            (
                format!("{} sheets released for fabrication", m.sheets.len()),
                (new > 0).then(|| (format!("{new} new"), pal.purple)),
            ) // WHAT: the count and the new ones. WHY: as mockup b
        }
        "shipping" => {
            let to_sign = waiting(job, "shipping"); // WHAT: tickets to sign. WHY: the chip
            let line = m
                .loads
                .iter()
                .take(2)
                .map(|l| {
                    format!(
                        "Load {} {}",
                        l.number,
                        if l.ships.is_empty() {
                            String::new()
                        } else {
                            short_day(&l.ships)
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join(" · "); // WHAT: the next loads. WHY: as mockup b
            (
                if line.is_empty() {
                    "No loads this week".into()
                } else {
                    line
                },
                (to_sign > 0).then(|| {
                    (
                        format!(
                            "{to_sign} ticket{} to sign",
                            if to_sign == 1 { "" } else { "s" }
                        ),
                        pal.amber,
                    )
                }),
            ) // WHAT: the line and the chip. WHY: likewise
        }
        "traveler" => match &m.traveler {
            Some(t) => (
                format!("{} pieces · {} complete", t.pieces, t.complete),
                (t.open_stations > 0)
                    .then(|| (format!("{} stations to sign", t.open_stations), pal.amber)),
            ), // WHAT: the counts. WHY: as mockup b
            None => ("No traveler yet".into(), None), // WHAT: none. WHY: said
        },
        "loading" => match &m.loading {
            Some(l) => (
                format!(
                    "Load {} · {} of {} lb",
                    l.load,
                    thousands(l.weight_lb),
                    thousands(l.capacity_lb)
                ),
                (l.ready_pieces > 0).then(|| {
                    (
                        format!("{} pieces ready to load", l.ready_pieces),
                        (pal.accent, pal.accent_soft),
                    )
                }),
            ), // WHAT: the load's weight. WHY: as mockup b
            None => ("Nothing loading".into(), None), // WHAT: none. WHY: said
        },
        _ => (
            format!("{} from the office", m.documents.len()),
            m.documents
                .first()
                .map(|d| (d.name.clone(), (pal.muted, pal.card2))),
        ), // WHAT: the pins. WHY: as mockup b
    }
}

/// "SK" from "Sam Kline".
fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase() // WHAT: the first letters of two words. WHY: the badge
}

/// "Good morning" and the rest, by the tablet's clock.
fn greet() -> &'static str {
    use chrono::Timelike as _; // WHAT: the hour. WHY: below
    match chrono::Local::now().hour() {
        0..=11 => "Good morning,",    // WHAT: morning. WHY: plain
        12..=16 => "Good afternoon,", // WHAT: afternoon. WHY: plain
        _ => "Good evening,",         // WHAT: evening. WHY: plain
    }
}

/// "10:42 AM" now, on the tablet's clock.
fn now_clock() -> String {
    chrono::Local::now().format("%-I:%M %p").to_string() // WHAT: the time. WHY: "Up to date with the office"
}

/// "Signed 2:14 PM, 11/23" from the server's RFC 3339 time, on the tablet's clock.
fn signed_words(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339).map_or_else(
        |_| "Signed".into(),
        |t| {
            t.with_timezone(&chrono::Local)
                .format("Signed %-I:%M %p, %m/%d/%Y")
                .to_string()
        },
    ) // WHAT: the server's time, read in local time. WHY: the time the office server kept
}

/// The world as the screens need it, now.
#[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
pub fn env_now(person: &str, connected: bool, bright: bool, desktop: bool) -> Env {
    let now = chrono::Local::now(); // WHAT: the tablet's clock. WHY: the date on the pad is set by the tablet
    Env {
        person: person.to_string(),
        connected,
        bright,
        desktop,
        today: now.format("%Y-%m-%d").to_string(),
        date_words: now.format("%a %m/%d/%Y · %-I:%M %p").to_string(),
        clock: now.format("%-I:%M %p").to_string(),
    } // WHAT: the env. WHY: drawn from
}

/// A connection trouble in plain words.
#[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
pub fn trouble_words(e: &hub::client::Trouble) -> String {
    match e {
        hub::client::Trouble::Unreachable(_) => {
            "No connection: signing needs the office. Try again near the trailer or office Wi-Fi."
                .to_string()
        } // WHAT: offline. WHY: said plainly
        other => other.to_string(), // WHAT: the server's own words. WHY: already a sentence
    }
}

impl App {
    /// Whether this window shows the tablet's screens: iPads and Android, a
    /// shop or field account, or Tablet mode turned on here; never against an
    /// office server too old to have them.
    #[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
    pub fn tablet_mode(&self) -> bool {
        if self.tablet.unsupported || self.tablet.desktop || !self.standing.signed_in() {
            return false; // WHAT: no tablet routes, or nobody signed in. WHY: the desktop layout, and its sign-in
        }
        crate::platform::tablet()
            || self.prefs.tablet_mode
            || self
                .standing
                .who
                .as_ref()
                .is_some_and(|w| w.role == hub::Role::Viewer) // WHAT: a tablet, asked for, or a shop or field account. WHY: design §6
    }

    /// The env for this frame.
    fn tablet_env(&self) -> Env {
        let person = self
            .standing
            .who
            .as_ref()
            .map(|w| w.name.clone())
            .unwrap_or_default(); // WHAT: who is signed in. WHY: greeted
        env_now(
            &person,
            self.standing.resume_tries == 0,
            self.prefs.tablet_bright,
            !crate::platform::tablet(),
        ) // WHAT: now. WHY: drawn from
    }

    /// Whether a drawing opened from the tablet is in front.
    #[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
    pub fn tablet_viewing(&self) -> bool {
        self.tablet.viewing.as_ref().is_some_and(|set| {
            self.doc()
                .and_then(|d| d.attached.as_ref())
                .is_some_and(|a| &a.set == set)
        }) // WHAT: the open drawing is the tablet's. WHY: its own bar and toolbar
    }

    /// The tablet's screens, when no drawing from it is in front.
    pub fn tablet_screens(&mut self, ctx: &egui::Context) {
        if self.tablet.jobs.is_none()
            && self.tablet.said.is_none()
            && self.tablet.asked_at.is_none()
        {
            self.tablet.seen = self.prefs.tablet_seen.iter().cloned().collect(); // WHAT: the sheets opened before. WHY: NEW only for the rest
            self.tablet.asked_at = Some(std::time::Instant::now()); // WHAT: asked. WHY: once
            self.ask(crate::server::Ask::TabletJobs); // WHAT: the job list. WHY: the first screen
        }
        if self
            .tablet
            .asked_at
            .is_some_and(|t| t.elapsed() > std::time::Duration::from_secs(30))
        {
            self.tablet.asked_at = Some(std::time::Instant::now()); // WHAT: asked again. WHY: every half minute
            match (
                self.tablet.job.as_ref().map(|j| j.project.clone()),
                self.tablet.pad.is_none(),
            ) {
                (Some(project), true) => self.ask(crate::server::Ask::Tablet(project)), // WHAT: the open job. WHY: a box signed elsewhere shows
                (Some(_), false) => {} // WHAT: signing. WHY: nothing changes under the pad
                (None, _) => self.ask(crate::server::Ask::TabletJobs), // WHAT: the list. WHY: what needs doing changes
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(30)); // WHAT: woken to ask again. WHY: kept fresh without spinning
        let env = self.tablet_env(); // WHAT: now. WHY: drawn from
        let actions = self.tablet.show(ctx, &env); // WHAT: drawn. WHY: and what to do
        self.tablet_do(actions); // WHAT: done. WHY: below
    }

    /// Carries out what the screens asked.
    fn tablet_do(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Jobs => self.ask(crate::server::Ask::TabletJobs), // WHAT: the list. WHY: asked
                Action::Job(project) => self.ask(crate::server::Ask::Tablet(project)), // WHAT: one job. WHY: asked
                Action::Open { set, name } if set.starts_with("file_") => {
                    self.ask(crate::server::Ask::OpenFile { file: set, name })
                } // WHAT: a pinned file. WHY: opened with its own program, as the Studio does
                Action::Open { set, name } => {
                    self.tablet.viewing = Some(set.clone()); // WHAT: the tablet's drawing. WHY: its own chrome
                    self.prefs.tablet_seen = self.tablet.seen.iter().cloned().collect(); // WHAT: seen kept. WHY: NEW survives a restart
                    let _ = self.prefs.save(); // WHAT: written. WHY: likewise; a failure only costs a badge
                    self.ask(crate::server::Ask::Open { set, name }); // WHAT: fetched and opened. WHY: View's own viewer
                }
                Action::Sign {
                    project,
                    id,
                    name,
                    strokes,
                } => self.ask(crate::server::Ask::TabletSign {
                    project,
                    id,
                    name,
                    strokes,
                }), // WHAT: signed. WHY: the existing line
                Action::Bright => {
                    self.prefs.tablet_bright = !self.prefs.tablet_bright; // WHAT: switched. WHY: the button
                    let _ = self.prefs.save(); // WHAT: kept. WHY: a field tablet stays bright
                }
                Action::Desktop => {
                    self.tablet.desktop = true; // WHAT: off for this session, even for a shop or field account. WHY: Desktop must always work on a desktop
                    self.prefs.tablet_mode = false; // WHAT: off here. WHY: the desktop layout
                    let _ = self.prefs.save(); // WHAT: kept. WHY: likewise
                }
            }
        }
    }

    /// The tablet's bar and toolbar over a drawing it opened.
    pub fn tablet_viewer(&mut self, ctx: &egui::Context) {
        let env = self.tablet_env(); // WHAT: now. WHY: the look
        let actions = self.tablet.viewer_chrome(ctx, &env, self.tool, self.color); // WHAT: drawn. WHY: and what to do
        for action in actions {
            match action {
                ViewerAction::Close => {
                    self.close_tab(self.current); // WHAT: the drawing closed. WHY: back to the tablet's screens
                    self.tablet.viewing = None; // WHAT: none open. WHY: likewise
                    self.tablet.more = false; // WHAT: More put away. WHY: likewise
                }
                ViewerAction::Tool(tool) => self.tool = tool, // WHAT: in hand. WHY: tapped
                ViewerAction::Colour => {
                    const COLOURS: [[u8; 4]; 4] = [
                        [220, 38, 38, 255],
                        [37, 99, 235, 255],
                        [22, 163, 74, 255],
                        [17, 24, 39, 255],
                    ]; // WHAT: red, blue, green, black. WHY: the field's few
                    let at = COLOURS
                        .iter()
                        .position(|c| *c == self.color)
                        .map_or(0, |i| (i + 1) % COLOURS.len()); // WHAT: the next. WHY: one tap each
                    self.color = COLOURS[at]; // WHAT: set. WHY: the next markup
                }
                ViewerAction::Undo => self.chrome.fire("Edit.Undo"), // WHAT: undo. WHY: the same command as the desktop's
                ViewerAction::More => self.tablet.more = !self.tablet.more, // WHAT: the rest of the tool chest. WHY: More
                ViewerAction::Flip(set) => {
                    self.close_tab(self.current); // WHAT: this sheet closed. WHY: one sheet at a time on a tablet
                    let mut actions = Vec::new(); // WHAT: the open. WHY: below
                    self.tablet.open_sheet(&set, &mut actions); // WHAT: marked seen and asked. WHY: flipped
                    self.tablet_do(actions); // WHAT: done. WHY: likewise
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hub::tablet::{BoxLine, Manifest};

    /// The shared steel-job example as a tablet in the field sees it.
    pub(crate) fn steel_view() -> TabletView {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/tablet-manifest-vectors.json"); // WHAT: the shared examples. WHY: the same file both sides test
        let all: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap(); // WHAT: read. WHY: below
        let mut manifest: Manifest =
            serde_json::from_value(all["valid"][0]["manifest"].clone()).unwrap(); // WHAT: the steel job. WHY: a real-looking job
        manifest.loads[0].boxes[2].line = Some(BoxLine {
            id: "so_1".into(),
            status: "open".into(),
            signer: String::new(),
            signed: String::new(),
        }); // WHAT: the receiver's line, open. WHY: as the server answers
        TabletView {
            project: "prj_1".into(),
            version: 1,
            updated: "2026-11-23T10:42:00Z".into(),
            shop: false,
            field: true,
            manifest,
        } // WHAT: as the server answers a field tablet. WHY: under test
    }

    /// One frame drawn at a size, with a click at `click` if given: the tablet after it.
    fn frame(tablet: &mut Tablet, size: Vec2, click: Option<Pos2>) -> Vec<Action> {
        let ctx = egui::Context::default(); // WHAT: a fresh context. WHY: no window needed
        ui::chrome::fonts(&ctx); // WHAT: Inter and the rest. WHY: as the program has them
        let env = Env {
            person: "Sam Kline".into(),
            connected: true,
            today: "2026-11-23".into(),
            date_words: "Mon 11/23/2026 · 2:14 PM".into(),
            clock: "10:42 AM".into(),
            ..Env::default()
        }; // WHAT: a fixed world. WHY: the same every run
        let mut actions = Vec::new(); // WHAT: what was asked. WHY: returned
        for pass in 0..3 {
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                ..Default::default()
            }; // WHAT: the screen. WHY: its size
            if let (Some(at), 1) = (click, pass) {
                input.events = vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    },
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::default(),
                    },
                ]; // WHAT: a tap. WHY: the second pass, once laid out
            }
            let _ = ctx.run(input, |ctx| actions.extend(tablet.show(ctx, &env)));
            // WHAT: a frame. WHY: drawn and answered
        }
        actions
    }

    #[test]
    fn nothing_on_any_screen_is_smaller_than_52_points() {
        for size in [Vec2::new(1024.0, 768.0), Vec2::new(390.0, 844.0)] {
            let mut tablet = Tablet {
                jobs: Some(vec![TabletJob {
                    project: "prj_1".into(),
                    number: "2601".into(),
                    name: "TCU Campus 2".into(),
                    town: "Fort Worth, TX".into(),
                    shop: false,
                    field: true,
                    todo: vec!["Load 2 is on site: sign the ticket".into()],
                    updated: String::new(),
                }]),
                ..Tablet::default()
            }; // WHAT: the job list. WHY: the first screen
            for screen in [
                None,
                Some(Screen::Home),
                Some(Screen::Drawings),
                Some(Screen::Shipping),
                Some(Screen::Ticket(2)),
                Some(Screen::Pad),
                Some(Screen::Documents),
            ] {
                tablet.job = Some(steel_view()); // WHAT: the steel job. WHY: every screen has something
                tablet.screen = screen.clone(); // WHAT: this screen. WHY: each in turn
                tablet.pad = Some(Pad {
                    load: 2,
                    which: "received_by".into(),
                    line: "so_1".into(),
                    ..Pad::default()
                }); // WHAT: a pad. WHY: for the pad screen
                frame(&mut tablet, size, None); // WHAT: drawn. WHY: its targets recorded
                assert!(!tablet.targets.is_empty(), "{screen:?} drew no targets"); // WHAT: something to tap. WHY: a screen with no way on is a dead end
                for (rect, id) in &tablet.targets {
                    assert!(
                        rect.width() >= SMALLEST - 0.01 && rect.height() >= SMALLEST - 0.01,
                        "{screen:?} at {size:?}: {id} is {rect:?}, under 52 points"
                    ); // WHAT: big enough for a glove. WHY: the design's rule
                }
            }
        }
    }

    #[test]
    fn the_receivers_name_starts_empty_and_the_shops_own_box_is_prefilled() {
        let mut tablet = Tablet {
            job: Some(steel_view()),
            screen: Some(Screen::Ticket(2)),
            ..Tablet::default()
        }; // WHAT: load 2's ticket. WHY: its receiver's box is open
        let load = tablet.job.as_ref().unwrap().manifest.loads[0].clone(); // WHAT: the load. WHY: its boxes
        tablet.open_pad(2, &load.boxes[2], "Sam Kline"); // WHAT: Received by. WHY: the GC's person signs
        assert_eq!(
            tablet.pad.as_ref().unwrap().name,
            "",
            "a receiver types their own name"
        ); // WHAT: empty. WHY: the design's fix
        tablet.open_pad(2, &load.boxes[0], "Sam Kline"); // WHAT: Loaded by. WHY: the shop's own box
        assert_eq!(tablet.pad.as_ref().unwrap().name, "Sam Kline"); // WHAT: prefilled. WHY: their own name
    }

    #[test]
    fn the_big_button_signs_the_open_receiver_line_and_waits_for_the_server() {
        let mut tablet = Tablet {
            job: Some(steel_view()),
            screen: Some(Screen::Ticket(2)),
            ..Tablet::default()
        }; // WHAT: load 2's ticket. WHY: under test
        frame(&mut tablet, Vec2::new(1024.0, 768.0), None); // WHAT: drawn once. WHY: its targets found
        let main = tablet
            .targets
            .iter()
            .find(|(_, id)| id == "Sign: received by")
            .expect("the big button")
            .0; // WHAT: the 76-point button. WHY: tapped
        assert!(
            main.height() >= MAIN - 0.01,
            "the main action is 76 points tall"
        ); // WHAT: its size. WHY: the design
        frame(&mut tablet, Vec2::new(1024.0, 768.0), Some(main.center())); // WHAT: tapped. WHY: the pad opens
        assert_eq!(tablet.screen, Some(Screen::Pad)); // WHAT: the pad. WHY: one tap from the ticket
        let pad = tablet.pad.as_mut().unwrap(); // WHAT: the pad. WHY: filled in below
        pad.strokes = vec![vec![[20.0, 100.0], [80.0, 30.0]]]; // WHAT: a signature. WHY: drawn
        pad.name = "Dana Whitfield".into(); // WHAT: the receiver's name. WHY: typed
        frame(&mut tablet, Vec2::new(1024.0, 768.0), None); // WHAT: drawn. WHY: its targets found
        let go = tablet
            .targets
            .iter()
            .find(|(_, id)| id == "✓ Sign and mark delivered")
            .expect("the green button")
            .0; // WHAT: the green button. WHY: tapped
        let actions = frame(&mut tablet, Vec2::new(1024.0, 768.0), Some(go.center())); // WHAT: tapped. WHY: sent
        assert!(
            actions.contains(&Action::Sign {
                project: "prj_1".into(),
                id: "so_1".into(),
                name: "Dana Whitfield".into(),
                strokes: vec![vec![[20.0, 100.0], [80.0, 30.0]]]
            }),
            "{actions:?}"
        ); // WHAT: the existing line signed with the typed name. WHY: no parallel path
        assert_eq!(
            tablet.screen,
            Some(Screen::Pad),
            "no tick until the server has it"
        ); // WHAT: still the pad. WHY: final once the server has it
        let mut after = steel_view(); // WHAT: the server's answer. WHY: the line signed
        after.manifest.loads[0].boxes[2].line = Some(BoxLine {
            id: "so_1".into(),
            status: "signed".into(),
            signer: "Dana Whitfield".into(),
            signed: "2026-11-23T21:14:00Z".into(),
        }); // WHAT: signed, as the server has it. WHY: the Delivered screen
        tablet.signed(after); // WHAT: answered. WHY: below
        assert_eq!(tablet.screen, Some(Screen::Done)); // WHAT: Delivered ✓. WHY: the server has it
        assert_eq!(tablet.done.as_ref().unwrap().signer, "Dana Whitfield"); // WHAT: the server's name. WHY: shown
    }

    #[test]
    fn sheets_are_by_number_filtered_and_found_by_piece_mark() {
        let view = steel_view(); // WHAT: the steel job. WHY: four sheets
        let seen = BTreeSet::from(["set_s101".to_string()]); // WHAT: one opened. WHY: New leaves it out
        let numbers = |list: Vec<&Sheet>| list.iter().map(|s| s.sheet.clone()).collect::<Vec<_>>(); // WHAT: their numbers. WHY: compared
        assert_eq!(
            numbers(sheets_shown(&view.manifest.sheets, 0, "", &seen)),
            ["AB-1", "E-101", "S-101", "S-102"]
        ); // WHAT: all, by number. WHY: the list
        assert_eq!(
            numbers(sheets_shown(&view.manifest.sheets, 1, "", &seen)),
            ["AB-1", "E-101", "S-102"]
        ); // WHAT: New. WHY: not opened
        assert_eq!(
            numbers(sheets_shown(&view.manifest.sheets, 3, "", &seen)),
            ["E-101"]
        ); // WHAT: Erection. WHY: by discipline
        assert_eq!(
            numbers(sheets_shown(&view.manifest.sheets, 0, "b12", &seen)),
            ["S-101"]
        ); // WHAT: a piece mark. WHY: "B12" finds its sheet
        assert!(natural("S-2") < natural("S-10")); // WHAT: numbers as numbers. WHY: S-2 before S-10
    }

    #[test]
    fn a_signature_the_server_did_not_keep_never_shows_delivered() {
        let mut tablet = Tablet {
            job: Some(steel_view()),
            screen: Some(Screen::Pad),
            pad: Some(Pad {
                load: 2,
                which: "received_by".into(),
                line: "so_1".into(),
                sending: true,
                ..Pad::default()
            }),
            ..Tablet::default()
        }; // WHAT: sent. WHY: waiting
        tablet.signed(steel_view()); // WHAT: answered, still open. WHY: the server didn't keep it
        assert_eq!(tablet.screen, Some(Screen::Pad)); // WHAT: still the pad. WHY: never a tick the server can't back
        assert!(tablet.pad.as_ref().unwrap().said.is_some()); // WHAT: said why. WHY: never quietly
        assert_eq!(thousands(18450), "18,450"); // WHAT: weights read. WHY: as the ticket
    }

    #[test]
    fn a_tablet_is_offered_only_the_sign_buttons_its_account_may_use() {
        let mut view = steel_view(); // WHAT: the steel job. WHY: load 2 has the three boxes.
        for b in &mut view.manifest.loads[0].boxes {
            b.line = Some(BoxLine { id: format!("so_{}", b.which), status: "open".into(), signer: String::new(), signed: String::new() }); // WHAT: every box's line open. WHY: each is offered or not by its account alone
        }
        let load = &view.manifest.loads[0]; // WHAT: load 2. WHY: under test.
        let open = |shop: bool, field: bool| -> Vec<&str> {
            let who = Audience { shop, field }; // WHAT: the account's audience. WHY: what the tablet is told
            load.boxes.iter().filter(|b| open_box(b, who)).map(|b| b.which.as_str()).collect() // WHAT: the boxes with a Sign button. WHY: the rule's own answer
        };
        assert_eq!(open(true, false), ["loaded_by"]); // WHAT: the shop. WHY: Loaded by only.
        assert_eq!(open(false, true), ["loaded_by", "driver", "received_by"]); // WHAT: the field. WHY: all three.
        assert_eq!(open(true, true), ["loaded_by", "driver", "received_by"]); // WHAT: the office, which is both. WHY: all three.
        assert_eq!(open(false, false), Vec::<&str>::new()); // WHAT: neither crew. WHY: none.
        assert!(next_box(load, true, Audience { shop: true, field: false }).is_some_and(|b| b.which == "loaded_by")); // WHAT: the shop's big button. WHY: skips the boxes it may not sign.
    }
}
