//! A model in 3D, drawn off the screen on a thread of its own.
//!
//! The window hands over each part's triangles once, then asks for pictures:
//! from this camera, this big, looking like this. The drawing happens on the
//! view's own graphics device and comes back as pixels, so it doesn't matter
//! which renderer the window itself runs on, the window never waits on it,
//! and a picture for a brochure at twice the size of the screen is the same
//! call with bigger numbers. A click is answered with the part under it.
//!
//! Parts are numbered by the caller, from zero. Their colours are a list in
//! the same order; a colour with no opacity hides its part.

pub mod camera;
mod edges;
mod gpu;

use std::collections::BTreeSet;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub use camera::{Bounds, Camera, Preset};
pub use edges::feature_edges;

/// Parts drawn from one pair of buffers.
const PARTS_PER_CHUNK: usize = 256;
/// Parts replaced one after another are uploaded together, at most this often.
const SETTLE: Duration = Duration::from_millis(250);

/// One part's triangles, in metres, relative to `origin`.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub origin: [f64; 3],
    pub positions: Vec<f32>,
    /// One per position, or empty to have them worked out from the faces.
    pub normals: Vec<f32>,
    pub indices: Vec<u32>,
}

/// What's behind the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Background {
    /// Pale grey fading to white at the top, like a studio backdrop.
    #[default]
    Studio,
    Dark,
    White,
    /// Nothing: a picture with a transparent background.
    Clear,
}

impl Background {
    pub const ALL: [Background; 4] = [Background::Studio, Background::Dark, Background::White, Background::Clear];

    pub fn name(self) -> &'static str {
        match self {
            Background::Studio => "Studio",
            Background::Dark => "Dark",
            Background::White => "White",
            Background::Clear => "Transparent",
        }
    }

    /// Top and bottom, sRGB.
    fn colours(self) -> ([u8; 3], [u8; 3]) {
        match self {
            Background::Studio => ([0xF6, 0xF7, 0xF9], [0xC9, 0xCF, 0xD6]),
            Background::Dark => ([0x33, 0x38, 0x40], [0x14, 0x17, 0x1B]),
            Background::White | Background::Clear => ([0xFF; 3], [0xFF; 3]),
        }
    }
}

/// How the model is drawn, apart from where it's seen from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub background: Background,
    /// Lines along each part's corners.
    pub edges: bool,
    /// Everything above this height, in the view's frame, is cut away.
    pub cut_above: Option<f32>,
}

impl Default for Look {
    fn default() -> Look {
        Look { background: Background::Studio, edges: true, cut_above: None }
    }
}

/// A finished picture: RGBA, sRGB, top row first. Alpha is premultiplied,
/// which only matters for a transparent background.
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Which request this answers; see [`View::draw`].
    pub serial: u64,
}

impl Frame {
    /// The picture with its alpha taken back out of its colours, the way a
    /// PNG file keeps them.
    pub fn unpremultiplied(mut self) -> Frame {
        for pixel in self.rgba.chunks_exact_mut(4) {
            let a = pixel[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut pixel[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        self
    }
}

/// A point on the model under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Probe {
    pub part: u32,
    /// In the view's frame; add [`View::anchor`] for the model's own.
    pub point: [f32; 3],
    pub snapped: Snap,
}

/// What a probed point caught on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Snap {
    /// A corner of the part.
    Corner,
    /// Somewhere along one of the part's edges.
    Edge,
    /// Its surface, where the pointer was.
    Face,
}

/// How near, in pixels, a corner or an edge has to be to catch the pointer.
const SNAP_PIXELS: f32 = 10.0;

/// Whether the view can draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Starting,
    /// Drawing on this adapter.
    Ready(String),
    Failed(String),
}

#[derive(Clone, Copy, Debug)]
struct Request {
    camera: Camera,
    width: u32,
    height: u32,
    look: Look,
    depth: f32,
    serial: u64,
}

enum Job {
    Part(u32, Mesh, [f64; 3]),
    Colours(Vec<[u8; 4]>),
    Draw(Request),
    Pick(Request, u32, u32, mpsc::Sender<Option<u32>>),
    Probe(Request, u32, u32, mpsc::Sender<Option<Probe>>),
    Picture(Request, mpsc::Sender<Result<Frame, String>>),
}

enum Reply {
    Ready(String),
    Failed(String),
    Frame(Frame),
}

/// The window's handle on a view. Dropping it ends the drawing thread, and
/// waits for the thread to put its graphics device away.
pub struct View {
    jobs: mpsc::Sender<Job>,
    replies: mpsc::Receiver<Reply>,
    // WHAT: the drawing thread itself, and a line that goes dead once it has finished.
    // WHY: so dropping the view can wait for the graphics device to be put away, instead of leaving it to race the program's exit.
    thread: Option<(std::thread::JoinHandle<()>, mpsc::Receiver<()>)>,
    state: State,
    newest: Option<Frame>,
    serial: u64,
    anchor: Option<[f64; 3]>,
    bounds: Vec<Option<Bounds>>,
    whole: Option<Bounds>,
}

impl View {
    /// Starts the drawing thread. It finds a graphics adapter in the
    /// background; `state` says when it has, or that there isn't one.
    pub fn start() -> View {
        let (jobs, work) = mpsc::channel();
        let (answer, replies) = mpsc::channel();
        let (finished, gone) = mpsc::channel::<()>(); // WHAT: a line the thread holds until its very end. WHY: when it goes dead, everything the thread owned, the GPU device included, has been dropped.
        let started = std::thread::Builder::new().name("3D view".into()).spawn(move || {
            worker(work, answer); // WHAT: the drawing loop, until the view is dropped. WHY: it returns only after its device, queue and buffers are gone.
            drop(finished); // WHAT: lets go of the line, last of all. WHY: says to the waiting view that the device is put away.
        });
        let (state, thread) = match started {
            Ok(handle) => (State::Starting, Some((handle, gone))), // WHAT: keeps hold of the running thread. WHY: so Drop can wait for it.
            Err(e) => (State::Failed(format!("The 3D view couldn't start ({e}).")), None), // WHAT: no thread to wait for. WHY: it never started.
        };
        View { jobs, replies, thread, state, newest: None, serial: 0, anchor: None, bounds: Vec::new(), whole: None } // WHAT: the handle, with the thread kept. WHY: the thread is waited for when the handle goes.
    }

    /// Takes in whatever the drawing thread has said since last asked.
    fn hear(&mut self) {
        while let Ok(reply) = self.replies.try_recv() {
            match reply {
                Reply::Ready(adapter) => self.state = State::Ready(adapter),
                Reply::Failed(why) => self.state = State::Failed(why),
                Reply::Frame(frame) => self.newest = Some(frame),
            }
        }
    }

    pub fn state(&mut self) -> &State {
        self.hear();
        &self.state
    }

    /// The newest picture, once, when one has arrived since the last call.
    pub fn frame(&mut self) -> Option<Frame> {
        self.hear();
        self.newest.take()
    }

    /// The point in the model the view's frame is measured from. Metres,
    /// the model's own coordinates: add it to a height in the view to get the
    /// model's elevation.
    pub fn anchor(&self) -> [f64; 3] {
        self.anchor.unwrap_or_default()
    }

    /// Gives part `part` these triangles, in place of any it had.
    pub fn set_part(&mut self, part: u32, mesh: Mesh) {
        let anchor = *self.anchor.get_or_insert(mesh.origin);
        let shift = [
            (mesh.origin[0] - anchor[0]) as f32,
            (mesh.origin[1] - anchor[1]) as f32,
            (mesh.origin[2] - anchor[2]) as f32,
        ];
        let mut bounds: Option<Bounds> = None;
        for p in mesh.positions.chunks_exact(3) {
            let at = [p[0] + shift[0], p[1] + shift[1], p[2] + shift[2]];
            match &mut bounds {
                Some(b) => b.grow(at),
                None => bounds = Some(Bounds::of_point(at)),
            }
        }
        let index = part as usize;
        if self.bounds.len() <= index {
            self.bounds.resize(index + 1, None);
        }
        self.bounds[index] = bounds;
        if let Some(b) = bounds {
            self.whole = Some(self.whole.map_or(b, |w| w.union(b)));
        }
        let _ = self.jobs.send(Job::Part(part, mesh, anchor));
    }

    /// Every part's colour, by part number. Alpha 0 hides a part.
    pub fn set_colours(&self, colours: Vec<[u8; 4]>) {
        let _ = self.jobs.send(Job::Colours(colours));
    }

    /// Around every part the view has been given.
    pub fn bounds(&self) -> Option<Bounds> {
        self.whole
    }

    /// Around the parts named.
    pub fn bounds_of(&self, parts: impl IntoIterator<Item = u32>) -> Option<Bounds> {
        parts
            .into_iter()
            .filter_map(|p| self.bounds.get(p as usize).copied().flatten())
            .reduce(Bounds::union)
    }

    fn request(&mut self, camera: Camera, width: u32, height: u32, look: Look) -> Request {
        self.serial += 1;
        let depth = self.depth_for(&camera);
        Request { camera, width, height, look, depth, serial: self.serial }
    }

    /// Asks for a picture. It arrives through [`View::frame`], carrying the
    /// serial returned here; asking again before it does replaces the ask.
    pub fn draw(&mut self, camera: Camera, width: u32, height: u32, look: Look) -> u64 {
        let request = self.request(camera, width, height, look);
        let _ = self.jobs.send(Job::Draw(request));
        request.serial
    }

    /// The part at pixel (`x`, `y`) of a view this size, counted from the top
    /// left, when the answer comes.
    pub fn pick(&mut self, camera: Camera, width: u32, height: u32, look: Look, x: u32, y: u32) -> mpsc::Receiver<Option<u32>> {
        let request = self.request(camera, width, height, look);
        let (tx, rx) = mpsc::channel();
        let _ = self.jobs.send(Job::Pick(request, x, y, tx));
        rx
    }

    /// The point on the model at pixel (`x`, `y`), caught on the nearest
    /// corner or edge of the part there when one is within a few pixels.
    pub fn probe(&mut self, camera: Camera, width: u32, height: u32, look: Look, x: u32, y: u32) -> mpsc::Receiver<Option<Probe>> {
        let request = self.request(camera, width, height, look);
        let (tx, rx) = mpsc::channel();
        let _ = self.jobs.send(Job::Probe(request, x, y, tx));
        rx
    }

    /// The matrix the view draws with, for putting marks over the picture
    /// where the model is.
    pub fn view_proj(&self, camera: Camera, aspect: f32) -> camera::Mat4 {
        let depth = self.depth_for(&camera);
        camera.view_proj(aspect, depth)
    }

    fn depth_for(&self, camera: &Camera) -> f32 {
        self.whole
            .map(|b| b.radius() + camera::dot(camera::sub(b.centre(), camera.target), camera::sub(b.centre(), camera.target)).sqrt())
            .unwrap_or(10.0)
    }

    /// A picture of its own, not shown on screen: for saving.
    pub fn picture(&mut self, camera: Camera, width: u32, height: u32, look: Look) -> mpsc::Receiver<Result<Frame, String>> {
        let request = self.request(camera, width, height, look);
        let (tx, rx) = mpsc::channel();
        let _ = self.jobs.send(Job::Picture(request, tx));
        rx
    }
}

/// How long dropping a view waits for its thread to put the device away.
const FINISH: Duration = Duration::from_secs(5); // WHAT: five seconds at most. WHY: a thread stuck in a driver must not hang the window that closes it.

impl Drop for View {
    fn drop(&mut self) {
        // WHAT: swaps the job line for one nobody listens on, dropping the real one. WHY: the thread sees its line close and stops.
        drop(std::mem::replace(&mut self.jobs, mpsc::channel().0));
        if let Some((handle, gone)) = self.thread.take() { // WHAT: the thread, when there is one. WHY: a view that never started has nothing to wait for.
            // WHAT: waits until the thread's line goes dead, at most FINISH. WHY: the line only goes dead once the device is dropped.
            if let Err(mpsc::RecvTimeoutError::Disconnected) = gone.recv_timeout(FINISH) {
                let _ = handle.join(); // WHAT: collects the finished thread. WHY: nothing of it outlives the view; a panic in it has nothing left to report here, so it is set aside.
            }
            // WHAT: past FINISH the thread is left to end by itself. WHY: better than a window that never closes; this is only the old behaviour.
        }
    }
}

/// A part as the drawing thread keeps it: ready to go into a buffer.
struct Part {
    vertices: Vec<gpu::Vertex>,
    indices: Vec<u32>,
    edges: Vec<u32>,
}

struct Worker {
    gpu: gpu::Gpu,
    parts: Vec<Option<Part>>,
    chunks: Vec<gpu::Chunk>,
    dirty: BTreeSet<usize>,
    uploaded: Instant,
    last: Option<Request>,
}

fn worker(jobs: mpsc::Receiver<Job>, replies: mpsc::Sender<Reply>) {
    let gpu = match gpu::Gpu::new() {
        Ok(gpu) => gpu,
        Err(why) => {
            let _ = replies.send(Reply::Failed(why));
            return;
        }
    };
    let _ = replies.send(Reply::Ready(gpu.adapter.clone()));
    let mut w = Worker {
        gpu,
        parts: Vec::new(),
        chunks: Vec::new(),
        dirty: BTreeSet::new(),
        uploaded: Instant::now() - SETTLE,
        last: None,
    };
    loop {
        // With parts waiting to be uploaded, don't sleep past the moment
        // they're due: the last picture asked for is drawn again with them.
        let first = if w.dirty.is_empty() {
            match jobs.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        } else {
            match jobs.recv_timeout(SETTLE) {
                Ok(job) => job,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    w.upload(true);
                    if let Some(request) = w.last {
                        if !w.send(request, &replies) {
                            return;
                        }
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        };
        let mut draw = None;
        for job in std::iter::once(first).chain(std::iter::from_fn(|| jobs.try_recv().ok())) {
            match job {
                Job::Part(index, mesh, anchor) => w.set_part(index as usize, mesh, anchor),
                Job::Colours(colours) => w.gpu.set_palette(&colours),
                Job::Draw(request) => draw = Some(request),
                Job::Pick(request, x, y, answer) => {
                    w.upload(true);
                    let globals = globals(&request);
                    let _ = answer.send(w.gpu.pick(&w.chunks, &globals, request.width, request.height, x, y));
                }
                Job::Probe(request, x, y, answer) => {
                    w.upload(true);
                    let _ = answer.send(w.probe(&request, x, y));
                }
                Job::Picture(request, answer) => {
                    w.upload(true);
                    let globals = globals(&request);
                    let drawn = w.gpu.draw(
                        &w.chunks,
                        &globals,
                        request.width,
                        request.height,
                        request.look.background != Background::Clear,
                        request.look.edges,
                    );
                    // A poster-sized target isn't worth keeping.
                    w.gpu.forget_targets();
                    let _ = answer.send(drawn.map(|rgba| Frame {
                        width: request.width.min(w.gpu.largest),
                        height: request.height.min(w.gpu.largest),
                        rgba,
                        serial: request.serial,
                    }));
                }
            }
        }
        if let Some(request) = draw {
            w.upload(false);
            w.last = Some(request);
            if !w.send(request, &replies) {
                return;
            }
        }
    }
}

impl Worker {
    fn set_part(&mut self, index: usize, mesh: Mesh, anchor: [f64; 3]) {
        let shift = [
            (mesh.origin[0] - anchor[0]) as f32,
            (mesh.origin[1] - anchor[1]) as f32,
            (mesh.origin[2] - anchor[2]) as f32,
        ];
        let with_normals = mesh.normals.len() == mesh.positions.len();
        let pack = |v: f32| (v.clamp(-1.0, 1.0) * 127.0).round() as i8;
        let vertices = mesh
            .positions
            .chunks_exact(3)
            .enumerate()
            .map(|(i, p)| {
                let normal = if with_normals {
                    let n = &mesh.normals[i * 3..i * 3 + 3];
                    [pack(n[0]), pack(n[1]), pack(n[2]), 0]
                } else {
                    [0; 4]
                };
                gpu::Vertex { position: [p[0] + shift[0], p[1] + shift[1], p[2] + shift[2]], normal, part: index as u32 }
            })
            .collect();
        let edges = feature_edges(&mesh.positions, &mesh.indices);
        if self.parts.len() <= index {
            self.parts.resize_with(index + 1, || None);
        }
        self.parts[index] = Some(Part { vertices, indices: mesh.indices, edges });
        self.dirty.insert(index / PARTS_PER_CHUNK);
    }

    /// Rebuilds the buffers of chunks whose parts changed: now, when `now`,
    /// or else once parts have stopped arriving for a moment.
    fn upload(&mut self, now: bool) {
        if self.dirty.is_empty() || (!now && self.uploaded.elapsed() < SETTLE) {
            return;
        }
        let wanted = self.parts.len().div_ceil(PARTS_PER_CHUNK);
        for chunk in std::mem::take(&mut self.dirty) {
            let (mut vertices, mut indices, mut edges) = (Vec::new(), Vec::new(), Vec::new());
            let end = ((chunk + 1) * PARTS_PER_CHUNK).min(self.parts.len());
            for part in self.parts[chunk * PARTS_PER_CHUNK..end].iter().flatten() {
                let base = vertices.len() as u32;
                vertices.extend_from_slice(&part.vertices);
                indices.extend(part.indices.iter().map(|i| i + base));
                edges.extend(part.edges.iter().map(|i| i + base));
            }
            let made = self.gpu.chunk(&vertices, &indices, &edges);
            while self.chunks.len() < wanted {
                self.chunks.push(self.gpu.chunk(&[], &[], &[]));
            }
            self.chunks[chunk] = made;
        }
        self.uploaded = Instant::now();
    }

    /// The point under a pixel: the part there comes from the picking pass,
    /// and the point from the ray through the pixel meeting that part's
    /// triangles, so it's exact rather than read back from a depth buffer.
    /// It's caught on a corner or an edge of the part when one is close.
    fn probe(&mut self, request: &Request, x: u32, y: u32) -> Option<Probe> {
        let globals = globals(request);
        let part = self.gpu.pick(&self.chunks, &globals, request.width, request.height, x, y)?;
        let data = self.parts.get(part as usize)?.as_ref()?;
        let (w, h) = (request.width.max(1) as f32, request.height.max(1) as f32);
        let (nx, ny) = ((x as f32 + 0.5) / w * 2.0 - 1.0, 1.0 - (y as f32 + 0.5) / h * 2.0);
        let back = camera::inverse(&globals.view_proj)?;
        let at_depth = |z: f32| {
            let c = camera::project(&back, [nx, ny, z]);
            [c[0] / c[3], c[1] / c[3], c[2] / c[3]]
        };
        // Depth is reversed: 1 is the near plane, and a half is further in.
        let from = at_depth(1.0);
        let along = camera::normalize(camera::sub(at_depth(0.5), from));
        let at = |i: u32| data.vertices[i as usize].position;

        let mut nearest: Option<f32> = None;
        for triangle in data.indices.chunks_exact(3) {
            if let Some(t) = ray_meets_triangle(from, along, at(triangle[0]), at(triangle[1]), at(triangle[2])) {
                if nearest.is_none_or(|n| t < n) {
                    nearest = Some(t);
                }
            }
        }
        // A pixel on the very rim of a part can miss all of its triangles by
        // a hair: then the point is where the ray passes closest to the part.
        let point = match nearest {
            Some(t) => camera::add(from, camera::scale(along, t)),
            None => {
                let centre = data
                    .vertices
                    .iter()
                    .fold([0.0f32; 3], |sum, v| camera::add(sum, v.position));
                let centre = camera::scale(centre, 1.0 / data.vertices.len().max(1) as f32);
                let t = camera::dot(camera::sub(centre, from), along).max(0.0);
                camera::add(from, camera::scale(along, t))
            }
        };

        // How much of the model a pixel covers at that point.
        let camera = &request.camera;
        let reach = if camera.orthographic {
            camera.half_height()
        } else {
            let away = camera::sub(point, camera.eye());
            camera::dot(away, away).sqrt() * (camera.fov_y * 0.5).tan()
        };
        let tolerance = 2.0 * reach / h * SNAP_PIXELS;
        // Measured across the line of sight, so a corner just behind the
        // face under the pointer still catches it.
        let off_ray = |p: [f32; 3]| {
            let v = camera::sub(p, from);
            let across = camera::sub(v, camera::scale(along, camera::dot(v, along)));
            camera::dot(across, across).sqrt()
        };
        let corner = data
            .edges
            .iter()
            .map(|&i| at(i))
            .map(|p| (off_ray(p), p))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((d, p)) = corner {
            if d <= tolerance {
                return Some(Probe { part, point: p, snapped: Snap::Corner });
            }
        }
        let on_edge = data
            .edges
            .chunks_exact(2)
            .map(|pair| nearest_on_segment_to_ray(at(pair[0]), at(pair[1]), from, along))
            .map(|p| (off_ray(p), p))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((d, p)) = on_edge {
            if d <= tolerance {
                return Some(Probe { part, point: p, snapped: Snap::Edge });
            }
        }
        nearest.map(|_| Probe { part, point, snapped: Snap::Face })
    }

    /// Draws a picture for the window. False when the window has gone.
    fn send(&mut self, request: Request, replies: &mpsc::Sender<Reply>) -> bool {
        let globals = globals(&request);
        let drawn = self.gpu.draw(
            &self.chunks,
            &globals,
            request.width,
            request.height,
            request.look.background != Background::Clear,
            request.look.edges,
        );
        let reply = match drawn {
            Ok(rgba) => Reply::Frame(Frame {
                width: request.width.clamp(1, self.gpu.largest),
                height: request.height.clamp(1, self.gpu.largest),
                rgba,
                serial: request.serial,
            }),
            Err(why) => Reply::Failed(format!("The 3D view stopped drawing ({why}).")),
        };
        replies.send(reply).is_ok()
    }
}

/// Where a ray meets a triangle, as a distance along the ray (Möller and
/// Trumbore's method), either side of the triangle.
fn ray_meets_triangle(from: [f32; 3], along: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Option<f32> {
    let (ab, ac) = (camera::sub(b, a), camera::sub(c, a));
    let p = camera::cross(along, ac);
    let det = camera::dot(ab, p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = camera::sub(from, a);
    let u = camera::dot(s, p) * inv;
    if !(-1e-6..=1.0 + 1e-6).contains(&u) {
        return None;
    }
    let q = camera::cross(s, ab);
    let v = camera::dot(along, q) * inv;
    if v < -1e-6 || u + v > 1.0 + 1e-6 {
        return None;
    }
    let t = camera::dot(ac, q) * inv;
    (t > 0.0).then_some(t)
}

/// The point on segment ab that passes closest to a ray.
fn nearest_on_segment_to_ray(a: [f32; 3], b: [f32; 3], from: [f32; 3], along: [f32; 3]) -> [f32; 3] {
    let d = camera::sub(b, a);
    let r = camera::sub(a, from);
    let (dd, da, ra, rd) = (camera::dot(d, d), camera::dot(d, along), camera::dot(r, along), camera::dot(r, d));
    let denominator = dd - da * da;
    let t = if dd <= 0.0 {
        0.0
    } else if denominator.abs() < 1e-12 {
        // Parallel to the line of sight: its nearer end will do.
        0.0
    } else {
        ((da * ra - rd) / denominator).clamp(0.0, 1.0)
    };
    camera::add(a, camera::scale(d, t))
}

fn linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn rgb(c: [u8; 3]) -> [f32; 4] {
    [linear(c[0]), linear(c[1]), linear(c[2]), 1.0]
}

fn globals(request: &Request) -> gpu::Globals {
    use camera::{add, normalize, scale};
    let camera = &request.camera;
    let aspect = request.width.max(1) as f32 / request.height.max(1) as f32;
    let back = camera.backwards();
    let (right, up) = camera.right_up();
    // The key light comes from over the viewer's left shoulder, the fill
    // from low on the right, so every face of a steel shape reads apart.
    let key = normalize(add(add(scale(right, -0.45), scale(up, 0.65)), scale(back, 0.62)));
    let fill = normalize(add(add(scale(right, 0.7), scale(up, -0.15)), scale(back, 0.45)));
    let (top, bottom) = request.look.background.colours();
    let eye = camera.eye();
    let four = |v: [f32; 3]| [v[0], v[1], v[2], 0.0];
    gpu::Globals {
        view_proj: camera.view_proj(aspect, request.depth),
        eye: [eye[0], eye[1], eye[2], if camera.orthographic { 1.0 } else { 0.0 }],
        forward: four(scale(back, -1.0)),
        key: four(key),
        fill: four(fill),
        sky: [0.34, 0.36, 0.40, 0.0],
        ground: [0.14, 0.13, 0.12, 0.0],
        top: rgb(top),
        bottom: rgb(bottom),
        cut: match request.look.cut_above {
            Some(z) => [z, 1.0, 0.0, 0.0],
            None => [0.0; 4],
        },
        edge: [0.32, 0.0, 0.0, 0.0],
    }
}
