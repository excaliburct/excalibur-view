//! Choosing a file: to open, to save to, or a folder to work in.
//!
//! On a desktop that is the operating system's own dialog, as it always was.
//! On a tablet there is no such dialog to borrow. iPadOS and Android each have
//! a picker, but it hands back a document the app has been lent rather than a
//! path, answers later rather than at once, and knows nothing about saving a
//! drawing set in place. So a tablet gets the program's own list instead: the
//! drawings kept in the app's own folder, which is also the folder the Files
//! app shows, with a button that brings a file in from anywhere else on the
//! device.
//!
//! Both are asked the same way. A question is put with what to do with the
//! answer, and the answer comes when there is one:
//!
//! ```ignore
//! self.filing.one(Choose::open().filter("Drawing sets (PDF)", &["pdf"]), |app, path| {
//!     app.take_file(path);
//! });
//! ```
//!
//! The question is answered after the frame that asked it, never in the middle
//! of it. A window that has taken its own state out of the program to draw
//! itself has put it back by then, so what is done with the answer finds
//! everything where it expects it.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use egui::{Color32, RichText};

use crate::app::App;

/// What to do with the files chosen.
pub type Then = Box<dyn FnOnce(&mut App, Vec<PathBuf>)>;

/// One kind of file a question will take: a name for it, and its extensions.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub name: String,
    pub extensions: Vec<String>,
}

impl Filter {
    /// Whether a file name is one of these.
    pub fn takes(&self, name: &str) -> bool {
        let Some((_, ext)) = name.rsplit_once('.') else {
            return false;
        };
        self.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Open,
    OpenMany,
    /// Save, suggesting this name.
    Save(String),
    Folder,
}

/// A question about a file, before it is asked.
#[derive(Clone, Debug, PartialEq)]
pub struct Choose {
    pub kind: Kind,
    pub title: Option<String>,
    pub filters: Vec<Filter>,
    pub start_in: Option<PathBuf>,
}

impl Choose {
    fn of(kind: Kind) -> Choose {
        Choose { kind, title: None, filters: Vec::new(), start_in: None }
    }

    pub fn open() -> Choose {
        Choose::of(Kind::Open)
    }

    pub fn open_many() -> Choose {
        Choose::of(Kind::OpenMany)
    }

    pub fn save(name: impl Into<String>) -> Choose {
        Choose::of(Kind::Save(name.into()))
    }

    pub fn folder() -> Choose {
        Choose::of(Kind::Folder)
    }

    pub fn title(mut self, title: impl Into<String>) -> Choose {
        self.title = Some(title.into());
        self
    }

    pub fn filter(mut self, name: impl Into<String>, extensions: &[&str]) -> Choose {
        let mut seen: Vec<String> = Vec::new();
        for e in extensions {
            if !seen.iter().any(|s| s.eq_ignore_ascii_case(e)) {
                seen.push(e.to_string());
            }
        }
        self.filters.push(Filter { name: name.into(), extensions: seen });
        self
    }

    pub fn start_in(mut self, folder: impl Into<PathBuf>) -> Choose {
        self.start_in = Some(folder.into());
        self
    }

    /// Whether a file of this name belongs in the list.
    pub fn shows(&self, name: &str) -> bool {
        self.filters.is_empty() || self.filters.iter().any(|f| f.takes(name))
    }

    /// What the tablet list says at the top.
    fn heading(&self) -> String {
        if let Some(title) = &self.title {
            return title.clone();
        }
        match &self.kind {
            Kind::Open => "Open".into(),
            Kind::OpenMany => "Choose files".into(),
            Kind::Save(_) => "Save".into(),
            Kind::Folder => "Choose a folder".into(),
        }
    }
}

/// A question waiting to be asked.
struct Asked {
    choose: Choose,
    then: Then,
    /// Told when nothing was chosen, too, with no files.
    always: bool,
}

/// The questions waiting to be asked, and the one being answered.
#[derive(Default)]
pub struct Choosing {
    waiting: VecDeque<Asked>,
    list: Option<List>,
}

impl Choosing {
    /// Asks, and hands every file chosen to `then`. Nothing chosen, nothing
    /// done.
    pub fn many(&mut self, choose: Choose, then: impl FnOnce(&mut App, Vec<PathBuf>) + 'static) {
        self.waiting.push_back(Asked { choose, then: Box::new(then), always: false });
    }

    /// Asks, and hands `then` whatever came of it: the files chosen, or none
    /// when the question was put away. For whatever has to be undone when
    /// nothing is chosen.
    pub fn answer(&mut self, choose: Choose, then: impl FnOnce(&mut App, Vec<PathBuf>) + 'static) {
        self.waiting.push_back(Asked { choose, then: Box::new(then), always: true });
    }

    /// Asks, and hands the one file chosen to `then`.
    pub fn one(&mut self, choose: Choose, then: impl FnOnce(&mut App, PathBuf) + 'static) {
        self.many(choose, move |app, mut paths| {
            if !paths.is_empty() {
                then(app, paths.swap_remove(0));
            }
        });
    }

    /// Whether anything is being asked or waiting to be.
    pub fn busy(&self) -> bool {
        self.list.is_some() || !self.waiting.is_empty()
    }
}

/// The kinds of file a question takes, as MIME types, for a system picker
/// that is asked that way. Empty — every file — when any of them has no
/// name this knows: better a picker that shows too much than one that hides
/// the very file somebody came for.
pub fn mime_types(filters: &[Filter]) -> Vec<&'static str> {
    let mut out = Vec::new();
    for ext in filters.iter().flat_map(|f| f.extensions.iter()) {
        let mime = match ext.to_ascii_lowercase().as_str() {
            "pdf" => "application/pdf",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "tif" | "tiff" => "image/tiff",
            "csv" => "text/csv",
            "json" => "application/json",
            _ => return Vec::new(),
        };
        if !out.contains(&mime) {
            out.push(mime);
        }
    }
    out
}

/// Whether this program uses the operating system's dialogs. Everywhere but a
/// tablet it does; `HYPERVIEW_TABLET=1` gives a desktop the tablet's list, to
/// see it and to test it.
pub fn system_dialogs() -> bool {
    cfg!(not(any(target_os = "android", target_os = "ios"))) && !crate::platform::tablet()
}

/// The folder a tablet keeps drawings in: the one the Files app shows as this
/// app's, on an iPad, and the app's own on Android. `EXV_DRAWINGS` says where,
/// when the app has told the program; a desktop pretending to be a tablet
/// uses a folder in its Documents.
pub fn drawings_folder() -> PathBuf {
    if let Some(dir) = std::env::var_os("EXV_DRAWINGS") {
        return PathBuf::from(dir);
    }
    let documents = directories::UserDirs::new()
        .and_then(|u| u.document_dir().map(Path::to_path_buf))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Documents")))
        .unwrap_or_else(std::env::temp_dir);
    documents.join("Excalibur View")
}

impl App {
    /// Asks whatever is waiting to be asked. Called once a frame, after
    /// everything else has been drawn.
    pub fn answer_file_questions(&mut self, ctx: &egui::Context) {
        if system_dialogs() {
            while let Some(asked) = self.filing.waiting.pop_front() {
                let chosen = desktop::ask(&asked.choose);
                if !chosen.is_empty() || asked.always {
                    (asked.then)(self, chosen);
                }
            }
            return;
        }
        if self.filing.list.is_none() {
            if let Some(asked) = self.filing.waiting.pop_front() {
                self.filing.list = Some(List::new(asked));
            }
        }
        let theme = self.chrome.theme;
        let Some(list) = self.filing.list.as_mut() else {
            return;
        };
        match list.show(ctx, theme) {
            Outcome::Waiting => {}
            Outcome::Cancelled => {
                let list = self.filing.list.take().expect("the list that was put away");
                if let (Some(then), true) = (list.then, list.always) {
                    then(self, Vec::new());
                }
            }
            Outcome::Chosen(paths) => {
                let list = self.filing.list.take().expect("the list that answered");
                if let Some(then) = list.then {
                    then(self, paths);
                }
            }
        }
    }
}

// ---- the desktop ------------------------------------------------------------

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod desktop {
    use super::{Choose, Kind};
    use std::path::PathBuf;

    pub fn ask(choose: &Choose) -> Vec<PathBuf> {
        let mut dialog = rfd::FileDialog::new();
        if let Some(title) = &choose.title {
            dialog = dialog.set_title(title);
        }
        for filter in &choose.filters {
            dialog = dialog.add_filter(&filter.name, &filter.extensions);
        }
        if let Some(folder) = &choose.start_in {
            dialog = dialog.set_directory(folder);
        }
        match &choose.kind {
            Kind::Open => dialog.pick_file().into_iter().collect(),
            Kind::OpenMany => dialog.pick_files().unwrap_or_default(),
            Kind::Save(name) => {
                if !name.is_empty() {
                    dialog = dialog.set_file_name(name);
                }
                dialog.save_file().into_iter().collect()
            }
            Kind::Folder => dialog.pick_folder().into_iter().collect(),
        }
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
mod desktop {
    use super::Choose;
    use std::path::PathBuf;

    /// Never reached: a tablet always uses the list.
    pub fn ask(_: &Choose) -> Vec<PathBuf> {
        Vec::new()
    }
}

// ---- the tablet's list --------------------------------------------------------

enum Outcome {
    Waiting,
    Cancelled,
    Chosen(Vec<PathBuf>),
}

/// One entry in a folder.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub folder: bool,
    pub bytes: u64,
    pub modified: Option<std::time::SystemTime>,
}

/// What is in a folder that this question can use: every folder, and the
/// files it takes, folders first and each lot in the order a person reads
/// them (2 before 10). Hidden files are left out.
pub fn listing(folder: &Path, choose: &Choose) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = read
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            let meta = e.metadata().ok()?;
            let folder = meta.is_dir();
            if !folder && (choose.kind == Kind::Folder || !choose.shows(&name)) {
                return None;
            }
            Some(Entry { name, path: e.path(), folder, bytes: meta.len(), modified: meta.modified().ok() })
        })
        .collect();
    entries.sort_by(|a, b| b.folder.cmp(&a.folder).then_with(|| natural(&a.name, &b.name)));
    entries
}

/// Orders names the way a person reads them: "S-2" before "S-10".
pub fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    fn chunks(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, run)) if *d == digit => run.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (ca, cb) = (chunks(a), chunks(b));
    for (x, y) in ca.iter().zip(cb.iter()) {
        let order = match (x.0, y.0) {
            (true, true) => {
                let (xa, ya) = (x.1.trim_start_matches('0'), y.1.trim_start_matches('0'));
                xa.len().cmp(&ya.len()).then_with(|| xa.cmp(ya))
            }
            _ => x.1.to_lowercase().cmp(&y.1.to_lowercase()),
        };
        if order != std::cmp::Ordering::Equal {
            return order;
        }
    }
    ca.len().cmp(&cb.len())
}

/// A name for a file that isn't taken in `folder`: the name itself when it is
/// free, and "name 2", "name 3" and so on when it is not.
pub fn free_name(folder: &Path, wanted: &str) -> String {
    if !folder.join(wanted).exists() {
        return wanted.to_string();
    }
    let (stem, ext) = match wanted.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (wanted.to_string(), String::new()),
    };
    (2..)
        .map(|n| format!("{stem} {n}{ext}"))
        .find(|name| !folder.join(name).exists())
        .expect("some number is free")
}

/// A name somebody typed, made safe to be a file name: no folder separators,
/// nothing the file system refuses, and the extension the question wants when
/// they left it off.
pub fn cleaned_name(typed: &str, choose: &Choose) -> Option<String> {
    let cleaned: String = typed
        .trim()
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { c })
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_string();
    if cleaned.is_empty() {
        return None;
    }
    if choose.shows(&cleaned) {
        return Some(cleaned);
    }
    match choose.filters.first().and_then(|f| f.extensions.first()) {
        Some(ext) => Some(format!("{cleaned}.{}", ext.to_lowercase())),
        None => Some(cleaned),
    }
}

struct List {
    choose: Choose,
    then: Option<Then>,
    always: bool,
    root: PathBuf,
    here: PathBuf,
    entries: Vec<Entry>,
    /// Files ticked, when more than one may be chosen.
    ticked: Vec<PathBuf>,
    name: String,
    new_folder: Option<String>,
    /// A file about to be replaced by a save, waiting for a second tap.
    replacing: Option<PathBuf>,
    /// Asked the device for files to bring in; they arrive through the
    /// platform and land in `here`.
    bringing: Option<u64>,
    said: Option<String>,
    fresh: bool,
}

impl List {
    fn new(asked: Asked) -> List {
        let Asked { choose, then, always } = asked;
        let root = drawings_folder();
        let _ = std::fs::create_dir_all(&root);
        let here = choose
            .start_in
            .clone()
            .filter(|p| p.starts_with(&root) && p.is_dir())
            .unwrap_or_else(|| root.clone());
        let name = match &choose.kind {
            Kind::Save(name) => name.clone(),
            _ => String::new(),
        };
        let mut list = List {
            choose,
            then: Some(then),
            always,
            root,
            here,
            entries: Vec::new(),
            ticked: Vec::new(),
            name,
            new_folder: None,
            replacing: None,
            bringing: None,
            said: None,
            fresh: true,
        };
        list.reread();
        list
    }

    fn reread(&mut self) {
        self.entries = listing(&self.here, &self.choose);
        self.ticked.retain(|p| p.exists());
    }

    fn go(&mut self, folder: PathBuf) {
        self.here = folder;
        self.replacing = None;
        self.new_folder = None;
        self.reread();
    }

    /// Where `here` is, as the names of the folders from the top.
    fn trail(&self) -> Vec<(String, PathBuf)> {
        let mut trail = vec![("Drawings".to_string(), self.root.clone())];
        if let Ok(rest) = self.here.strip_prefix(&self.root) {
            let mut at = self.root.clone();
            for part in rest.components() {
                at = at.join(part);
                trail.push((part.as_os_str().to_string_lossy().to_string(), at.clone()));
            }
        }
        trail
    }

    fn show(&mut self, ctx: &egui::Context, theme: ui::chrome::Theme) -> Outcome {
        // Files brought in from elsewhere on the device.
        if let Some(asked) = self.bringing {
            for arrived in crate::platform::take_imported(asked) {
                self.bringing = None;
                self.said = Some(match arrived.len() {
                    0 => "Nothing was brought in.".into(),
                    1 => "Brought in 1 file.".into(),
                    n => format!("Brought in {n} files."),
                });
                self.reread();
            }
        }

        let mut outcome = Outcome::Waiting;
        let screen = ctx.screen_rect();
        let width = (screen.width() - 32.0).clamp(280.0, 760.0);
        let height = (screen.height() - 48.0).clamp(240.0, 820.0);
        let row = 44.0;

        let modal = egui::Modal::new(egui::Id::new("choose-a-file")).show(ctx, |ui| {
            ui.set_width(width);
            ui.set_max_height(height);

            ui.horizontal(|ui| {
                ui.label(RichText::new(self.choose.heading()).size(18.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if big_button(ui, "Cancel").clicked() {
                        outcome = Outcome::Cancelled;
                    }
                });
            });
            ui.add_space(6.0);

            // Where we are, each folder a step back up.
            ui.horizontal_wrapped(|ui| {
                let trail = self.trail();
                let last = trail.len() - 1;
                for (n, (name, path)) in trail.into_iter().enumerate() {
                    if n > 0 {
                        ui.label(RichText::new("›").color(theme.faint));
                    }
                    if n == last {
                        ui.label(RichText::new(name).strong());
                    } else if ui.link(name).clicked() {
                        self.go(path);
                    }
                }
            });
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                if self.here != self.root && big_button(ui, "Up").clicked() {
                    if let Some(up) = self.here.parent().map(Path::to_path_buf) {
                        self.go(up);
                    }
                }
                if big_button(ui, "New folder").clicked() {
                    self.new_folder = Some(String::new());
                }
                if self.choose.kind != Kind::Folder
                    && !matches!(self.choose.kind, Kind::Save(_))
                    && crate::platform::can_import()
                    && big_button(ui, "Bring in from this device…").clicked()
                {
                    let asked = crate::platform::import(&self.choose.filters, true, &self.here);
                    self.bringing = Some(asked);
                    self.said = Some("Choose the files to bring in…".into());
                }
            });

            if let Some(typed) = self.new_folder.as_mut() {
                let mut make = false;
                let mut drop = false;
                ui.horizontal(|ui| {
                    let edit = ui.add(
                        egui::TextEdit::singleline(typed)
                            .hint_text("Folder name")
                            .min_size(egui::vec2(220.0, 32.0)),
                    );
                    edit.request_focus();
                    if big_button(ui, "Make it").clicked()
                        || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        make = true;
                    }
                    if big_button(ui, "Never mind").clicked() {
                        drop = true;
                    }
                });
                if make {
                    let name = typed.trim().replace(['/', '\\'], "-");
                    if !name.is_empty() {
                        let folder = self.here.join(free_name(&self.here, &name));
                        match std::fs::create_dir_all(&folder) {
                            Ok(()) => {
                                self.new_folder = None;
                                self.go(folder);
                            }
                            Err(e) => self.said = Some(format!("The folder could not be made: {e}")),
                        }
                    }
                } else if drop {
                    self.new_folder = None;
                }
            }

            if let Some(said) = &self.said {
                ui.label(RichText::new(said).color(theme.faint));
            }
            ui.separator();

            // The folder's contents.
            let list_height = (height - 200.0).max(120.0);
            let mut go_to: Option<PathBuf> = None;
            let mut chosen: Option<PathBuf> = None;
            let mut share: Option<PathBuf> = None;
            egui::ScrollArea::vertical().max_height(list_height).auto_shrink([false, false]).show(ui, |ui| {
                if self.entries.is_empty() {
                    ui.add_space(12.0);
                    let empty = if self.here == self.root {
                        if crate::platform::can_import() {
                            "Nothing here yet. Bring drawings in from this device, or copy them \
                             into Excalibur View's folder with the Files app."
                        } else {
                            "Nothing here yet."
                        }
                    } else {
                        "This folder is empty."
                    };
                    ui.label(RichText::new(empty).color(theme.faint));
                }
                for entry in &self.entries {
                    let ticked = self.ticked.contains(&entry.path);
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(ui.available_width(), row), egui::Sense::click());
                    if response.hovered() || ticked {
                        ui.painter().rect_filled(rect, 4.0, if ticked { theme.pressed } else { theme.hover });
                    }
                    let glyph = rect.left_center() + egui::vec2(18.0, 0.0);
                    if entry.folder {
                        folder_glyph(ui.painter(), glyph, theme.accent_text);
                    } else {
                        file_glyph(ui.painter(), glyph, theme.glyph);
                    }
                    ui.painter().text(
                        rect.left_center() + egui::vec2(40.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        &entry.name,
                        egui::FontId::proportional(15.0),
                        theme.text,
                    );
                    if !entry.folder {
                        ui.painter().text(
                            rect.right_center() - egui::vec2(10.0, 0.0),
                            egui::Align2::RIGHT_CENTER,
                            size_of(entry.bytes),
                            egui::FontId::proportional(12.0),
                            theme.faint,
                        );
                    }
                    if response.clicked() {
                        if entry.folder {
                            go_to = Some(entry.path.clone());
                        } else {
                            match self.choose.kind {
                                Kind::Open => chosen = Some(entry.path.clone()),
                                Kind::OpenMany => {
                                    if ticked {
                                        self.ticked.retain(|p| p != &entry.path);
                                    } else {
                                        self.ticked.push(entry.path.clone());
                                    }
                                }
                                Kind::Save(_) => {
                                    self.name = entry.name.clone();
                                    self.replacing = None;
                                }
                                Kind::Folder => {}
                            }
                        }
                    }
                    if !entry.folder && crate::platform::can_share() {
                        let path = entry.path.clone();
                        response.context_menu(|ui| {
                            if ui.button("Send a copy…").clicked() {
                                share = Some(path.clone());
                                ui.close();
                            }
                        });
                    }
                }
            });
            if let Some(folder) = go_to {
                self.go(folder);
            }
            if let Some(path) = share {
                crate::platform::share(&path);
            }
            if let Some(path) = chosen {
                outcome = Outcome::Chosen(vec![path]);
            }

            ui.separator();
            // What finishes the question.
            match self.choose.kind.clone() {
                Kind::OpenMany => {
                    ui.horizontal(|ui| {
                        let n = self.ticked.len();
                        let label = match n {
                            0 => "Tap files to choose them".to_string(),
                            1 => "Use 1 file".to_string(),
                            n => format!("Use {n} files"),
                        };
                        if ui
                            .add_enabled(
                                n > 0,
                                egui::Button::new(label).min_size(egui::vec2(160.0, 36.0)).fill(theme.accent),
                            )
                            .clicked()
                        {
                            outcome = Outcome::Chosen(self.ticked.clone());
                        }
                        let files: Vec<PathBuf> =
                            self.entries.iter().filter(|e| !e.folder).map(|e| e.path.clone()).collect();
                        if !files.is_empty() && big_button(ui, "Choose all here").clicked() {
                            self.ticked = files;
                        }
                    });
                }
                Kind::Save(_) => {
                    let mut save = false;
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut self.name).min_size(egui::vec2(280.0, 32.0)),
                        );
                        if self.fresh {
                            edit.request_focus();
                            self.fresh = false;
                        }
                        if edit.changed() {
                            self.replacing = None;
                        }
                        if ui
                            .add(egui::Button::new("Save here").min_size(egui::vec2(120.0, 36.0)).fill(theme.accent))
                            .clicked()
                            || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            save = true;
                        }
                    });
                    if let Some(replacing) = &self.replacing {
                        ui.label(
                            RichText::new(format!(
                                "There is already a {} here. Tap Save here again to replace it.",
                                replacing.file_name().unwrap_or_default().to_string_lossy()
                            ))
                            .color(theme.warn),
                        );
                    }
                    if save {
                        match cleaned_name(&self.name, &self.choose) {
                            None => self.said = Some("Type a name for the file first.".into()),
                            Some(name) => {
                                let path = self.here.join(&name);
                                if path.exists() && self.replacing.as_ref() != Some(&path) {
                                    self.replacing = Some(path);
                                } else {
                                    outcome = Outcome::Chosen(vec![path]);
                                }
                            }
                        }
                    }
                }
                Kind::Folder => {
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new("Use this folder").min_size(egui::vec2(160.0, 36.0)).fill(theme.accent))
                            .clicked()
                        {
                            outcome = Outcome::Chosen(vec![self.here.clone()]);
                        }
                    });
                }
                Kind::Open => {}
            }
        });
        if modal.should_close() && matches!(outcome, Outcome::Waiting) {
            outcome = Outcome::Cancelled;
        }
        outcome
    }
}

/// A button a finger finds: framed and filled, where the program's own
/// toolbar buttons are drawn flat for a mouse.
fn big_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let visuals = ui.visuals();
    let (fill, line) = (visuals.widgets.inactive.weak_bg_fill, visuals.widgets.noninteractive.bg_stroke);
    ui.add(
        egui::Button::new(text)
            .min_size(egui::vec2(0.0, 36.0))
            .fill(fill.gamma_multiply(1.6))
            .stroke(egui::Stroke::new(1.0_f32, line.color)), // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
    )
}

fn size_of(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{} KB", b >> 10),
        b => format!("{b} bytes"),
    }
}

fn folder_glyph(painter: &egui::Painter, at: egui::Pos2, colour: Color32) {
    let stroke = egui::Stroke::new(1.5_f32, colour); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
    let body = egui::Rect::from_center_size(at + egui::vec2(0.0, 1.5), egui::vec2(18.0, 12.0));
    painter.rect_stroke(body, 2.0, stroke, egui::StrokeKind::Middle);
    let tab = egui::Rect::from_min_size(body.left_top() - egui::vec2(0.0, 3.0), egui::vec2(7.0, 3.0));
    painter.rect_stroke(tab, 1.0, stroke, egui::StrokeKind::Middle);
}

fn file_glyph(painter: &egui::Painter, at: egui::Pos2, colour: Color32) {
    let stroke = egui::Stroke::new(1.5_f32, colour); // WHAT: the line width written as f32. WHY: a bare float there leans on the f32 fallback the compiler is retiring (float_literal_f32_fallback).
    let page = egui::Rect::from_center_size(at, egui::vec2(13.0, 17.0));
    painter.rect_stroke(page, 1.5, stroke, egui::StrokeKind::Middle);
    for n in 0..3 {
        let y = page.top() + 5.0 + n as f32 * 3.5;
        painter.line_segment([egui::pos2(page.left() + 3.0, y), egui::pos2(page.right() - 3.0, y)], stroke);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("exv-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_filter_takes_its_extensions_whatever_their_case_and_nothing_else() {
        let choose = Choose::open().filter("Drawing sets (PDF)", &["pdf", "PDF"]);
        assert_eq!(choose.filters[0].extensions, vec!["pdf".to_string()]);
        assert!(choose.shows("S-101.pdf"));
        assert!(choose.shows("S-101.PDF"));
        assert!(!choose.shows("S-101.pdf.txt"));
        assert!(!choose.shows("pdf"));
        assert!(Choose::open().shows("anything.at-all"));
    }

    #[test]
    fn a_folder_lists_its_folders_first_then_the_files_the_question_takes_in_reading_order() {
        let dir = scratch("listing");
        std::fs::create_dir_all(dir.join("Jobs")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        for name in ["S-10.pdf", "S-2.pdf", "notes.txt", ".secret.pdf"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        let choose = Choose::open().filter("PDF", &["pdf"]);
        let names: Vec<String> = listing(&dir, &choose).into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["Jobs", "S-2.pdf", "S-10.pdf"]);

        let folders: Vec<String> = listing(&dir, &Choose::folder()).into_iter().map(|e| e.name).collect();
        assert_eq!(folders, vec!["Jobs"]);
    }

    #[test]
    fn names_sort_the_way_they_read() {
        let mut names = vec!["S-10", "S-2", "s-1", "A-101", "A-11", "S-002"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(names, vec!["A-11", "A-101", "s-1", "S-2", "S-002", "S-10"]);
    }

    #[test]
    fn a_save_never_takes_a_name_already_in_the_folder_without_asking() {
        let dir = scratch("free");
        assert_eq!(free_name(&dir, "Set.pdf"), "Set.pdf");
        std::fs::write(dir.join("Set.pdf"), b"x").unwrap();
        std::fs::write(dir.join("Set 2.pdf"), b"x").unwrap();
        assert_eq!(free_name(&dir, "Set.pdf"), "Set 3.pdf");
        std::fs::create_dir_all(dir.join("Jobs")).unwrap();
        assert_eq!(free_name(&dir, "Jobs"), "Jobs 2");
    }

    #[test]
    fn a_typed_name_is_made_safe_and_given_the_extension_it_was_asked_for() {
        let pdf = Choose::save("").filter("PDF", &["pdf"]);
        assert_eq!(cleaned_name("  Takeoff summary ", &pdf).as_deref(), Some("Takeoff summary.pdf"));
        assert_eq!(cleaned_name("a/b:c.pdf", &pdf).as_deref(), Some("a-b-c.pdf"));
        assert_eq!(cleaned_name("Report.PDF", &pdf).as_deref(), Some("Report.PDF"));
        assert_eq!(cleaned_name("  ..  ", &pdf), None);
        assert_eq!(cleaned_name("notes", &Choose::save("")).as_deref(), Some("notes"));
    }

    #[test]
    fn a_picker_is_asked_for_the_kinds_it_can_name_or_for_everything() {
        let pdf = Choose::open().filter("Drawing sets (PDF)", &["pdf", "PDF"]);
        assert_eq!(mime_types(&pdf.filters), vec!["application/pdf"]);
        let pictures = Choose::open().filter("Pictures", &["png", "jpg", "jpeg"]);
        assert_eq!(mime_types(&pictures.filters), vec!["image/png", "image/jpeg"]);
        let chest = Choose::open().filter("Tool chests", &["evtools", "bpx"]);
        assert!(mime_types(&chest.filters).is_empty());
    }

    #[test]
    fn one_answer_is_handed_on_and_no_answer_does_nothing() {
        let mut choosing = Choosing::default();
        assert!(!choosing.busy());
        choosing.one(Choose::open(), |_, _| {});
        assert!(choosing.busy());
        let asked = choosing.waiting.pop_front().unwrap();
        assert!(!asked.always, "a plain question is not answered when nothing is chosen");
        choosing.answer(Choose::open(), |_, _| {});
        assert!(choosing.waiting.pop_front().unwrap().always);
    }
}
