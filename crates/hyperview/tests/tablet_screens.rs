//! The tablet's screens drawn for real, by egui, and turned into pictures
//! without a graphics card: egui's own triangles filled in here, the font
//! atlas and all. Each screen of phase 1 (docs/TABLET-MANIFEST.md) is drawn
//! at an iPad's size (1024 by 768 points, at 2x) and a phone's, from the
//! shared steel-job example.
//!
//! Always checked: every screen draws, fills the window, and keeps every tap
//! target at 52 points or more. With `TABLET_SHOTS=<folder>` set, the
//! pictures are written there as PNGs (the screenshots for the owner).

// WHAT: the lints a software rasteriser is excused from, for this file only. WHY: pixels are turned
// between f32 and counts on a picture a few thousand wide, and a triangle's corners read best as p, v, w0...
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::many_single_char_names
)]

use egui::{Color32, Pos2, Rect, Vec2}; // WHAT: egui's geometry. WHY: the screen and its triangles.
use hub::tablet::{BoxLine, Manifest, TabletJob, TabletView};
use hyperview::tablet::{Done, Env, Pad, Screen, Tablet, SMALLEST}; // WHAT: the tablet's screens. WHY: under test. // WHAT: the manifest's shapes. WHY: the screens' data.

/// The shared steel job, as the server answers a field tablet.
fn steel() -> TabletView {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/tablet-manifest-vectors.json"); // WHAT: the shared examples. WHY: the same file Fabwire tests
    let all: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap(); // WHAT: read. WHY: below
    let mut manifest: Manifest =
        serde_json::from_value(all["valid"][0]["manifest"].clone()).unwrap(); // WHAT: the steel job. WHY: realistic
    manifest.loads[0].boxes[2].line = Some(BoxLine {
        id: "so_1".into(),
        status: "open".into(),
        signer: String::new(),
        signed: String::new(),
    }); // WHAT: the receiver's line open. WHY: as the server answers
    for b in &mut manifest.loads[1].boxes {
        b.line = Some(BoxLine {
            id: format!("so_{}", b.which),
            status: "open".into(),
            signer: String::new(),
            signed: String::new(),
        }); // WHAT: load 3's boxes open. WHY: the shop signs them
    }
    TabletView {
        project: "prj_1".into(),
        version: 4,
        updated: "2026-11-23T10:42:00Z".into(),
        shop: true,
        field: true,
        manifest,
    } // WHAT: a person in both crews. WHY: every tile shows
}

/// The job list the server would give.
fn jobs() -> Vec<TabletJob> {
    let job = |project: &str,
               number: &str,
               name: &str,
               town: &str,
               shop: bool,
               field: bool,
               todo: &[&str]| TabletJob {
        project: project.into(),
        number: number.into(),
        name: name.into(),
        town: town.into(),
        shop,
        field,
        todo: todo.iter().map(|t| (*t).to_string()).collect(),
        updated: String::new(),
    }; // WHAT: one card. WHY: short to write
    vec![
        job(
            "prj_1",
            "2601",
            "TCU Campus 2",
            "Fort Worth, TX",
            true,
            true,
            &["2 tickets to sign", "Load 2 ships today"],
        ),
        job(
            "prj_2",
            "2588",
            "Lakeside Clinic Ph 2",
            "Weatherford, TX",
            true,
            false,
            &["14 stations to sign"],
        ),
        job(
            "prj_3",
            "2612",
            "Westgate Parking Garage",
            "Arlington, TX",
            true,
            false,
            &["Load 1 loading · 46%"],
        ),
        job(
            "prj_4",
            "2594",
            "Riverside Elementary Canopy",
            "Grapevine, TX",
            false,
            true,
            &[],
        ),
    ] // WHAT: four jobs. WHY: as mockup a
}

/// One screen drawn: its picture, and the tablet after.
fn draw(tablet: &mut Tablet, size: Vec2, bright: bool) -> image::RgbaImage {
    let ctx = egui::Context::default(); // WHAT: a fresh context. WHY: no window
    ui::chrome::fonts(&ctx); // WHAT: Inter and the rest. WHY: as the program has them
    let env = Env {
        person: "Sam Kline".into(),
        connected: true,
        bright,
        desktop: false,
        today: "2026-11-23".into(),
        date_words: "Mon 11/23/2026 · 2:14 PM".into(),
        clock: "10:42 AM".into(),
    }; // WHAT: a fixed world. WHY: the same pictures every run
    let ppp = 2.0; // WHAT: two pixels a point. WHY: an iPad's Retina screen, as the mockups
    let mut textures: std::collections::HashMap<egui::TextureId, (usize, usize, Vec<Color32>)> =
        std::collections::HashMap::new(); // WHAT: the textures egui made. WHY: the font atlas is sampled
    let mut output = None; // WHAT: the last frame. WHY: drawn
    for _ in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            viewports: std::iter::once((
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    native_pixels_per_point: Some(ppp),
                    ..Default::default()
                },
            ))
            .collect(),
            ..Default::default()
        }; // WHAT: the screen at 2x. WHY: sharp pictures
        let out = ctx.run(input, |ctx| {
            let _ = tablet.show(ctx, &env); // WHAT: the screen. WHY: under test
        });
        for (id, delta) in &out.textures_delta.set {
            let egui::ImageData::Color(image) = &delta.image; // WHAT: the pixels. WHY: kept
            let (w, h) = (image.size[0], image.size[1]); // WHAT: its size. WHY: placed below
            match delta.pos {
                None => {
                    textures.insert(*id, (w, h, image.pixels.clone())); // WHAT: a whole texture. WHY: new
                }
                Some([x0, y0]) => {
                    let t = textures
                        .get_mut(id)
                        .expect("a texture updated before it was made"); // WHAT: the texture. WHY: a part of it changes
                    for y in 0..h {
                        for x in 0..w {
                            t.2[(y0 + y) * t.0 + x0 + x] = image.pixels[y * w + x];
                            // WHAT: the part written in. WHY: as a renderer would
                        }
                    }
                }
            }
        }
        output = Some(out); // WHAT: kept. WHY: the last pass is drawn
    }
    let out = output.unwrap(); // WHAT: the last frame. WHY: drawn
    let primitives = ctx.tessellate(out.shapes, ppp); // WHAT: triangles. WHY: filled below
    let (pw, ph) = ((size.x * ppp) as usize, (size.y * ppp) as usize); // WHAT: the picture's size. WHY: 2048 by 1536 for an iPad
    let mut px = vec![[0.0f32; 4]; pw * ph]; // WHAT: the picture, premultiplied, 0 to 1. WHY: blended into
    for clipped in primitives {
        let egui::epaint::Primitive::Mesh(mesh) = clipped.primitive else {
            continue;
        }; // WHAT: a mesh. WHY: callbacks are never used here
        let Some(tex) = textures.get(&mesh.texture_id) else {
            continue;
        }; // WHAT: its texture. WHY: sampled
        let clip = clipped.clip_rect; // WHAT: its clip, in points. WHY: nothing drawn outside
        let (cx0, cy0) = (
            (clip.min.x * ppp).max(0.0) as usize,
            (clip.min.y * ppp).max(0.0) as usize,
        ); // WHAT: the clip's top left in pixels. WHY: below
        let (cx1, cy1) = (
            ((clip.max.x * ppp).min(pw as f32)).max(0.0) as usize,
            ((clip.max.y * ppp).min(ph as f32)).max(0.0) as usize,
        ); // WHAT: its bottom right. WHY: below
        for tri in mesh.indices.as_chunks::<3>().0 {
            let v = [
                mesh.vertices[tri[0] as usize],
                mesh.vertices[tri[1] as usize],
                mesh.vertices[tri[2] as usize],
            ]; // WHAT: the three corners. WHY: filled
            let p: Vec<[f32; 2]> = v.iter().map(|q| [q.pos.x * ppp, q.pos.y * ppp]).collect(); // WHAT: in pixels. WHY: below
            let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]); // WHAT: twice its area, signed. WHY: barycentric weights
            if area.abs() < 1e-6 {
                continue; // WHAT: no area. WHY: nothing to fill
            }
            let xs = (
                p.iter()
                    .map(|q| q[0])
                    .fold(f32::MAX, f32::min)
                    .floor()
                    .max(cx0 as f32) as usize,
                p.iter()
                    .map(|q| q[0])
                    .fold(f32::MIN, f32::max)
                    .ceil()
                    .min(cx1 as f32) as usize,
            ); // WHAT: across. WHY: its box, clipped
            let ys = (
                p.iter()
                    .map(|q| q[1])
                    .fold(f32::MAX, f32::min)
                    .floor()
                    .max(cy0 as f32) as usize,
                p.iter()
                    .map(|q| q[1])
                    .fold(f32::MIN, f32::max)
                    .ceil()
                    .min(cy1 as f32) as usize,
            ); // WHAT: down. WHY: likewise
            for y in ys.0..ys.1 {
                for x in xs.0..xs.1 {
                    let (sx, sy) = (x as f32 + 0.5, y as f32 + 0.5); // WHAT: the pixel's middle. WHY: sampled there
                    let w0 =
                        ((p[1][0] - sx) * (p[2][1] - sy) - (p[2][0] - sx) * (p[1][1] - sy)) / area; // WHAT: corner 0's weight. WHY: barycentric
                    let w1 =
                        ((p[2][0] - sx) * (p[0][1] - sy) - (p[0][0] - sx) * (p[2][1] - sy)) / area; // WHAT: corner 1's. WHY: likewise
                    let w2 = 1.0 - w0 - w1; // WHAT: corner 2's. WHY: they add to one
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue; // WHAT: outside. WHY: not this triangle's
                    }
                    let u = v[0].uv.x * w0 + v[1].uv.x * w1 + v[2].uv.x * w2; // WHAT: the texture across. WHY: sampled
                    let t = v[0].uv.y * w0 + v[1].uv.y * w1 + v[2].uv.y * w2; // WHAT: the texture down. WHY: sampled
                    let (tx, ty) = (
                        ((u * tex.0 as f32) as usize).min(tex.0 - 1),
                        ((t * tex.1 as f32) as usize).min(tex.1 - 1),
                    ); // WHAT: the texel. WHY: nearest
                    let texel = tex.2[ty * tex.0 + tx].to_array(); // WHAT: its colour. WHY: multiplied in
                    let mut src = [0.0f32; 4]; // WHAT: this pixel's colour. WHY: blended below
                    for (k, out) in src.iter_mut().enumerate() {
                        let c = (f32::from(v[0].color.to_array()[k]) * w0
                            + f32::from(v[1].color.to_array()[k]) * w1
                            + f32::from(v[2].color.to_array()[k]) * w2)
                            / 255.0; // WHAT: the corners' colour, mixed. WHY: gradients and alpha
                        *out = c * f32::from(texel[k]) / 255.0; // WHAT: times the texture. WHY: words are white texels with alpha
                    }
                    let dst = &mut px[y * pw + x]; // WHAT: the pixel. WHY: blended into
                    for k in 0..4 {
                        dst[k] = src[k] + dst[k] * (1.0 - src[3]); // WHAT: premultiplied "over". WHY: as a renderer blends
                    }
                }
            }
        }
    }
    image::RgbaImage::from_fn(pw as u32, ph as u32, |x, y| {
        let c = px[y as usize * pw + x as usize]; // WHAT: the pixel. WHY: written out
        image::Rgba([
            (c[0] * 255.0).round().clamp(0.0, 255.0) as u8,
            (c[1] * 255.0).round().clamp(0.0, 255.0) as u8,
            (c[2] * 255.0).round().clamp(0.0, 255.0) as u8,
            255,
        ]) // WHAT: opaque sRGB. WHY: a screenshot
    })
}

#[test]
fn every_phase_one_screen_draws_and_keeps_to_52_points() {
    let shots = std::env::var_os("TABLET_SHOTS").map(std::path::PathBuf::from); // WHAT: where to write the pictures, if asked. WHY: the screenshots for the owner
    if let Some(dir) = &shots {
        std::fs::create_dir_all(dir).unwrap(); // WHAT: made. WHY: written into
    }
    let ipad = Vec2::new(1024.0, 768.0); // WHAT: an iPad, landscape. WHY: as the mockups
    let phone = Vec2::new(390.0, 844.0); // WHAT: a phone. WHY: mockup j
    let signature = vec![vec![
        [40.0, 110.0],
        [60.0, 40.0],
        [75.0, 105.0],
        [95.0, 60.0],
        [110.0, 100.0],
        [130.0, 55.0],
        [150.0, 100.0],
        [175.0, 35.0],
        [180.0, 100.0],
        [205.0, 70.0],
        [230.0, 95.0],
        [300.0, 75.0],
    ]]; // WHAT: a signature, in the pad's units. WHY: the pad and Delivered show one
    let cases: Vec<(&str, Option<Screen>, Vec2, bool)> = vec![
        ("a-job-list", None, ipad, false),
        ("b-job-home", Some(Screen::Home), ipad, false),
        ("b-job-home-bright", Some(Screen::Home), ipad, true),
        ("c-drawings", Some(Screen::Drawings), ipad, false),
        ("e-shipping-loads", Some(Screen::Shipping), ipad, false),
        ("f-ticket", Some(Screen::Ticket(2)), ipad, false),
        ("g-signature-pad", Some(Screen::Pad), ipad, false),
        ("h-delivered", Some(Screen::Done), ipad, false),
        ("i-documents", Some(Screen::Documents), ipad, false),
        (
            "j-phone-ticket-bright",
            Some(Screen::Ticket(2)),
            phone,
            true,
        ),
        ("j-phone-sign-bright", Some(Screen::Pad), phone, true),
        ("j-phone-delivered-bright", Some(Screen::Done), phone, true),
    ]; // WHAT: each screen of phase 1. WHY: drawn and checked
    for (name, screen, size, bright) in cases {
        let mut tablet = Tablet {
            jobs: Some(jobs()),
            jobs_at: "10:42 AM".into(),
            job: Some(steel()),
            screen: screen.clone(),
            ..Tablet::default()
        }; // WHAT: the tablet on that screen. WHY: from the shared example
        tablet.seen.insert("set_e101".into()); // WHAT: one sheet opened before. WHY: the rest show NEW
        tablet.pad = Some(Pad {
            load: 2,
            which: "received_by".into(),
            line: "so_1".into(),
            name: if name.contains("phone") {
                "Dana Whitfield".into()
            } else {
                String::new()
            },
            strokes: signature.clone(),
            ..Pad::default()
        }); // WHAT: a signature drawn, the name still to type on the iPad. WHY: as mockup g
        tablet.done = Some(Done {
            load: 2,
            which: "received_by".into(),
            signer: "Dana Whitfield".into(),
            signed: "2026-11-23T21:14:00Z".into(),
            strokes: signature.clone(),
        }); // WHAT: what the server kept. WHY: mockup h
        let picture = draw(&mut tablet, size, bright); // WHAT: drawn. WHY: checked and maybe written
        assert_eq!(
            (picture.width(), picture.height()),
            ((size.x * 2.0) as u32, (size.y * 2.0) as u32),
            "{name}"
        ); // WHAT: the whole window. WHY: at 2x
        let distinct: std::collections::HashSet<[u8; 4]> =
            picture.pixels().step_by(97).map(|p| p.0).collect(); // WHAT: colours used. WHY: a blank screen has one
        assert!(distinct.len() > 8, "{name} drew next to nothing"); // WHAT: something drawn. WHY: a screen that draws nothing fails
        for (rect, id) in &tablet.targets {
            assert!(
                rect.width() >= SMALLEST - 0.01 && rect.height() >= SMALLEST - 0.01,
                "{name}: {id} is {rect:?}"
            ); // WHAT: big enough for a glove. WHY: the design's rule
        }
        if let Some(dir) = &shots {
            picture.save(dir.join(format!("{name}.png"))).unwrap(); // WHAT: written. WHY: the screenshots
        }
    }
}
