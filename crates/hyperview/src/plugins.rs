//! Plugins: an office's own tools, kept out of everybody else's copy.
//!
//! A plugin is WebAssembly, signed by the publisher, handed round by the
//! office's server the way the tool chest is. It runs here in an
//! interpreter with nothing to reach: no files, no network, no screen and
//! no drawing. It is handed the sheets and the takeoff as data and hands
//! back findings and suggested fixes, which this program carries out only
//! when somebody clicks one. See `plugin_api` for the contract.
//!
//! Three walls, each enough on its own to matter:
//!
//! - **The signature.** A plugin is loaded only when one of the keys this
//!   program was built with signed exactly those bytes. The office server
//!   checks it when an administrator adds one; every seat checks it again.
//! - **The sandbox.** A module that imports anything at all is refused —
//!   there is nothing for it to import. It gets a budget of work and of
//!   memory, and running out of either is an error, not a hang.
//! - **The fixes.** A plugin cannot change the drawing. It can only offer
//!   one of a short list of actions, carried out here, each an undo step.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hub::update::Trusted;
use plugin_api::{Input, Manifest};

pub use plugin_host::{manifest_of, run, FUEL, MEMORY};

pub const EXTENSION: &str = "hvplugin";

#[derive(Clone, Debug)]
pub struct Plugin {
    pub manifest: Manifest,
    pub wasm: Arc<Vec<u8>>,
    /// SHA-256 of the whole plugin file.
    pub digest: String,
    /// Which trusted key signed it.
    pub key: String,
    /// Came from the office's server, rather than added on this computer.
    pub from_office: bool,
    pub path: PathBuf,
}

/// The plugins this seat has.
#[derive(Clone, Debug, Default)]
pub struct Shelf {
    pub plugins: Vec<Plugin>,
    /// Files that were not loaded, and why.
    pub refused: Vec<(String, String)>,
}

/// Where plugins added on this computer are kept: beside the program, like
/// the tool chests.
pub fn folder() -> PathBuf {
    crate::server::profiles_folder()
        .parent()
        .map(|p| p.join("plugins"))
        .unwrap_or_else(|| PathBuf::from("plugins"))
}

/// Where the office's plugins are kept. Everything in here came from the
/// server and goes when the server stops listing it.
pub fn office_folder() -> PathBuf {
    folder().join("office")
}

impl Shelf {
    /// The office's plugins as the server describes them, for a copy that
    /// doesn't run plugins itself: the server runs them, so there is nothing
    /// here to load, only commands to list.
    pub fn from_office(list: &[hub::PluginInfo]) -> Shelf {
        Shelf {
            plugins: list
                .iter()
                .filter_map(|info| {
                    Some(Plugin {
                        manifest: info.manifest.clone()?,
                        wasm: Arc::new(Vec::new()),
                        digest: info.digest.to_lowercase(),
                        key: info.key.clone(),
                        from_office: true,
                        path: PathBuf::new(),
                    })
                })
                .collect(),
            refused: list
                .iter()
                .filter(|info| info.manifest.is_none())
                .map(|info| (info.name.clone(), "the server couldn't read what it does".into()))
                .collect(),
        }
    }

    /// Everything in the two folders that passes.
    pub fn load(trusted: &Trusted) -> Shelf {
        Shelf::load_from(trusted, &folder(), &office_folder())
    }

    pub fn load_from(trusted: &Trusted, own: &Path, office: &Path) -> Shelf {
        let mut shelf = Shelf::default();
        // The office's first: when both folders hold the same plugin, the
        // one the office hands out is the one everybody is running.
        for (dir, from_office) in [(office, true), (own, false)] {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            let mut paths: Vec<PathBuf> = entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case(EXTENSION)).unwrap_or(false))
                .collect();
            paths.sort();
            for path in paths {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let loaded = std::fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| open(trusted, &bytes, &path, from_office));
                match loaded {
                    Ok(plugin) => {
                        if shelf.find(&plugin.manifest.id).is_none() {
                            shelf.plugins.push(plugin);
                        }
                    }
                    Err(why) => {
                        log::warn!("plugin {name} not loaded: {why}");
                        shelf.refused.push((name, why));
                    }
                }
            }
        }
        shelf.plugins.sort_by(|a, b| a.manifest.name.to_lowercase().cmp(&b.manifest.name.to_lowercase()));
        shelf
    }

    pub fn find(&self, id: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|p| p.manifest.id == id)
    }

    /// What the Plugins menu lists.
    pub fn menu(&self) -> Vec<ui::chrome::PluginItem> {
        let mut out = Vec::new();
        for plugin in &self.plugins {
            for command in &plugin.manifest.commands {
                out.push(ui::chrome::PluginItem {
                    fire: fire_id(&plugin.manifest.id, &command.id),
                    group: plugin.manifest.name.clone(),
                    label: command.name.clone(),
                    help: command.description.clone(),
                });
            }
        }
        out
    }

    /// Digests of the office plugins this seat already holds.
    pub fn office_digests(&self) -> HashSet<String> {
        self.plugins
            .iter()
            .filter(|p| p.from_office)
            .map(|p| p.digest.to_lowercase())
            .collect()
    }
}

/// The command id a menu item fires.
pub fn fire_id(plugin: &str, command: &str) -> String {
    format!("Plugin:{plugin}:{command}")
}

/// The other way round.
pub fn from_fire_id(id: &str) -> Option<(String, String)> {
    let rest = id.strip_prefix("Plugin:")?;
    let (plugin, command) = rest.split_once(':')?;
    Some((plugin.to_string(), command.to_string()))
}

/// Checks a plugin file and reads what it says about itself.
pub fn open(trusted: &Trusted, file: &[u8], path: &Path, from_office: bool) -> Result<Plugin, String> {
    let checked = hub::plugin::check(trusted, file)?;
    let manifest = manifest_of(&checked.wasm)?;
    if manifest.id != checked.header.id || manifest.version != checked.header.version {
        return Err(format!(
            "The plugin says it is {} {} but was signed as {} {}.",
            manifest.id, manifest.version, checked.header.id, checked.header.version
        ));
    }
    if manifest.abi > plugin_api::ABI {
        return Err(format!(
            "{} needs a newer Excalibur View than this one. It will load once this copy updates.",
            manifest.name
        ));
    }
    if manifest.commands.is_empty() {
        return Err(format!("{} has nothing in it to run.", manifest.name));
    }
    Ok(Plugin {
        manifest,
        wasm: Arc::new(checked.wasm),
        digest: checked.digest,
        key: checked.header.key,
        from_office,
        path: path.to_path_buf(),
    })
}

/// Checks a plugin somebody picked, and puts it in this computer's folder.
pub fn add_from_file(trusted: &Trusted, source: &Path) -> Result<(Plugin, Vec<u8>), String> {
    let bytes = std::fs::read(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let checked = open(trusted, &bytes, source, false)?;
    let dir = folder();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not make the plugins folder: {e}"))?;
    let path = dir.join(format!("{}.{EXTENSION}", checked.manifest.id));
    std::fs::write(&path, &bytes).map_err(|e| format!("Could not keep the plugin: {e}"))?;
    Ok((Plugin { path, ..checked }, bytes))
}

// ---- checking a plugin before it is signed ------------------------------------

/// What `Hyperview.exe --plugin-check` found.
pub struct Checkup {
    pub passed: bool,
    pub report: String,
    /// Every command's answer, as JSON, beside the plugin.
    pub written: Option<PathBuf>,
}

/// `Hyperview.exe --plugin-check plugin.wasm [sheet.json]`, for somebody
/// writing a plugin.
///
/// Reads the manifest and runs every command in the same sandbox, with the
/// same limits, that a signed plugin gets: against a sheet saved with
/// Plugins ▸ Save This Sheet for a Plugin Test…, or against nothing at all.
/// Nothing is installed or added to any menu, and no signature is asked for,
/// because nothing here is trusted: it is run once, reported on and thrown
/// away. Each command's answer goes in `<plugin>-check.json` beside it.
pub fn checkup(wasm_path: &Path, input_path: Option<&Path>) -> Checkup {
    let fail = |report: String| Checkup { passed: false, report, written: None };
    let wasm = match std::fs::read(wasm_path) {
        Ok(w) => w,
        Err(e) => return fail(format!("{}: {e}", wasm_path.display())),
    };
    if wasm.starts_with(plugin_api::MAGIC.as_bytes()) {
        return fail(
            "That is a signed .hvplugin. Check the .wasm it was made from, or add this one \
             with Plugins > Add Plugin…"
                .into(),
        );
    }
    let manifest = match manifest_of(&wasm) {
        Ok(m) => m,
        Err(e) => return fail(format!("Not ready: {e}")),
    };
    let mut problems = Vec::new();
    let id_ok = !manifest.id.is_empty()
        && manifest.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !id_ok {
        problems.push(format!("The id '{}' can only have letters, digits, - and _.", manifest.id));
    }
    if manifest.version.trim().is_empty() {
        problems.push("It has no version.".into());
    }
    if manifest.abi > plugin_api::ABI {
        problems.push(format!(
            "It is built for contract {}, and this copy knows {}.",
            manifest.abi,
            plugin_api::ABI
        ));
    }
    if manifest.commands.is_empty() {
        problems.push("It has no commands, so there would be nothing in the menu.".into());
    }
    let mut seen = HashSet::new();
    for command in &manifest.commands {
        if !seen.insert(command.id.as_str()) {
            problems.push(format!("Two commands are called '{}'.", command.id));
        }
    }
    let mut seen = HashSet::new();
    for setting in &manifest.settings {
        if !seen.insert(setting.id.as_str()) {
            problems.push(format!("Two settings are called '{}'.", setting.id));
        }
    }

    let base = match input_path {
        None => Input::default(),
        Some(path) => match std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| plugin_api::unpack(&b)) {
            Ok(input) => input,
            Err(e) => return fail(format!("{}: {e}", path.display())),
        },
    };
    let on = match input_path {
        Some(path) => format!("on {}", path.file_name().unwrap_or_default().to_string_lossy()),
        None => "on an empty drawing".into(),
    };
    let mut lines = vec![format!(
        "{} {} ({}), {} command{}, {} KB",
        manifest.name,
        manifest.version,
        manifest.id,
        manifest.commands.len(),
        if manifest.commands.len() == 1 { "" } else { "s" },
        wasm.len() / 1024
    )];
    let mut answers = Vec::new();
    for command in &manifest.commands {
        let mut input = base.clone();
        input.command = command.id.clone();
        input.context = only_what_was_asked(&command.needs, input.context); // WHAT: the test sheet's context, cut to what the command asked for. WHY: the check runs it as View would, so a plugin that forgot to ask finds out here, not on a seat.
        if command.needs.context { // WHAT: the command asks to be told who and which project. WHY: whoever signs it should see that it does.
            lines.push(format!("  {}: asks to be told who runs it, and the office project and drawing set.", command.id)); // WHAT: said in the report. WHY: the same.
        }
        let mut settings = settings_for(&manifest, &Settings::new());
        settings.extend(base.settings.clone());
        input.settings = settings;
        let started = std::time::Instant::now();
        let result = run(&wasm, &input);
        let took = started.elapsed();
        match &result {
            Ok(output) => {
                let findings = output.findings.len();
                let serious = output.findings.iter().filter(|f| f.level == plugin_api::Level::Problem).count();
                lines.push(format!(
                    "  {}: ran {on} in {:.1}s. {findings} finding{}, {serious} marked a problem, {} table{}.",
                    command.id,
                    took.as_secs_f64(),
                    if findings == 1 { "" } else { "s" },
                    output.tables.len(),
                    if output.tables.len() == 1 { "" } else { "s" },
                ));
                for (file, verdict) in output.offered_files() { // WHAT: each file the command offered. WHY: one the program would refuse to save is a mistake the plugin's writer should hear about before signing.
                    match verdict {
                        Ok(()) => lines.push(format!("    offers {} ({} bytes)", file.name, file.text.len())), // WHAT: said in the report. WHY: the writer sees the file would be offered.
                        Err(why) => problems.push(format!("{}: the file \"{}\" would not be saved: {why}.", command.id, file.name.escape_debug())), // WHAT: a problem, with the name escaped. WHY: not ready to sign while a file it offers would be refused.
                    }
                }
                for finding in &output.findings {
                    for fix in &finding.fixes {
                        if fix.actions.is_empty() {
                            problems.push(format!("{}: the fix '{}' does nothing.", command.id, fix.label));
                        }
                    }
                }
            }
            Err(why) => {
                lines.push(format!("  {}: did not finish {on}. {why}", command.id));
                problems.push(format!("{} did not finish: {why}", command.id));
            }
        }
        answers.push(serde_json::json!({
            "command": command.id,
            "seconds": took.as_secs_f64(),
            "output": result.as_ref().ok(),
            "error": result.as_ref().err(),
        }));
    }
    let written = {
        let stem = wasm_path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let path = wasm_path.with_file_name(format!("{stem}-check.json"));
        let body = serde_json::json!({ "manifest": manifest, "runs": answers });
        serde_json::to_vec_pretty(&body)
            .ok()
            .and_then(|bytes| std::fs::write(&path, bytes).ok())
            .map(|_| path)
    };
    let passed = problems.is_empty();
    if passed {
        lines.insert(0, "Ready to be signed.".into());
    } else {
        lines.insert(0, "Not ready yet:".into());
        for (i, problem) in problems.iter().enumerate() {
            lines.insert(1 + i, format!("  - {problem}"));
        }
        lines.insert(1 + problems.len(), String::new());
    }
    if let Some(path) = &written {
        lines.push(format!("\nEvery answer is in {}", path.display()));
    }
    Checkup { passed, report: lines.join("\n"), written }
}

// ---- settings ------------------------------------------------------------------

/// What each plugin's settings were last set to on this computer.
pub type Settings = BTreeMap<String, BTreeMap<String, serde_json::Value>>;

fn settings_file() -> PathBuf {
    folder().join("settings.json")
}

pub fn load_settings() -> Settings {
    std::fs::read(settings_file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_settings(settings: &Settings) {
    let _ = std::fs::create_dir_all(folder());
    if let Ok(bytes) = serde_json::to_vec_pretty(settings) {
        let _ = std::fs::write(settings_file(), bytes);
    }
}

/// A plugin's settings with its defaults filled in.
pub fn settings_for(manifest: &Manifest, saved: &Settings) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    let mine = saved.get(&manifest.id);
    for setting in &manifest.settings {
        let value = mine
            .and_then(|m| m.get(&setting.id))
            .cloned()
            .unwrap_or_else(|| setting.default_value());
        out.insert(setting.id.clone(), value);
    }
    out
}

// ---- turning the takeoff into what a plugin reads ------------------------------

/// Inches in one of a scale's units.
fn inches_in(unit: &str) -> f64 {
    match unit.trim().trim_end_matches('.').to_lowercase().as_str() {
        "'" | "ft" | "feet" | "foot" => 12.0,
        "\"" | "in" | "inch" | "inches" => 1.0,
        "yd" | "yard" | "yards" => 36.0,
        "mm" => 1.0 / 25.4,
        "cm" => 1.0 / 2.54,
        "m" => 1.0 / 0.0254,
        _ => 12.0,
    }
}

/// A sheet's scale as a plugin reads it.
pub fn scale_of(measure: &annot::measure::Measure) -> plugin_api::Scale {
    let unit = measure.x.first().map(|f| f.unit.clone()).unwrap_or_default();
    plugin_api::Scale {
        ratio: measure.per_point() * inches_in(&unit) * 72.0,
        text: measure.ratio.clone(),
    }
}

/// Feet in one of a scale's units.
pub fn feet_per_unit(measure: &annot::measure::Measure) -> f64 {
    let unit = measure.x.first().map(|f| f.unit.clone()).unwrap_or_default();
    inches_in(&unit) / 12.0
}

/// How a markup is named to a plugin: its own name when it has one, its
/// place in the list when it does not.
pub fn markup_id(index: usize, markup: &annot::Markup) -> String {
    let name = markup.name();
    if name.is_empty() {
        format!("#{index}")
    } else {
        name
    }
}

/// What the save box for a file a plugin offers starts with: its name and
/// its extension. Refused, with the reason, when the file fails
/// [`plugin_api::OutFile::check`], so nothing that fails it ever gets as far
/// as a save box.
///
/// # Errors
///
/// Why the file can't be saved.
pub fn save_plan(file: &plugin_api::OutFile) -> Result<(String, String), String> {
    file.check()?; // WHAT: the checks, again, here. WHY: the window already checked, but the disk is only reached through this.
    Ok((file.name.clone(), file.extension())) // WHAT: the name and the type. WHY: what the save box shows and filters by.
}

/// A PDF date, `D:20261103140500Z`, as RFC 3339 UTC, `2026-11-03T14:05:00Z`.
/// `None` for anything that isn't one to the second in UTC.
pub fn iso_from_pdf_date(pdf: &str) -> Option<String> {
    let digits = pdf.strip_prefix("D:")?.strip_suffix('Z')?; // WHAT: the fourteen digits between "D:" and "Z". WHY: the form annot::place::now writes, UTC to the second.
    if digits.len() != 14 || !digits.bytes().all(|b| b.is_ascii_digit()) { // WHAT: exactly fourteen digits. WHY: anything else isn't a date this can read, and slicing it would be wrong.
        return None; // WHAT: not one. WHY: an empty time beats a wrong one.
    }
    let part = |from: usize, to: usize| digits.get(from..to).unwrap_or_default(); // WHAT: a run of the digits, never panicking. WHY: the length was checked; the fallback is never taken.
    Some(format!("{}-{}-{}T{}:{}:{}Z", part(0, 4), part(4, 6), part(6, 8), part(8, 10), part(10, 12), part(12, 14))) // WHAT: year-month-day, T, hours:minutes:seconds, Z. WHY: RFC 3339, which every program reads.
}

/// What a plugin is told about where and when it is being run. `set` is the
/// office server's drawing set the drawing came from, with its project, when
/// it came from one; otherwise the drawing set is the file's own name. Who is
/// running it, the project and the set are told only to a command whose
/// manifest asks for them (`needs.context`); every command is told the
/// program, its version and the time.
pub fn context_for(
    needs: &plugin_api::Needs, // WHAT: what the command asked for. WHY: decides whether who and which project are told.
    user: &str,
    set: Option<&hub::DrawingSet>,
    projects: &[hub::Project],
    file_stem: &str,
    pdf_now: &str,
) -> plugin_api::Context {
    let project = set
        .and_then(|s| projects.iter().find(|p| p.id == s.project)) // WHAT: the project the set belongs to. WHY: a set names its project by id only.
        .map(|p| p.name.clone()) // WHAT: the project's name. WHY: what people call it.
        .unwrap_or_default(); // WHAT: empty when it has none. WHY: a drawing from a folder belongs to no project.
    let full = plugin_api::Context { // WHAT: everything this program knows. WHY: cut below to what was asked.
        app: "Excalibur View".into(), // WHAT: this program. WHY: a plugin's file says what made it.
        app_version: env!("CARGO_PKG_VERSION").into(), // WHAT: this build's version. WHY: the same, to the version.
        user: user.trim().into(), // WHAT: the person, as their markups name them. WHY: who ran it.
        project,
        drawing_set: set.map(|s| s.name.clone()).unwrap_or_else(|| file_stem.into()), // WHAT: the set's name, or the file's. WHY: always something a person recognises.
        now: iso_from_pdf_date(pdf_now).unwrap_or_default(), // WHAT: the time, in RFC 3339. WHY: a plugin has no clock of its own.
    };
    only_what_was_asked(needs, full) // WHAT: cut to what the command asked for. WHY: who and which project go only to a plugin whose signed manifest asks.
}

/// `context` with who is running it, the project and the drawing set
/// emptied unless the command asked for them. The program, its version and
/// the time say nothing about the office, so every command keeps those.
#[must_use] // WHAT: a caller that drops the context gets a warning. WHY: cutting it has no other effect.
pub fn only_what_was_asked(needs: &plugin_api::Needs, context: plugin_api::Context) -> plugin_api::Context {
    if needs.context { // WHAT: the command's signed manifest asks to be told. WHY: then it is told everything.
        return context; // WHAT: all of it. WHY: it asked.
    }
    plugin_api::Context { user: String::new(), project: String::new(), drawing_set: String::new(), ..context } // WHAT: the three emptied, the rest kept. WHY: a plugin that didn't ask learns nothing about who or which job.
}

pub fn kind_word(kind: annot::Kind) -> &'static str {
    use annot::Kind;
    match kind {
        Kind::Length => "length",
        Kind::Polylength => "polylength",
        Kind::Area => "area",
        Kind::Volume => "volume",
        Kind::Count => "count",
        Kind::Diameter => "diameter",
        Kind::Radius => "radius",
        Kind::Angle => "angle",
        _ => "markup",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plugin written by hand in WebAssembly's text form: it says what it
    /// is, and answers every run with the same thing.
    fn tiny(run_body: &str) -> Vec<u8> {
        tiny_answering(run_body, r#"{"done":{"title":"Hello","findings":[{"level":"check","message":"Look here","page":0,"area":[1,2,3,4]}]}}"#) // WHAT: the usual answer. WHY: most tests only need a plugin that answers something.
    }

    /// The tiny plugin, answering every run with `answer`.
    fn tiny_answering(run_body: &str, answer: &str) -> Vec<u8> {
        let manifest = r#"{"id":"tiny","name":"Tiny","version":"1.0.0","commands":[{"id":"hello","name":"Hello"}]}"#;
        let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let text = format!(
            r#"(module
                (memory (export "memory") 1)
                (global $next (mut i32) (i32.const 8192))
                (data (i32.const 0) "{m}")
                (data (i32.const 2048) "{a}")
                (func (export "hv_alloc") (param i32) (result i32) (local i32)
                    global.get $next
                    local.set 1
                    global.get $next
                    local.get 0
                    i32.add
                    global.set $next
                    local.get 1)
                (func (export "hv_manifest") (result i64)
                    i64.const {ml})
                (func (export "hv_run") (param i32 i32) (result i64)
                    {run_body}
                    i64.const {packed}))"#,
            m = escape(manifest),
            a = escape(answer),
            ml = manifest.len(),
            packed = (2048i64 << 32) | answer.len() as i64,
        );
        wat::parse_str(text).unwrap()
    }

    #[test]
    fn a_copy_that_doesnt_run_plugins_lists_what_the_server_says_they_do() {
        let manifest = manifest_of(&tiny("")).unwrap();
        let info = |id: &str, manifest: Option<Manifest>| hub::PluginInfo {
            id: id.into(),
            name: id.into(),
            version: "1.0.0".into(),
            digest: "AB".into(),
            bytes: 1,
            uploaded: String::new(),
            uploaded_by: String::new(),
            key: "test-2026".into(),
            manifest,
        };
        let shelf = Shelf::from_office(&[info("tiny", Some(manifest)), info("broken", None)]);
        assert_eq!(shelf.plugins.len(), 1);
        assert!(shelf.plugins[0].wasm.is_empty(), "nothing to run here");
        assert!(shelf.plugins[0].from_office);
        assert_eq!(shelf.menu()[0].fire, "Plugin:tiny:hello");
        assert_eq!(shelf.refused.len(), 1, "one the server couldn't read is named, not listed");
    }

    #[test]
    fn a_plugin_checked_before_signing_is_run_and_reported_on() {
        let dir = std::env::temp_dir().join(format!("hv-checkup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wasm = dir.join("tiny.wasm");
        std::fs::write(&wasm, tiny("")).unwrap();
        let sheet = dir.join("S-201.json");
        let input = Input {
            sheets: vec![plugin_api::Sheet { page: 0, name: "S-201".into(), ..Default::default() }],
            ..Input::default()
        };
        std::fs::write(&sheet, serde_json::to_vec(&input).unwrap()).unwrap();

        let checkup = checkup(&wasm, Some(&sheet));
        assert!(checkup.passed, "{}", checkup.report);
        assert!(checkup.report.starts_with("Ready to be signed."), "{}", checkup.report);
        assert!(checkup.report.contains("hello: ran on S-201.json"), "{}", checkup.report);
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(checkup.written.unwrap()).unwrap()).unwrap();
        assert_eq!(written["runs"][0]["output"]["findings"][0]["message"], "Look here");

        // A signed file is not what this checks, and it says so.
        std::fs::write(&wasm, b"HVPLUGIN1\n{}\n").unwrap();
        assert!(!checkup_of(&wasm).passed);
        // Something that isn't a plugin at all is not ready, with a reason.
        std::fs::write(&wasm, b"not webassembly").unwrap();
        let not = checkup_of(&wasm);
        assert!(!not.passed && not.report.starts_with("Not ready"), "{}", not.report);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_a_plugin_offers_comes_out_of_the_sandbox_and_a_bad_one_stops_it_being_ready() {
        let good = r#"{"done":{"title":"T","files":[{"name":"NorthYard.estimate","text":"{\"format\":1}"}]}}"#; // WHAT: an answer offering one data file. WHY: the new way out of the sandbox.
        let output = run(&tiny_answering("", good), &Input::default()).unwrap(); // WHAT: run in the real sandbox. WHY: the file must survive the trip as the plugin wrote it.
        assert_eq!(output.files, vec![plugin_api::OutFile { name: "NorthYard.estimate".into(), text: "{\"format\":1}".into() }]); // WHAT: the same name and text. WHY: nothing added or lost.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/hv-plugin-files").join(std::process::id().to_string()); // WHAT: a folder of this build's own for the checkup's files. WHY: it writes its answers beside the plugin, and a shared temporary folder is anybody's.
        std::fs::create_dir_all(&dir).unwrap();
        let wasm = dir.join("tiny.wasm"); // WHAT: where the plugin goes. WHY: checkup reads a file.
        std::fs::write(&wasm, tiny_answering("", good)).unwrap();
        let ready = checkup(&wasm, None); // WHAT: checked before signing. WHY: a good file is no reason not to sign.
        assert!(ready.passed && ready.report.contains("offers NorthYard.estimate"), "{}", ready.report); // WHAT: ready, and the file named. WHY: the writer sees what it offers.
        let bad = r#"{"done":{"title":"T","files":[{"name":"update.exe","text":"MZ"}]}}"#; // WHAT: an answer offering a program. WHY: what must never reach the disk.
        std::fs::write(&wasm, tiny_answering("", bad)).unwrap();
        let refused = checkup(&wasm, None); // WHAT: checked again. WHY: the bad file.
        assert!(!refused.passed && refused.report.contains("update.exe") && refused.report.contains("may not offer"), "{}", refused.report); // WHAT: not ready, saying which file and why. WHY: it is caught before anybody signs it.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_file_that_passes_the_checks_gets_a_save_box() {
        let ok = plugin_api::OutFile { name: "NorthYard.Estimate".into(), text: "{}".into() }; // WHAT: a data file. WHY: the ordinary case.
        assert_eq!(save_plan(&ok), Ok(("NorthYard.Estimate".into(), "estimate".into()))); // WHAT: its name, and its type in lower case. WHY: what the save box starts with.
        let bad = plugin_api::OutFile { name: "..\\Startup\\run.bat".into(), text: "x".into() }; // WHAT: a program into another folder. WHY: the worst a plugin could try.
        assert!(save_plan(&bad).is_err()); // WHAT: refused. WHY: no save box is ever opened for it.
    }

    fn checkup_of(wasm: &Path) -> Checkup {
        checkup(wasm, None)
    }

    #[test]
    fn a_plugin_says_what_it_is_and_answers() {
        let wasm = tiny("");
        let manifest = manifest_of(&wasm).unwrap();
        assert_eq!(manifest.id, "tiny");
        assert_eq!(manifest.commands[0].name, "Hello");
        let input = Input {
            command: "hello".into(),
            ..Input::default()
        };
        let output = run(&wasm, &input).unwrap();
        assert_eq!(output.title, "Hello");
        assert_eq!(output.findings[0].area, Some([1.0, 2.0, 3.0, 4.0]));
    }

    #[test]
    fn a_plugin_that_never_finishes_is_stopped() {
        let wasm = tiny("(loop $forever (br $forever))");
        let started = std::time::Instant::now();
        // Smaller budget than the real one, so the test is quick; the same
        // wall stops both.
        let e = plugin_host::run_with_fuel(&wasm, &Input::default(), 50_000_000).unwrap_err();
        assert!(e.contains("too long"), "{e}");
        assert!(started.elapsed().as_secs() < 30);
    }

    #[test]
    fn a_plugin_that_grabs_memory_is_stopped() {
        // Twenty-six thousand pages of 64 KiB is well over the allowance.
        let wasm = tiny("(drop (memory.grow (i32.const 26000))) (if (i32.lt_s (memory.size) (i32.const 2)) (then unreachable))");
        let e = run(&wasm, &Input::default()).unwrap_err();
        assert!(e.contains("stopped"), "{e}");
    }

    #[test]
    fn a_plugin_that_reaches_outside_is_not_run() {
        let wasm = wat::parse_str(
            r#"(module (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))
                       (memory (export "memory") 1))"#,
        )
        .unwrap();
        let e = manifest_of(&wasm).unwrap_err();
        assert!(e.contains("does not give plugins"), "{e}");
        assert!(e.contains("fd_write"));
    }

    #[test]
    fn a_plugin_that_answers_from_nowhere_is_caught() {
        let wasm = wat::parse_str(
            r#"(module (memory (export "memory") 1)
                 (func (export "hv_manifest") (result i64) i64.const 0x7fff000000000010))"#,
        )
        .unwrap();
        assert!(manifest_of(&wasm).unwrap_err().contains("outside"));
    }

    #[test]
    fn a_signed_plugin_on_the_shelf_is_loaded_and_a_forged_one_is_listed_as_refused() {
        use ed25519_dalek::{Signer, SigningKey};
        use sha2::{Digest, Sha256};
        let dir = std::env::temp_dir().join(format!("hv-plugins-{}", std::process::id()));
        let (own, office) = (dir.join("own"), dir.join("office"));
        std::fs::create_dir_all(&own).unwrap();
        std::fs::create_dir_all(&office).unwrap();
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let trusted = Trusted {
            keys: vec![("test".into(), signing.verifying_key().to_bytes())],
        };
        let seal = |wasm: &[u8], key: &SigningKey| {
            let sha = hub::update::hex(&Sha256::digest(wasm));
            let signature = key.sign(&plugin_api::signing_payload("tiny", "1.0.0", &sha));
            plugin_api::seal(
                &plugin_api::Header {
                    id: "tiny".into(),
                    version: "1.0.0".into(),
                    name: "Tiny".into(),
                    sha256: sha,
                    bytes: wasm.len() as u64,
                    key: "test".into(),
                    signature: hub::update::hex(&signature.to_bytes()),
                },
                wasm,
            )
        };
        let wasm = tiny("");
        std::fs::write(office.join("tiny.hvplugin"), seal(&wasm, &signing)).unwrap();
        std::fs::write(own.join("forged.hvplugin"), seal(&wasm, &SigningKey::from_bytes(&[8u8; 32]))).unwrap();
        let shelf = Shelf::load_from(&trusted, &own, &office);
        assert_eq!(shelf.plugins.len(), 1);
        assert!(shelf.plugins[0].from_office);
        assert_eq!(shelf.refused.len(), 1);
        assert_eq!(shelf.refused[0].0, "forged.hvplugin");
        let menu = shelf.menu();
        assert_eq!(menu[0].fire, "Plugin:tiny:hello");
        assert_eq!(from_fire_id(&menu[0].fire), Some(("tiny".into(), "hello".into())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real plugin through the sandbox: `HV_TEST_PLUGIN=path/to.wasm
    /// cargo test -p hyperview --lib real_plugin -- --ignored`.
    #[test]
    #[ignore]
    fn a_real_plugin_runs_in_the_sandbox() {
        let path = std::env::var("HV_TEST_PLUGIN").expect("HV_TEST_PLUGIN");
        let wasm = std::fs::read(path).unwrap();
        let started = std::time::Instant::now();
        let manifest = manifest_of(&wasm).unwrap();
        println!("{} {} — {} commands, read in {:?}", manifest.name, manifest.version, manifest.commands.len(), started.elapsed());
        for command in &manifest.commands {
            let mut input = Input {
                command: command.id.clone(),
                sheets: vec![plugin_api::Sheet {
                    page: 0,
                    name: "S-201".into(),
                    width: 2592.0,
                    height: 1728.0,
                    scale: Some(plugin_api::Scale { ratio: 96.0, text: "1/8\" = 1'-0\"".into() }),
                    words: (0..3000)
                        .map(|i| plugin_api::Word {
                            text: if i % 7 == 0 { "W12X26".into() } else if i == 5 { "SCALE: 1/4\" = 1'-0\"".into() } else { format!("NOTE {i}") },
                            area: [(i % 60) as f64 * 40.0, (i / 60) as f64 * 30.0, (i % 60) as f64 * 40.0 + 30.0, (i / 60) as f64 * 30.0 + 8.0],
                            size: 8.0,
                        })
                        .collect(),
                    lines: (0..40_000)
                        .map(|i| plugin_api::Line {
                            a: [(i % 200) as f64 * 12.0, (i / 200) as f64 * 8.0],
                            b: [(i % 200) as f64 * 12.0 + 10.0, (i / 200) as f64 * 8.0],
                            width: if i % 50 == 0 { 1.4 } else { 0.25 },
                            dashed: false,
                        })
                        .collect(),
                    ..Default::default()
                }],
                markups: (0..500)
                    .map(|i| plugin_api::Markup {
                        id: format!("#{i}"),
                        kind: "length".into(),
                        subject: "W12X26".into(),
                        points: vec![[i as f64 * 4.0, 100.0], [i as f64 * 4.0 + 50.0, 100.0]],
                        length: Some(10.0),
                        quantity: 1.0,
                        columns: vec!["26".into()],
                        pounds: Some(260.0),
                        ..Default::default()
                    })
                    .collect(),
                columns: vec!["LBS Per FT".into()],
                ..Input::default()
            };
            input.settings = settings_for(&manifest, &Settings::new());
            let started = std::time::Instant::now();
            let output = run(&wasm, &input).unwrap();
            println!("{:12} {:>5} findings, {:>2} tables in {:?} — {}", command.id, output.findings.len(), output.tables.len(), started.elapsed(), output.summary);
        }
    }

    #[test]
    fn a_pdf_date_becomes_rfc_3339_and_anything_else_is_nothing() {
        assert_eq!(iso_from_pdf_date("D:20261103140500Z").as_deref(), Some("2026-11-03T14:05:00Z")); // WHAT: the form annot writes. WHY: what a plugin is told.
        for odd in ["", "D:2026110314Z", "20261103140500Z", "D:20261103140500", "D:2026110314050aZ", "D:20261103140500+01'00'"] { // WHAT: short, unprefixed, unsuffixed, non-digit, and with an offset. WHY: none is UTC to the second.
            assert_eq!(iso_from_pdf_date(odd), None, "{odd}"); // WHAT: nothing. WHY: an empty time beats a wrong one.
        }
        assert!(iso_from_pdf_date(&annot::place::now()).is_some()); // WHAT: the real clock reads. WHY: the two ends agree on the form.
    }

    #[test]
    fn a_plugin_is_told_the_project_and_set_a_drawing_came_from() {
        let project: hub::Project = serde_json::from_value(serde_json::json!({"id":"p1","number":"2630","name":"North Yard Storm","created":"","sets":1})).unwrap(); // WHAT: a project. WHY: the set below belongs to it.
        let set: hub::DrawingSet = serde_json::from_value(serde_json::json!({"id":"s1","project":"p1","name":"C-Sheets Rev 2","filename":"c.pdf","digest":"","bytes":0,"sheets":2,"uploaded":"","uploaded_by":"","revision":0})).unwrap(); // WHAT: a drawing set in that project. WHY: where the drawing came from.
        let other: hub::Project = serde_json::from_value(serde_json::json!({"id":"p0","number":"2601","name":"Another job","created":"","sets":1})).unwrap(); // WHAT: a project listed first that the set isn't in. WHY: the set's own project must be found, not the first.
        let asks = plugin_api::Needs { context: true, ..Default::default() }; // WHAT: a command whose manifest asks to be told. WHY: the full case.
        let from_server = context_for(&asks, " Sam ", Some(&set), &[other, project], "c", "D:20261103140500Z"); // WHAT: a drawing opened from the office server. WHY: the full case.
        assert_eq!((from_server.project.as_str(), from_server.drawing_set.as_str(), from_server.user.as_str()), ("North Yard Storm", "C-Sheets Rev 2", "Sam")); // WHAT: project and set by name, the user trimmed. WHY: what the person sees.
        assert_eq!(from_server.now, "2026-11-03T14:05:00Z"); // WHAT: the time. WHY: in the form a plugin can write as it is.
        assert_eq!(from_server.app, "Excalibur View"); // WHAT: the program's name. WHY: provenance.
        assert!(!from_server.app_version.is_empty()); // WHAT: a version. WHY: the same.
        let from_folder = context_for(&asks, "Sam", None, &[], "North Yard C-Sheets", "D:20261103140500Z"); // WHAT: a drawing opened from a folder. WHY: no project to name.
        assert_eq!((from_folder.project.as_str(), from_folder.drawing_set.as_str()), ("", "North Yard C-Sheets")); // WHAT: no project, the file's name as the set. WHY: never a made-up project.
    }

    #[test]
    fn a_plugin_that_didnt_ask_is_told_when_but_not_who_or_which_project() {
        let project: hub::Project = serde_json::from_value(serde_json::json!({"id":"p1","number":"2630","name":"North Yard Storm","created":"","sets":1})).unwrap(); // WHAT: a project. WHY: the set below belongs to it.
        let set: hub::DrawingSet = serde_json::from_value(serde_json::json!({"id":"s1","project":"p1","name":"C-Sheets Rev 2","filename":"c.pdf","digest":"","bytes":0,"sheets":2,"uploaded":"","uploaded_by":"","revision":0})).unwrap(); // WHAT: a drawing set in that project. WHY: where the drawing came from.
        let quiet = plugin_api::Needs::default(); // WHAT: a command that asked for nothing. WHY: every plugin written before the field, and any that doesn't need to know.
        let told = context_for(&quiet, "Sam", Some(&set), &[project], "c", "D:20261103140500Z"); // WHAT: the same drawing as before. WHY: only the asking differs.
        assert_eq!((told.user.as_str(), told.project.as_str(), told.drawing_set.as_str()), ("", "", "")); // WHAT: no name, project or set. WHY: a third party's plugin learns them only when its signed manifest says it wants them.
        assert_eq!((told.app.as_str(), told.now.as_str()), ("Excalibur View", "2026-11-03T14:05:00Z")); // WHAT: the program and the time, still. WHY: neither says anything about the office.
    }

    #[test]
    fn quarter_inch_scale_reads_as_forty_eight() {
        let measure = annot::measure::imperial(48.0, "1/4\" = 1'-0\"", 16);
        let scale = scale_of(&measure);
        assert!((scale.ratio - 48.0).abs() < 1e-9, "{}", scale.ratio);
        assert!((feet_per_unit(&measure) - 1.0).abs() < 1e-12);
    }
}
