//! Files that arrive to be added rather than opened: a plugin or a tool
//! chest, double-clicked, dropped on the window, handed over by a second
//! start, or opened with this program by another one.
//!
//! What happened to each goes in one window, so a plugin and the tool chest
//! that goes with it, arriving together, get one answer between them. A
//! question — add this plugin? replace that tool chest? — is asked in the
//! same window, one at a time, and only once the file has been checked: a
//! plugin's signature is verified against the keys this program was built
//! with before anything about it is shown, and one that fails is refused
//! with nothing kept.

use std::collections::VecDeque;
use std::path::Path;

use egui::RichText;

use crate::app::App;
use crate::plugins::{self, Fits, Plugin, Rule};

/// A tool chest that has been read and is ready to keep.
pub struct ChestOffer {
    /// What it is called, cleaned for showing.
    pub name: String,
    /// The file it is kept as beside the program.
    pub kept_as: String,
    /// What is written there: Excalibur View's own form, whatever it came in as.
    pub bytes: Vec<u8>,
    /// The chest itself, for this session if it can't be kept.
    pub profile: chest::Profile,
    /// How many tools it has.
    pub tools: usize,
    /// It came in as a Revu file and was turned into Excalibur View's own.
    pub from_revu: bool,
    /// A different chest is already kept under the same file name.
    pub replacing: bool, // WHAT: it would replace one. WHY: the question and the line afterwards say so.
    /// The names, cleaned, of tool sets this seat already has that this
    /// chest's sets of the same name would take the place of.
    pub shadows: Vec<String>, // WHAT: the sets it takes over. WHY: the seat uses the newest chest's set of each name, so "Beams" with other weights would quietly become everybody's Beams here.
    /// It has toolbars, so as the newest chest it sets this seat's toolbars
    /// and the markups list's columns.
    pub layout: bool, // WHAT: it brings a layout. WHY: the newest chest with toolbars sets the layout, columns and their formulas included; said before the yes.
}

/// Something waiting for a yes or a no.
pub enum Question {
    /// A plugin that passed every check: add it, or update to it?
    Plugin {
        /// The checked plugin.
        plugin: Box<Plugin>, // WHAT: the plugin, boxed. WHY: its manifest is far bigger than a pointer, and the queue holds either kind of question.
        /// The very bytes that were checked, which are what is kept.
        bytes: Vec<u8>,
        /// Chosen with Plugins ▸ Add Plugin…, rather than opened from outside.
        from_menu: bool, // WHAT: where it came from. WHY: only the menu shares a plugin with the office.
    },
    /// A tool chest from outside, or one with the same name as one already
    /// here but different.
    Chest(Box<ChestOffer>), // WHAT: the offer, boxed. WHY: a chest offer is far bigger than a plugin's question, and the queue holds either.
}

/// What has arrived: questions still to answer and what has been done.
#[derive(Default)]
pub struct Arrivals {
    /// Waiting for an answer, first first.
    pub asking: VecDeque<Question>,
    /// What happened, one line each, until the window is closed.
    pub said: Vec<String>,
    /// Files that arrived while the questions were already full, and so were
    /// not read.
    pub left_out: usize, // WHAT: how many. WHY: said in one line, not one line each.
}

/// The most questions waiting at once.
pub const MOST_ASKING: usize = 8; // WHAT: eight. WHY: more than a plugin and its chests arriving together; a thousand files dropped or handed over can't queue a thousand questions or hold a thousand files in memory.

/// The most a tool chest file opened from outside may weigh.
pub const MOST_CHEST_BYTES: u64 = 64 * 1024 * 1024; // WHAT: 64 MB. WHY: real chests are a few megabytes at most; the file is read whole before anything is known about it.

/// The most tools a chest may have.
pub const MOST_TOOLS: usize = 50_000; // WHAT: fifty thousand. WHY: many times the biggest real chest; a chest with millions of tools would make every toolbar and list crawl.

/// The most lines the window keeps.
const MOST_SAID: usize = 30; // WHAT: thirty lines. WHY: a hundred files dropped at once must not make a window taller than the screen.

/// How long a reason may be in the window.
const SHOWN_REASON: usize = 400; // WHAT: four hundred characters. WHY: any real reason fits; a manifest stuffed with text does not get to fill the window.

impl Arrivals {
    /// Adds a line to what happened.
    pub fn say(&mut self, line: String) {
        self.said.push(line); // WHAT: the line, at the end. WHY: read top to bottom in the order things happened.
        if self.said.len() > MOST_SAID {
            // WHAT: past the limit. WHY: see MOST_SAID.
            self.said.remove(0); // WHAT: the oldest dropped. WHY: the newest are the ones being looked at.
        }
    }

    /// Whether as many questions are waiting as may.
    #[must_use] // WHAT: a caller that drops the answer gets a warning. WHY: asking has no other effect.
    pub fn full(&self) -> bool {
        self.asking.len() >= MOST_ASKING // WHAT: at the limit. WHY: see MOST_ASKING.
    }

    /// Whether there is room to read one more file that arrived. When there
    /// isn't, it is counted as left out, to be said in one line.
    pub fn room(&mut self) -> bool {
        if self.full() {
            // WHAT: no room. WHY: see MOST_ASKING.
            self.left_out += 1; // WHAT: counted. WHY: said in one line, not one each.
            return false; // WHAT: not read. WHY: the same.
        }
        true // WHAT: room. WHY: read it.
    }

    /// Whether a plugin with this digest is already waiting for an answer.
    #[must_use] // WHAT: a caller that drops the answer gets a warning. WHY: asking has no other effect.
    pub fn asking_about_plugin(&self, digest: &str) -> bool {
        self.asking
            .iter()
            .any(|q| matches!(q, Question::Plugin { plugin, .. } if plugin.digest == digest))
        // WHAT: the same file already asked about. WHY: a program that opens the same file twice gets one question, not two.
    }

    /// Whether a tool chest kept as this name is already waiting for an answer.
    #[must_use] // WHAT: a caller that drops the answer gets a warning. WHY: asking has no other effect.
    pub fn asking_about_chest(&self, kept_as: &str) -> bool {
        self.asking.iter().any(
            |q| matches!(q, Question::Chest(offer) if offer.kept_as.eq_ignore_ascii_case(kept_as)),
        ) // WHAT: the same name already asked about, in any case. WHY: one question per chest; Windows treats the names as one file.
    }
}

/// "1 tool", "42 tools".
fn tools_word(n: usize) -> String {
    format!("{n} tool{}", if n == 1 { "" } else { "s" }) // WHAT: the count with the right ending. WHY: plain English.
}

/// Reads a tool chest file: Excalibur View's own, or a Revu profile or tool
/// set, which is turned into Excalibur View's own once, here.
///
/// # Errors
///
/// Why it can't be read, in words for the person.
pub fn read_chest(path: &Path) -> Result<ChestOffer, String> {
    read_chest_within(path, MOST_CHEST_BYTES, MOST_TOOLS) // WHAT: with the usual limits. WHY: see MOST_CHEST_BYTES and MOST_TOOLS.
}

/// [`read_chest`], with the limits given, so they can be tested small.
///
/// # Errors
///
/// Why it can't be read, in words for the person.
pub fn read_chest_within(
    path: &Path,
    most_bytes: u64,
    most_tools: usize,
) -> Result<ChestOffer, String> {
    let file_name = plugins::shown(
        &path.file_name().unwrap_or_default().to_string_lossy(),
        plugins::SHOWN_NAME,
    ); // WHAT: the file's name, cleaned. WHY: it is shown, and a file name can carry a direction mark too.
    let bytes = plugins::read_at_most(path, most_bytes, "tool chest")
        .map_err(|why| format!("{file_name} was not added. {why}"))?; // WHAT: the whole file, but only an ordinary file and no more than the limit. WHY: it is read once, before anything is known about it; what is kept comes from these bytes.
    let Some(profile) = chest::Profile::read(&bytes) else {
        // WHAT: read as a chest. WHY: anything else is refused with a reason.
        return Err(format!(
            "{file_name} is not a tool chest this can read. It should be an Excalibur View tool chest \
             (.evtools), or a Revu profile (.bpx) or tool set (.btx) to import."
        )); // WHAT: the words Load Tool Chest already used. WHY: the same file gets the same answer however it arrives.
    };
    let tools: usize = profile.sets.iter().map(|s| s.tools.len()).sum(); // WHAT: every tool in every set. WHY: the number the person is told.
    if tools > most_tools {
        // WHAT: more tools than any real chest. WHY: see MOST_TOOLS.
        return Err(format!("{file_name} was not added. It has {tools} tools, more than the {most_tools} a tool chest may have."));
        // WHAT: refused, saying why. WHY: plain.
    }
    let native = chest::native::is_native(&bytes); // WHAT: already Excalibur View's own form. WHY: a Revu file is turned into it before it is kept.
    let stem = path
        .file_stem()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| profile.name.clone()); // WHAT: the file's name without its extension. WHY: the chest is kept under it, as it always was.
    let kept_as = format!("{stem}.{}", chest::native::EXTENSION); // WHAT: the name it is kept as. WHY: the same as before, so a chest kept by an older copy is the one this one replaces.
    let bytes = if native {
        bytes
    } else {
        chest::native::write(&profile, &format!("Revu: {file_name}"))
    }; // WHAT: the file as it came, or turned into Excalibur View's own. WHY: the Revu file is never needed again.
    let name = plugins::shown(
        if profile.name.trim().is_empty() {
            &stem
        } else {
            &profile.name
        },
        plugins::SHOWN_NAME,
    ); // WHAT: the chest's own name, or the file's, cleaned. WHY: shown in the window; the chest's name is text anybody could have written.
    let layout = ui::chrome::has_toolbars(&profile.toolbars); // WHAT: it brings toolbars. WHY: then, as the newest, it sets the layout.
    Ok(ChestOffer {
        name,
        kept_as,
        bytes,
        profile,
        tools,
        from_revu: !native,
        replacing: false,
        shadows: Vec::new(),
        layout,
    }) // WHAT: ready to keep. WHY: kept now, or after a question; what it would replace is filled in against this seat.
}

/// The names of the tool sets in `offer` that this seat already has, as it
/// stands in `now`, cleaned for showing.
#[must_use] // WHAT: a caller that drops the names gets a warning. WHY: finding them has no other effect.
pub fn shadows(offer: &chest::Profile, now: Option<&chest::Profile>) -> Vec<String> {
    let Some(now) = now else { return Vec::new() }; // WHAT: no tools here yet. WHY: nothing to take over.
    let have: std::collections::HashSet<&str> = now.sets.iter().map(|s| s.title.as_str()).collect(); // WHAT: the set names this seat has. WHY: sets are matched by name.
    let mut out: Vec<String> = Vec::new(); // WHAT: the names found. WHY: filled below.
    for set in &offer.sets {
        // WHAT: each set the chest brings. WHY: each is matched on its own.
        let name = plugins::shown(&set.title, plugins::SHOWN_NAME); // WHAT: its name, cleaned. WHY: shown in the question.
        if have.contains(set.title.as_str()) && !out.contains(&name) {
            // WHAT: a set this seat has, named once. WHY: the same name twice is said once.
            out.push(name); // WHAT: noted. WHY: said.
        }
    }
    out // WHAT: the sets it takes over. WHY: said before the yes.
}

/// The names in a list, for a sentence: "'A'", "'A', 'B'", "'A', 'B', 'C' and 4 more".
fn some_names(names: &[String]) -> String {
    let shown: Vec<String> = names.iter().take(3).map(|n| format!("'{n}'")).collect(); // WHAT: the first three, quoted. WHY: enough to recognise; a hundred names would push the buttons off the screen.
    match names.len().saturating_sub(3) {
        // WHAT: how many more. WHY: said as a number.
        0 => shown.join(", "), // WHAT: all of them. WHY: three or fewer.
        more => format!("{} and {more} more", shown.join(", ")), // WHAT: the rest counted. WHY: the same.
    }
}

/// What the person is told a chest brings beyond its tools: the sets of
/// theirs it takes the place of, and the layout. Empty when it brings nothing
/// more.
fn chest_takes_over(offer: &ChestOffer) -> String {
    let mut text = String::new(); // WHAT: the sentences. WHY: built below.
    if !offer.shadows.is_empty() {
        // WHAT: it takes over sets this seat has. WHY: their weights and settings become this chest's.
        text.push_str(&format!(
            " Its tool sets take the place of yours with the same names: {}.",
            some_names(&offer.shadows)
        )); // WHAT: which ones. WHY: so nobody's Beams changes without them knowing.
    }
    if offer.layout {
        // WHAT: it brings toolbars. WHY: the newest chest with toolbars sets them.
        text.push_str(" It also sets your toolbars and the markups list's columns.");
        // WHAT: said. WHY: the same.
    }
    text // WHAT: the sentences. WHY: added to the question and the line afterwards.
}

/// The question for a tool chest, with its two buttons.
#[must_use] // WHAT: a caller that drops the question gets a warning. WHY: putting it into words has no other effect.
pub fn chest_question(offer: &ChestOffer) -> (String, &'static str, &'static str) {
    let tools = tools_word(offer.tools); // WHAT: how many tools. WHY: said.
    let more = chest_takes_over(offer); // WHAT: what else it changes. WHY: said before the yes.
    if offer.replacing {
        // WHAT: one by that name is here. WHY: replacing asks in its own words.
        (format!("A tool chest called '{}' is already here. Replace it with the one just opened ({tools})?{more}", offer.name), "Replace", "Keep the one I have")
    // WHAT: the replace question. WHY: a chest is replaced only when the person says so.
    } else {
        (
            format!("Add the tool chest '{}' ({tools})?{more}", offer.name),
            "Add",
            "Not now",
        ) // WHAT: the add question. WHY: a chest opened from outside changes this seat's tools, so it is asked about once.
    }
}

/// What to do with a tool chest, given what is already in `dir`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChestPlan {
    /// Nothing by that name: keep it.
    New,
    /// The very same file is already kept: nothing to do.
    Same,
    /// A different chest by that name is kept: ask before replacing it.
    Differs,
}

/// Where `offer` stands against what is kept in `dir`.
#[must_use] // WHAT: a caller that drops the plan gets a warning. WHY: working it out has no other effect.
pub fn chest_plan(dir: &Path, offer: &ChestOffer) -> ChestPlan {
    match std::fs::read(dir.join(&offer.kept_as)) {
        // WHAT: whatever is kept under that name. WHY: the same name is the only clash; chests with different names load side by side.
        Err(_) => ChestPlan::New, // WHAT: nothing there. WHY: keep it without asking.
        Ok(have) if have == offer.bytes => ChestPlan::Same, // WHAT: byte for byte the same. WHY: nothing would change, so nothing is asked.
        Ok(_) => ChestPlan::Differs, // WHAT: something else by that name. WHY: replaced only once the person says so.
    }
}

/// What to do with a tool chest that isn't already here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChestStep {
    /// Keep it without asking.
    KeepNow,
    /// Ask in the window first.
    Ask,
}

/// Whether a chest that isn't the very one already here is kept at once or
/// asked about. `from_outside`: double-clicked, dropped or handed over,
/// rather than chosen with Load Tool Chest.
#[must_use] // WHAT: a caller that drops the step gets a warning. WHY: working it out has no other effect.
pub fn chest_step(plan: ChestPlan, from_outside: bool) -> ChestStep {
    if plan == ChestPlan::New && !from_outside {
        // WHAT: chosen from the menu, nothing by that name. WHY: the person just picked it, and nothing of theirs is replaced.
        ChestStep::KeepNow // WHAT: kept. WHY: the same.
    } else {
        ChestStep::Ask // WHAT: asked. WHY: a chest from outside can take over this seat's tool sets, weights and toolbars, so it is asked about once; one replacing another is always asked about.
    }
}

/// Whether Plugins ▸ Add Plugin… asks before adding: only for an update.
/// The same or an older version, or the office's own, is never added.
#[must_use] // WHAT: a caller that drops the answer gets a warning. WHY: working it out has no other effect.
pub fn menu_asks(fits: &Fits) -> bool {
    !matches!(fits, Fits::New) // WHAT: anything but new asks (an update), or is turned away before this by plugin_not_asked. WHY: replacing a plugin somebody uses is asked about, however it arrived.
}

/// The rule for a plugin, given where it came from.
#[must_use] // WHAT: a caller that drops the rule gets a warning. WHY: working it out has no other effect.
pub fn rule_for(rule: Rule, from_menu: bool) -> Rule {
    if from_menu {
        rule
    } else {
        rule.from_outside()
    } // WHAT: the menu's rule, or never the office's. WHY: see Rule::from_outside.
}

/// Keeps a tool chest in `dir`. True when it was written.
pub fn keep_chest(dir: &Path, offer: &ChestOffer) -> bool {
    std::fs::create_dir_all(dir).is_ok()
        && plugins::write_replacing(&dir.join(&offer.kept_as), &offer.bytes).is_ok()
    // WHAT: the folder made and the file written as a new file put in place. WHY: it loads again next time, and a link or second name left at that path is replaced, not written through.
}

/// The question for a plugin that passed its checks, and the word on the yes
/// button. Every name in it is cleaned first.
#[must_use] // WHAT: a caller that drops the question gets a warning. WHY: putting it into words has no other effect.
pub fn plugin_question(plugin: &Plugin, fits: &Fits, rule: Rule) -> (String, &'static str) {
    let name = plugins::shown(&plugin.manifest.name, plugins::SHOWN_NAME); // WHAT: the plugin's name, cleaned. WHY: it comes from the plugin's own manifest, and must read only as what it is.
    let version = plugins::shown(&plugin.manifest.version, plugins::SHOWN_NAME); // WHAT: its version, cleaned. WHY: the same.
    let key = plugins::shown(&plugin.key, plugins::SHOWN_NAME); // WHAT: the key that signed it. WHY: it matched one built into this program, but it is cleaned like everything else shown.
    let (mut text, yes) = match fits {
        // WHAT: the question itself. WHY: an update reads differently from an addition.
        Fits::Update { have } => (
            format!(
                "Update '{name}' to {version}? This computer has {}. It is signed by {key}.",
                plugins::shown(have, plugins::SHOWN_NAME)
            ), // WHAT: from which version to which. WHY: the person sees it is newer.
            "Update", // WHAT: the yes button. WHY: says what it does.
        ),
        _ => (
            format!(
                "Add the plugin '{name}' {version}, signed by {key}? It will show under Plugins."
            ), // WHAT: the one plain question. WHY: who made it and where it will be.
            "Add", // WHAT: the yes button. WHY: says what it does.
        ),
    };
    if plugin.manifest.commands.iter().any(|c| c.needs.context) {
        // WHAT: a command asks to be told who runs it and which project. WHY: the Manage Plugins window says so too; the person should hear it before adding it.
        text.push_str(" It will be told who runs it, and the office project and drawing set.");
        // WHAT: said plainly. WHY: the same.
    }
    match rule { // WHAT: where it goes on this copy. WHY: said before the yes, not after.
        Rule::HereAndOffice => text.push_str(" You are an administrator, so it is also shared with everyone in your office."), // WHAT: it goes to the office too. WHY: what Add Plugin… does for an administrator.
        Rule::HereNotOffice => text.push_str(" It is added on this computer only. Only an administrator can add it for everyone in your office."), // WHAT: this computer only. WHY: who can give it to the office, said plainly.
        Rule::HereShareFromMenu => text.push_str(" It is added on this computer only. To share it with everyone in your office, add it with Plugins \u{25b8} Add Plugin\u{2026}"), // WHAT: this computer only, and how to share it. WHY: a file opened from outside never goes to the office on one click.
        Rule::Here | Rule::NotOnThisCopy => {} // WHAT: nothing more to say. WHY: a copy on its own; a copy that doesn't run plugins never asks.
    }
    (text, yes) // WHAT: the question and the button. WHY: the window shows both.
}

/// What is said when a checked plugin is already here, or is the office's.
/// `None` when it should be asked about.
#[must_use] // WHAT: a caller that drops the line gets a warning. WHY: putting it into words has no other effect.
pub fn plugin_not_asked(plugin: &Plugin, fits: &Fits) -> Option<String> {
    let name = plugins::shown(&plugin.manifest.name, plugins::SHOWN_NAME); // WHAT: the name, cleaned. WHY: shown.
    let version = plugins::shown(&plugin.manifest.version, plugins::SHOWN_NAME); // WHAT: the version, cleaned. WHY: shown.
    match fits {
        Fits::New | Fits::Update { .. } => None, // WHAT: to be asked about. WHY: something would change.
        Fits::Have { have } if hub::update::compare(&plugin.manifest.version, have) == std::cmp::Ordering::Equal => {
            Some(format!("The plugin '{name}' {version} is already added. Nothing changed.")) // WHAT: the same version. WHY: nothing to do, and no question.
        }
        Fits::Have { have } => Some(format!(
            "This computer already has '{name}' {}, which is newer than the {version} just opened. Nothing changed.",
            plugins::shown(have, plugins::SHOWN_NAME)
        )), // WHAT: an older one arrived. WHY: never a downgrade, and the person is told why nothing happened.
        Fits::Office { .. } => Some(format!(
            "The plugin '{name}' comes from your office's server, so nothing changed here. \
             An administrator adds new versions for everyone."
        )), // WHAT: the office hands this one out. WHY: who can change it, said plainly.
    }
}

impl App {
    /// Says what happened in the window, and on the status line.
    fn say_arrived(&mut self, line: String) {
        self.status = line.clone(); // WHAT: on the status line too. WHY: where the menus' answers have always gone.
        self.arrivals.say(line); // WHAT: in the window. WHY: hard to miss, and one place for everything that arrived together.
    }

    /// The rule for adding a plugin here, as things stand now.
    fn plugin_rule(&self) -> Rule {
        let signed_in = self.standing.signed_in(); // WHAT: signed in to an office's server. WHY: then only an administrator adds for the office.
        let admin = self.standing.who.as_ref().map(|w| w.role) == Some(hub::Role::Admin); // WHAT: as an administrator. WHY: the same test Add Plugin… uses.
        plugins::rule(
            crate::edition::runs_plugins_itself(),
            signed_in,
            admin && !self.standing.base.is_empty(),
        ) // WHAT: the rule. WHY: the same rule as the menu, worked out in one place.
    }

    /// A tool chest arrived from outside (`from_outside`: double-clicked,
    /// dropped, handed over), or was chosen with Load Tool Chest. One from
    /// outside is asked about once before it is kept; one chosen from the
    /// menu is kept at once. Either asks before replacing a different chest
    /// of the same name.
    pub fn take_chest(&mut self, path: &Path, from_outside: bool) {
        if !self.arrivals.room() {
            // WHAT: as many questions waiting as may be. WHY: see MOST_ASKING; the file is not even read, and is counted as left out.
            return; // WHAT: left. WHY: the same.
        }
        let mut offer = match read_chest(path) {
            // WHAT: read it. WHY: a file that isn't a chest is refused with a reason.
            Ok(offer) => offer,
            Err(why) => return self.say_arrived(why), // WHAT: refused. WHY: said where the person is looking.
        };
        let plan = chest_plan(&crate::server::profiles_folder(), &offer); // WHAT: against what is kept. WHY: decides whether to ask.
        if plan == ChestPlan::Same {
            // WHAT: already here, the very same. WHY: opening it twice is not a reason to ask anything.
            let line = format!(
                "The tool chest '{}' is already added ({}). Nothing changed.",
                offer.name,
                tools_word(offer.tools)
            ); // WHAT: said. WHY: so the person knows it is there.
            return self.say_arrived(line); // WHAT: said. WHY: the same.
        }
        offer.replacing = plan == ChestPlan::Differs; // WHAT: whether it would replace one. WHY: asked in other words.
        offer.shadows = shadows(&offer.profile, self.profile.as_ref()); // WHAT: the sets of this seat's it takes over. WHY: said before the yes, and in the line afterwards.
        match chest_step(plan, from_outside) {
            // WHAT: keep now or ask. WHY: worked out in one place, tested.
            ChestStep::KeepNow => self.keep_offered_chest(offer), // WHAT: kept now. WHY: chosen from the menu and nothing by that name, as Load Tool Chest always did.
            ChestStep::Ask if self.arrivals.asking_about_chest(&offer.kept_as) => {} // WHAT: already being asked about. WHY: one question per chest.
            ChestStep::Ask => self
                .arrivals
                .asking
                .push_back(Question::Chest(Box::new(offer))), // WHAT: asked in the window. WHY: a chest from outside, or one replacing another, changes this seat's tools only once the person says so.
        }
    }

    /// Keeps a tool chest and loads it with everything else this seat has.
    fn keep_offered_chest(&mut self, offer: ChestOffer) {
        let kept = keep_chest(&crate::server::profiles_folder(), &offer); // WHAT: written beside the program. WHY: it loads again next time.
        let (name, tools, more) = (
            offer.name.clone(),
            tools_word(offer.tools),
            chest_takes_over(&offer),
        ); // WHAT: what is said about it. WHY: the offer is moved below.
        self.profile = if kept {
            crate::profiles::load().or(Some(offer.profile))
        } else {
            Some(offer.profile)
        }; // WHAT: put together with every other chest here, or this one alone if it couldn't be kept. WHY: as Load Tool Chest always did.
        let line = match (kept, offer.replacing, offer.from_revu) { // WHAT: what happened, in words. WHY: one clear line.
            (false, _, _) => format!("The tool chest '{name}': {tools} loaded, but it could not be saved beside the program, so you will have to load it again next time."), // WHAT: loaded, not kept. WHY: worth saying, not worth refusing over.
            (true, true, _) => format!("The tool chest '{name}' was replaced: {tools}. It will load again next time."), // WHAT: replaced. WHY: the person said yes to it.
            (true, false, true) => format!("Tool chest '{name}' imported from Revu: {tools}, kept as {}. The Revu file is not needed again.", plugins::shown(&offer.kept_as, plugins::SHOWN_NAME)), // WHAT: imported and kept. WHY: as before.
            (true, false, false) => format!("Tool chest '{name}' added: {tools}. It will load again next time."), // WHAT: the plain case. WHY: the words asked for.
        };
        self.say_arrived(format!("{line}{more}")); // WHAT: said, with what it took over. WHY: chosen from the menu it was not asked about, so it is told afterwards.
    }

    /// A plugin arrived from outside: checked first, then asked about in the
    /// window. Never shared with the office from here.
    pub fn take_plugin(&mut self, path: &Path) {
        if self.plugin_rule() == Rule::NotOnThisCopy {
            // WHAT: this copy doesn't run plugins. WHY: the office's server does; nothing is read or checked here.
            return self.say_arrived(plugins::OFFICE_RUNS_PLUGINS.into()); // WHAT: who adds them, said plainly. WHY: the same words as Add Plugin….
        }
        if !self.arrivals.room() {
            // WHAT: as many questions waiting as may be. WHY: see MOST_ASKING; the file is not even read, and is counted as left out.
            return; // WHAT: left. WHY: the same.
        }
        let file_name = plugins::shown(
            &path.file_name().unwrap_or_default().to_string_lossy(),
            plugins::SHOWN_NAME,
        ); // WHAT: the file's name, cleaned. WHY: shown when it is refused.
        let (plugin, bytes) = match plugins::check_file(&crate::install::plugin_trusted(), path) {
            // WHAT: the signature, against the keys built in, and the sandbox's rules. WHY: nothing about a plugin is shown or asked until it passes.
            Ok(checked) => checked,
            Err(why) => {
                return self.say_arrived(format!(
                    "{file_name} was not added. {}",
                    plugins::shown(&why, SHOWN_REASON)
                ))
            } // WHAT: refused, with the reason, cleaned, and nothing kept. WHY: the reason can quote the file's own words.
        };
        let fits = plugins::fits(&self.plugins, &plugin.manifest); // WHAT: against the plugins already here. WHY: never a downgrade, and no question when nothing would change.
        if let Some(line) = plugin_not_asked(&plugin, &fits) {
            // WHAT: already here, or the office's. WHY: said, not asked.
            return self.say_arrived(line); // WHAT: said. WHY: the same.
        }
        if !self.arrivals.asking_about_plugin(&plugin.digest) {
            // WHAT: not already being asked about. WHY: one question per file.
            self.arrivals.asking.push_back(Question::Plugin {
                plugin: Box::new(plugin),
                bytes,
                from_menu: false,
            }); // WHAT: asked in the window. WHY: the one confirmation.
        }
    }

    /// Plugins ▸ Add Plugin…: the same checks as a plugin opened from
    /// outside, and never a downgrade. A new plugin is added at once, since
    /// the person just chose it; an update is asked about first.
    pub fn add_chosen_plugin(&mut self, path: &Path) {
        let (plugin, bytes) = match plugins::check_file(&crate::install::plugin_trusted(), path) {
            // WHAT: checked, nothing kept. WHY: the same check as from outside.
            Ok(checked) => checked,
            Err(why) => return self.status = plugins::shown(&why, SHOWN_REASON), // WHAT: refused, with the reason. WHY: on the status line, where the menu's answers go.
        };
        let fits = plugins::fits(&self.plugins, &plugin.manifest); // WHAT: against the plugins already here. WHY: never a downgrade from the menu either.
        if let Some(line) = plugin_not_asked(&plugin, &fits) {
            // WHAT: the same or older, or the office's. WHY: nothing changes, and the person is told why.
            return self.status = line; // WHAT: said. WHY: the same.
        }
        if !menu_asks(&fits) {
            // WHAT: not here yet. WHY: the person just chose it, so nothing is asked.
            let line = self.add_offered_plugin(&plugin, &bytes, true); // WHAT: kept, and shared by an administrator. WHY: what Add Plugin… has always done.
            return self.status = line; // WHAT: said. WHY: on the status line, as before.
        }
        if self.arrivals.asking_about_plugin(&plugin.digest) {
            // WHAT: already being asked about. WHY: one question per file.
            return; // WHAT: nothing more. WHY: the window is already asking.
        }
        if self.arrivals.full() {
            // WHAT: as many questions waiting as may be. WHY: see MOST_ASKING.
            return self.status = "Answer the questions in the Adding to Excalibur View window first, then add it again.".into();
            // WHAT: said. WHY: nothing is lost.
        }
        self.arrivals.asking.push_back(Question::Plugin {
            plugin: Box::new(plugin),
            bytes,
            from_menu: true,
        }); // WHAT: the update asked about. WHY: replacing a plugin somebody uses is asked about, however it arrived.
    }

    /// Yes to a plugin: kept and the menu rebuilt. Shared with the office
    /// only when an administrator added it from the menu. The line that says
    /// what happened comes back.
    fn add_offered_plugin(&mut self, plugin: &Plugin, bytes: &[u8], from_menu: bool) -> String {
        let fits = plugins::fits(&self.plugins, &plugin.manifest); // WHAT: worked out again. WHY: another file may have arrived while the question was up; still never a downgrade.
        if let Some(line) = plugin_not_asked(plugin, &fits) {
            // WHAT: nothing to do now. WHY: the same.
            return line; // WHAT: said. WHY: the same.
        }
        let rule = rule_for(self.plugin_rule(), from_menu); // WHAT: the rule now, and never the office's for a file from outside. WHY: the person may have signed in or out while the question was up; see Rule::from_outside.
        if rule == Rule::NotOnThisCopy {
            // WHAT: never true here. WHY: such a copy never asks; checked again rather than trusted.
            return plugins::OFFICE_RUNS_PLUGINS.into(); // WHAT: refused. WHY: the same.
        }
        let kept = match plugins::keep_in(&plugins::folder(), plugin, bytes) {
            // WHAT: the very bytes that were checked, kept. WHY: not the file read again, which could have changed since.
            Ok(kept) => kept,
            Err(why) => return why, // WHAT: could not be written. WHY: said.
        };
        self.reload_plugins(); // WHAT: the folders read again, checking every plugin again. WHY: the menu shows it, and it is checked as every plugin is on loading.
        let name = plugins::shown(&plugin.manifest.name, plugins::SHOWN_NAME); // WHAT: the name, cleaned. WHY: shown.
        let version = plugins::shown(&plugin.manifest.version, plugins::SHOWN_NAME); // WHAT: the version, cleaned. WHY: shown.
        let done = match fits {
            // WHAT: added or updated. WHY: the words differ.
            Fits::Update { .. } => format!("'{name}' updated to {version}."), // WHAT: updated. WHY: plain.
            _ => format!("The plugin '{name}' {version} added."), // WHAT: added. WHY: plain.
        };
        match rule { // WHAT: where it is, and who else gets it. WHY: the person knows where to find it.
            Rule::HereAndOffice => {
                self.ask(crate::server::Ask::SharePlugin(kept.path)); // WHAT: shared with the office. WHY: what Add Plugin… does for an administrator.
                format!("{done} It is in the Plugins menu, under '{name}'. Sharing it with your office…") // WHAT: said. WHY: the same.
            }
            Rule::HereNotOffice => format!("{done} It is in the Plugins menu, under '{name}', on this computer only. Only an administrator can add it for everyone in your office."), // WHAT: this computer only. WHY: who can do more, said plainly.
            Rule::HereShareFromMenu => format!("{done} It is in the Plugins menu, under '{name}', on this computer only. To share it with your office, add it with Plugins \u{25b8} Add Plugin\u{2026}"), // WHAT: this computer only, and how to share it. WHY: see Rule::from_outside.
            Rule::Here | Rule::NotOnThisCopy => format!("{done} It is in the Plugins menu, under '{name}'."), // WHAT: where it is. WHY: the plain case.
        }
    }

    /// The window for what arrived, drawn every frame while there is
    /// something in it.
    pub fn arrivals_window(&mut self, ctx: &egui::Context) {
        if self.arrivals.asking.is_empty()
            && self.arrivals.said.is_empty()
            && self.arrivals.left_out == 0
        {
            // WHAT: nothing arrived. WHY: no window.
            return; // WHAT: done. WHY: the same.
        }
        let theme = self.chrome.theme; // WHAT: the colours. WHY: the faint line below.
        let mut said = self.arrivals.said.clone(); // WHAT: the lines to show. WHY: the window's closure can't borrow the program.
        if self.arrivals.left_out > 0 {
            // WHAT: files that weren't read. WHY: said in one line however many there were.
            let n = self.arrivals.left_out; // WHAT: how many. WHY: said.
            said.push(format!("{n} more file{} not opened, so as not to ask too much at once. Open {} again once these are answered.", if n == 1 { " was" } else { "s were" }, if n == 1 { "it" } else { "them" }));
            // WHAT: said plainly. WHY: nothing was lost; the person knows to open them again.
        }
        let rule = self.plugin_rule(); // WHAT: the rule now. WHY: the question says where the plugin goes.
        let question: Option<(String, &'static str, &'static str)> =
            self.arrivals.asking.front().map(|q| match q {
                // WHAT: the first question, in words, with its two buttons. WHY: one at a time.
                Question::Plugin {
                    plugin, from_menu, ..
                } => {
                    let rule = rule_for(rule, *from_menu); // WHAT: the rule for where it came from. WHY: one opened from outside is never shared with the office; see Rule::from_outside.
                    let (text, yes) = plugin_question(
                        plugin,
                        &plugins::fits(&self.plugins, &plugin.manifest),
                        rule,
                    ); // WHAT: the plugin's question. WHY: worked out each frame, so it is never stale.
                    (text, yes, "Not now") // WHAT: with Not now. WHY: the words asked for.
                }
                Question::Chest(offer) => chest_question(offer), // WHAT: the chest's question. WHY: added or replaced only when the person says so, told first what it takes over.
            });
        let waiting = self.arrivals.asking.len().saturating_sub(1); // WHAT: questions after this one. WHY: the person knows there is more.
        let mut answer: Option<bool> = None; // WHAT: yes or no, if clicked this frame. WHY: carried out after the window.
        let mut close = false; // WHAT: OK clicked. WHY: the same.
        egui::Window::new("Adding to Excalibur View") // WHAT: one window for everything that arrived. WHY: one combined answer.
            .id(egui::Id::new("arrivals")) // WHAT: a fixed id. WHY: the title is the same, but an id keeps it one window.
            .collapsible(false) // WHAT: can't be folded away. WHY: a question must not hide.
            .resizable(false) // WHAT: sized to what it says. WHY: a short message.
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)) // WHAT: in the middle. WHY: where it will be seen.
            .max_width(560.0) // WHAT: no wider than this. WHY: the lines wrap and stay readable.
            .show(ctx, |ui| {
                for line in &said {
                    // WHAT: each thing that happened. WHY: one combined result.
                    ui.label(line); // WHAT: shown. WHY: the same.
                }
                match &question {
                    Some((text, yes, no)) => {
                        if !said.is_empty() {
                            // WHAT: lines above the question. WHY: a line between them.
                            ui.separator(); // WHAT: the line. WHY: the same.
                        }
                        ui.label(RichText::new(text).strong()); // WHAT: the question. WHY: stands out from what already happened.
                        if waiting > 0 {
                            // WHAT: more questions after this one. WHY: said.
                            ui.label(
                                RichText::new(format!("{waiting} more after this."))
                                    .color(theme.faint)
                                    .size(11.0),
                            ); // WHAT: how many. WHY: the same.
                        }
                        ui.add_space(8.0); // WHAT: room. WHY: the buttons stand apart from the words.
                        ui.horizontal(|ui| {
                            if ui.button(*yes).clicked() {
                                // WHAT: yes. WHY: the person said so.
                                answer = Some(true); // WHAT: noted. WHY: carried out below.
                            }
                            if ui.button(*no).clicked() {
                                // WHAT: no. WHY: the same.
                                answer = Some(false); // WHAT: noted. WHY: the same.
                            }
                        });
                    }
                    None => {
                        ui.add_space(8.0); // WHAT: room. WHY: the button stands apart.
                        if ui.button("OK").clicked() {
                            // WHAT: closes it. WHY: everything is done; nothing needs it.
                            close = true; // WHAT: noted. WHY: carried out below.
                        }
                    }
                }
            });
        if close {
            // WHAT: OK. WHY: the window goes.
            self.arrivals.said.clear(); // WHAT: nothing left to show. WHY: the same.
            self.arrivals.left_out = 0; // WHAT: the count of files left out, said and done. WHY: the same.
        }
        let Some(yes) = answer else { return }; // WHAT: no answer this frame. WHY: nothing to do.
        let Some(question) = self.arrivals.asking.pop_front() else {
            return;
        }; // WHAT: the question answered. WHY: the next one shows.
        match (question, yes) {
            (
                Question::Plugin {
                    plugin,
                    bytes,
                    from_menu,
                },
                true,
            ) => {
                let line = self.add_offered_plugin(&plugin, &bytes, from_menu); // WHAT: kept. WHY: yes.
                self.say_arrived(line); // WHAT: said. WHY: in the window that asked.
            }
            (Question::Plugin { plugin, .. }, false) => {
                let line = format!(
                    "The plugin '{}' was not added.",
                    plugins::shown(&plugin.manifest.name, plugins::SHOWN_NAME)
                ); // WHAT: not added. WHY: Not now leaves nothing behind.
                self.say_arrived(line); // WHAT: said. WHY: the person knows nothing changed.
            }
            (Question::Chest(offer), true) => self.keep_offered_chest(*offer), // WHAT: kept, or replaced. WHY: yes.
            (Question::Chest(offer), false) => {
                let line = if offer.replacing {
                    format!("Kept the tool chest '{}' you had.", offer.name)
                } else {
                    format!("The tool chest '{}' was not added.", offer.name)
                }; // WHAT: not kept. WHY: said in the words of the question.
                self.say_arrived(line); // WHAT: said. WHY: the same.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, name: &str, version: &str) -> plugin_api::Manifest {
        serde_json::from_value(serde_json::json!({"id": id, "name": name, "version": version, "commands": [{"id": "go", "name": "Go"}]})).unwrap()
        // WHAT: a manifest with one command. WHY: what every plugin has.
    }

    fn plugin(id: &str, name: &str, version: &str, from_office: bool) -> Plugin {
        Plugin {
            manifest: manifest(id, name, version),
            wasm: Default::default(),
            digest: format!("{id}-{version}"),
            key: "test-2026".into(),
            from_office,
            path: Default::default(),
        } // WHAT: a plugin without its code. WHY: the questions read only its manifest and key.
    }

    fn shelf_with(plugins: Vec<Plugin>) -> plugins::Shelf {
        plugins::Shelf {
            plugins,
            refused: Vec::new(),
        } // WHAT: a shelf holding these. WHY: what fits is checked against.
    }

    #[test]
    fn a_new_plugin_is_asked_about_in_one_plain_question() {
        let incoming = plugin("takeoff-check", "Takeoff Check", "1.2.0", false); // WHAT: a plugin not here yet. WHY: the ordinary case.
        let fits = plugins::fits(&shelf_with(Vec::new()), &incoming.manifest); // WHAT: against an empty shelf. WHY: new.
        assert_eq!(fits, Fits::New); // WHAT: new. WHY: nothing has its id.
        assert_eq!(plugin_not_asked(&incoming, &fits), None); // WHAT: to be asked. WHY: something would change.
        let (text, yes) = plugin_question(&incoming, &fits, Rule::Here); // WHAT: the question. WHY: what the person reads.
        assert_eq!(text, "Add the plugin 'Takeoff Check' 1.2.0, signed by test-2026? It will show under Plugins."); // WHAT: the words asked for. WHY: name, version and who signed it.
        assert_eq!(yes, "Add"); // WHAT: the button. WHY: says what it does.
    }

    #[test]
    fn a_newer_version_asks_to_update_and_the_same_or_older_never_downgrades() {
        let shelf = shelf_with(vec![plugin(
            "takeoff-check",
            "Takeoff Check",
            "1.2.0",
            false,
        )]); // WHAT: 1.2.0 already here. WHY: the case.
        let newer = plugin("takeoff-check", "Takeoff Check", "1.10.0", false); // WHAT: 1.10.0, newer read as a person reads it. WHY: not newer as text.
        let fits = plugins::fits(&shelf, &newer.manifest); // WHAT: where it fits. WHY: an update.
        assert_eq!(
            fits,
            Fits::Update {
                have: "1.2.0".into()
            }
        ); // WHAT: an update from 1.2.0. WHY: the same.
        let (text, yes) = plugin_question(&newer, &fits, Rule::Here); // WHAT: the question. WHY: what the person reads.
        assert!(
            text.starts_with("Update 'Takeoff Check' to 1.10.0?"),
            "{text}"
        ); // WHAT: the words asked for. WHY: the same.
        assert_eq!(yes, "Update"); // WHAT: the button. WHY: says what it does.
        for (version, says) in [
            ("1.2.0", "is already added"),
            ("1.1.9", "which is newer than the 1.1.9"),
        ] {
            // WHAT: the same version and an older one. WHY: neither may replace what is here.
            let incoming = plugin("takeoff-check", "Takeoff Check", version, false); // WHAT: the arriving one. WHY: the case.
            let fits = plugins::fits(&shelf, &incoming.manifest); // WHAT: where it fits. WHY: already have it.
            assert_eq!(
                fits,
                Fits::Have {
                    have: "1.2.0".into()
                },
                "{version}"
            ); // WHAT: have it. WHY: never a downgrade.
            let line = plugin_not_asked(&incoming, &fits).expect("said, not asked"); // WHAT: a line, no question. WHY: nothing would change.
            assert!(
                line.contains(says) && line.ends_with("Nothing changed."),
                "{line}"
            ); // WHAT: says why. WHY: the person knows nothing happened.
        }
    }

    #[test]
    fn the_offices_own_plugin_is_left_to_the_office() {
        let shelf = shelf_with(vec![plugin(
            "takeoff-check",
            "Takeoff Check",
            "1.0.0",
            true,
        )]); // WHAT: the office hands this one out. WHY: the case.
        let incoming = plugin("takeoff-check", "Takeoff Check", "2.0.0", false); // WHAT: a newer one opened here. WHY: it would never run; the office's wins on load.
        let fits = plugins::fits(&shelf, &incoming.manifest); // WHAT: where it fits. WHY: the office's.
        assert_eq!(
            fits,
            Fits::Office {
                have: "1.0.0".into()
            }
        ); // WHAT: the office's. WHY: an administrator updates it for everybody.
        assert!(plugin_not_asked(&incoming, &fits)
            .unwrap()
            .contains("An administrator adds new versions")); // WHAT: who can, said plainly. WHY: the office's rule kept.
    }

    #[test]
    fn who_may_add_a_plugin_follows_the_menus_rule() {
        assert_eq!(plugins::rule(false, true, true), Rule::NotOnThisCopy); // WHAT: a copy that doesn't run plugins, even an administrator's. WHY: the office's server runs them.
        assert_eq!(plugins::rule(true, true, true), Rule::HereAndOffice); // WHAT: an administrator signed in. WHY: shared with the office, as from the menu.
        assert_eq!(plugins::rule(true, true, false), Rule::HereNotOffice); // WHAT: anybody else signed in. WHY: this computer only.
        assert_eq!(plugins::rule(true, false, false), Rule::Here); // WHAT: a copy on its own. WHY: this computer is all there is.
        let incoming = plugin("takeoff-check", "Takeoff Check", "1.0.0", false); // WHAT: a plugin. WHY: the questions below.
        let (text, _) = plugin_question(&incoming, &Fits::New, Rule::HereNotOffice); // WHAT: asked of somebody who isn't an administrator. WHY: told who can add it for everyone.
        assert!(
            text.contains("Only an administrator can add it for everyone in your office."),
            "{text}"
        ); // WHAT: said plainly. WHY: the office's rule.
        let (text, _) = plugin_question(&incoming, &Fits::New, Rule::HereAndOffice); // WHAT: asked of an administrator. WHY: told it goes to the office too.
        assert!(
            text.contains("also shared with everyone in your office"),
            "{text}"
        ); // WHAT: said before the yes. WHY: no surprise afterwards.
    }

    #[test]
    fn a_crafted_name_cant_reach_into_the_question() {
        let mut incoming = plugin(
            "x",
            "Safe\u{202e}exe.gnp\u{200b}\n\nAdd anything? ",
            "1.0\u{2066}.0\u{0007}",
            false,
        ); // WHAT: a right-to-left override, a zero-width space, new lines, an isolate and a bell. WHY: the tricks a name could use to read as something else.
        incoming.key = "test\u{202d}-2026".into(); // WHAT: an override in the key's name too. WHY: everything shown is cleaned, not only the name.
        let (text, _) = plugin_question(&incoming, &Fits::New, Rule::Here); // WHAT: the question. WHY: what the person reads.
        assert!(
            !text
                .chars()
                .any(|c| c.is_control() || plugin_api::invisible(c)),
            "{text:?}"
        ); // WHAT: nothing invisible or controlling left. WHY: the question reads as what it is.
        assert!(
            text.contains("'Safeexe.gnp Add anything?' 1.0.0, signed by test-2026?"),
            "{text:?}"
        ); // WHAT: the name on one line, as typed, with nothing hidden. WHY: the same.
        let long = plugin("x", &"W".repeat(5000), "1.0.0", false); // WHAT: a name five thousand characters long. WHY: one that would fill the window.
        let (text, _) = plugin_question(&long, &Fits::New, Rule::Here); // WHAT: the question. WHY: what the person reads.
        assert!(
            text.chars().count() < 200 && text.contains('\u{2026}'),
            "{}",
            text.len()
        ); // WHAT: cut short, with an ellipsis. WHY: the question stays on screen.
    }

    fn chest_dir(name: &str) -> std::path::PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/hv-arrivals")
            .join(format!("{name}-{}", std::process::id())); // WHAT: a folder of this build's own. WHY: a shared temporary folder is anybody's.
        let _ = std::fs::remove_dir_all(&dir); // WHAT: emptied. WHY: a run before must not decide this one.
        std::fs::create_dir_all(&dir).unwrap(); // WHAT: made. WHY: the chest is written into it.
        dir // WHAT: the folder. WHY: the test's.
    }

    fn a_chest(dir: &Path, file: &str, title: &str, tools: usize) -> std::path::PathBuf {
        let mut profile =
            chest::Profile::read(b"<RevuProfile Name=\"Steel\"></RevuProfile>").unwrap(); // WHAT: an empty profile. WHY: filled below.
        profile.name = "Steel takeoff".into(); // WHAT: its name. WHY: the window says it.
        profile.sets = vec![chest::ToolSet {
            title: title.into(),
            tools: (0..tools)
                .map(|i| chest::toolset::plain(&format!("{title} {i}"), chest::Kind::Length))
                .collect(),
        }]; // WHAT: one set of tools. WHY: the count is said.
        let path = dir.join(file); // WHAT: where the arriving file is. WHY: read from there.
        std::fs::write(&path, chest::native::write(&profile, "")).unwrap(); // WHAT: written as Excalibur View's own. WHY: the usual file.
        path // WHAT: the file. WHY: the test reads it.
    }

    #[test]
    fn a_chest_is_kept_at_once_and_the_same_one_again_asks_nothing() {
        let (from, kept) = (chest_dir("from-a"), chest_dir("kept-a")); // WHAT: where it arrives from, and where chests are kept. WHY: two folders, as on a real seat.
        let offer = read_chest(&a_chest(&from, "Steel.evtools", "Beams", 42)).unwrap(); // WHAT: read. WHY: the case.
        assert_eq!(
            (offer.name.as_str(), offer.kept_as.as_str(), offer.tools),
            ("Steel takeoff", "Steel.evtools", 42)
        ); // WHAT: its name, the file it is kept as, its tools. WHY: what the window says.
        assert_eq!(chest_plan(&kept, &offer), ChestPlan::New); // WHAT: nothing by that name. WHY: kept without asking.
        assert!(keep_chest(&kept, &offer)); // WHAT: kept. WHY: the same.
        assert_eq!(chest_plan(&kept, &offer), ChestPlan::Same); // WHAT: the very same again. WHY: nothing to ask and nothing to change.
        let _ = (
            std::fs::remove_dir_all(&from),
            std::fs::remove_dir_all(&kept),
        ); // WHAT: tidied. WHY: the folders were the test's.
    }

    #[test]
    fn a_different_chest_by_the_same_name_is_replaced_only_after_asking() {
        let (from, kept) = (chest_dir("from-b"), chest_dir("kept-b")); // WHAT: the two folders. WHY: as above.
        let first = read_chest(&a_chest(&from, "Steel.evtools", "Beams", 3)).unwrap(); // WHAT: the chest already here. WHY: the case.
        assert!(keep_chest(&kept, &first)); // WHAT: kept. WHY: the same.
        let second = read_chest(&a_chest(&from, "Steel.evtools", "Beams", 5)).unwrap(); // WHAT: a different one with the same file name. WHY: what a newer copy from another program looks like.
        assert_eq!(chest_plan(&kept, &second), ChestPlan::Differs); // WHAT: differs. WHY: asked, not overwritten.
        let mut arrivals = Arrivals::default(); // WHAT: the window's state. WHY: the question goes in it.
        arrivals.asking.push_back(Question::Chest(Box::new(second))); // WHAT: asked. WHY: as take_chest does.
        assert!(arrivals.asking_about_chest("steel.EVTOOLS")); // WHAT: found in any case. WHY: the same file to Windows, asked about once.
        assert_eq!(
            std::fs::read(kept.join("Steel.evtools")).unwrap(),
            first.bytes
        ); // WHAT: what is kept is still the first. WHY: nothing is replaced before the answer.
        let _ = (
            std::fs::remove_dir_all(&from),
            std::fs::remove_dir_all(&kept),
        ); // WHAT: tidied. WHY: the folders were the test's.
    }

    #[test]
    fn what_happened_stays_a_short_list() {
        let mut arrivals = Arrivals::default(); // WHAT: the window's state. WHY: the case.
        for i in 0..100 {
            // WHAT: a hundred lines. WHY: a hundred files dropped at once.
            arrivals.say(format!("line {i}")); // WHAT: each said. WHY: the same.
        }
        assert_eq!(arrivals.said.len(), MOST_SAID); // WHAT: no more than the limit. WHY: the window stays on screen.
        assert_eq!(arrivals.said.last().map(String::as_str), Some("line 99")); // WHAT: the newest kept. WHY: the ones being looked at.
    }

    #[test]
    fn a_chest_from_outside_is_asked_about_and_one_chosen_from_the_menu_is_not() {
        assert_eq!(chest_step(ChestPlan::New, true), ChestStep::Ask); // WHAT: opened from outside, nothing by that name. WHY: it can take over this seat's tools, so it is asked about once.
        assert_eq!(chest_step(ChestPlan::New, false), ChestStep::KeepNow); // WHAT: chosen with Load Tool Chest. WHY: the person just picked it.
        assert_eq!(chest_step(ChestPlan::Differs, false), ChestStep::Ask); // WHAT: replacing one, from the menu. WHY: replacing is always asked about.
        assert_eq!(chest_step(ChestPlan::Differs, true), ChestStep::Ask); // WHAT: replacing one, from outside. WHY: the same.
    }

    #[test]
    fn a_chest_says_which_of_your_tool_sets_it_takes_over_before_the_yes() {
        let from = chest_dir("from-c"); // WHAT: where it arrives from. WHY: as on a real seat.
        let mut offer = read_chest(&a_chest(&from, "Other.evtools", "Beams", 2)).unwrap(); // WHAT: a chest with a set called Beams. WHY: the case.
        assert!(!offer.layout, "a chest with no toolbars brings no layout"); // WHAT: no toolbars. WHY: said only when it does.
        let mut here = offer.profile.clone(); // WHAT: this seat's tools. WHY: matched against.
        here.sets[0].tools.clear(); // WHAT: its own Beams, different. WHY: what would be taken over.
        here.sets.push(chest::ToolSet {
            title: "Columns".into(),
            tools: Vec::new(),
        }); // WHAT: a set the chest doesn't have. WHY: not named.
        offer.shadows = shadows(&offer.profile, Some(&here)); // WHAT: what it takes over. WHY: as take_chest works it out.
        assert_eq!(offer.shadows, vec!["Beams".to_string()]); // WHAT: Beams, and only Beams. WHY: the seat uses the newest chest's set of each name.
        let (text, yes, no) = chest_question(&offer); // WHAT: the question. WHY: what the person reads.
        assert!(
            text.starts_with("Add the tool chest 'Steel takeoff' (2 tools)?"),
            "{text}"
        ); // WHAT: the plain question. WHY: a chest from outside is asked about.
        assert!(
            text.contains("take the place of yours with the same names: 'Beams'."),
            "{text}"
        ); // WHAT: which sets. WHY: so nobody's Beams changes without them knowing.
        assert_eq!((yes, no), ("Add", "Not now")); // WHAT: the buttons. WHY: say what they do.
        offer.layout = true; // WHAT: one that brings toolbars. WHY: said too.
        offer.replacing = true; // WHAT: and replaces one by that name. WHY: asked in those words.
        let (text, yes, _) = chest_question(&offer); // WHAT: the question. WHY: the same.
        assert!(
            text.starts_with("A tool chest called 'Steel takeoff' is already here."),
            "{text}"
        ); // WHAT: the replace question. WHY: the same.
        assert!(
            text.ends_with("It also sets your toolbars and the markups list's columns."),
            "{text}"
        ); // WHAT: the layout said. WHY: the same.
        assert_eq!(yes, "Replace"); // WHAT: the button. WHY: says what it does.
        let many: Vec<String> = (0..6).map(|i| format!("Set {i}")).collect(); // WHAT: six names. WHY: the question names three and counts the rest.
        assert_eq!(some_names(&many), "'Set 0', 'Set 1', 'Set 2' and 3 more"); // WHAT: cut short. WHY: the buttons stay on screen.
        let _ = std::fs::remove_dir_all(&from); // WHAT: tidied. WHY: the folder was the test's.
    }

    #[test]
    fn a_chest_too_big_or_with_too_many_tools_or_not_a_file_is_not_read() {
        let from = chest_dir("from-d"); // WHAT: where it arrives from. WHY: as on a real seat.
        let path = a_chest(&from, "Steel.evtools", "Beams", 5); // WHAT: a chest of five tools. WHY: the limits below are set around it.
        let size = std::fs::metadata(&path).unwrap().len(); // WHAT: its size. WHY: the limit is set just under it.
        assert!(read_chest_within(&path, size, 5).is_ok()); // WHAT: at both limits. WHY: allowed.
        let why = read_chest_within(&path, size - 1, 5).err().unwrap(); // WHAT: a byte too big. WHY: refused before it is read.
        assert!(why.contains("bigger than any tool chest"), "{why}"); // WHAT: says why. WHY: plain.
        let why = read_chest_within(&path, size, 4).err().unwrap(); // WHAT: a tool too many. WHY: refused.
        assert!(why.contains("more than the 4"), "{why}"); // WHAT: says why. WHY: plain.
        let why = read_chest_within(&from, size, 5).err().unwrap(); // WHAT: a folder named like a chest. WHY: not a file.
        assert!(why.contains("not a tool chest file"), "{why}"); // WHAT: says why. WHY: plain.
        let big = from.join("Huge.evtools"); // WHAT: a file far past the real limit. WHY: refused by its size, without reading it.
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MOST_CHEST_BYTES + 1)
            .unwrap(); // WHAT: sparse, so it costs nothing. WHY: the same.
        assert!(read_chest(&big)
            .err()
            .unwrap()
            .contains("bigger than any tool chest")); // WHAT: refused. WHY: the same.
        let _ = std::fs::remove_dir_all(&from); // WHAT: tidied. WHY: the folder was the test's.
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_named_like_a_chest_is_refused_without_waiting_on_it() {
        let from = chest_dir("from-e"); // WHAT: where it arrives from. WHY: as on a real seat.
        let pipe = from.join("Pipe.evtools"); // WHAT: a named pipe. WHY: opening one waits forever for a writer, which would freeze the window.
        assert!(std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap()
            .success()); // WHAT: made. WHY: the case.
        assert!(read_chest(&pipe)
            .err()
            .unwrap()
            .contains("not a tool chest file")); // WHAT: refused at once. WHY: never opened.
        let _ = std::fs::remove_dir_all(&from); // WHAT: tidied. WHY: the folder was the test's.
    }

    #[test]
    fn many_files_at_once_ask_no_more_than_the_limit() {
        let mut arrivals = Arrivals::default(); // WHAT: the window's state. WHY: the case.
        let from = chest_dir("from-f"); // WHAT: a folder. WHY: one chest, asked about many times.
        let offer = || read_chest(&a_chest(&from, "Steel.evtools", "Beams", 1)).unwrap(); // WHAT: a chest. WHY: something to queue.
        for _ in 0..MOST_ASKING {
            // WHAT: up to the limit. WHY: each has room.
            assert!(arrivals.room()); // WHAT: room. WHY: under the limit.
            arrivals
                .asking
                .push_back(Question::Chest(Box::new(offer()))); // WHAT: queued. WHY: as take_chest does.
        }
        for _ in 0..1000 {
            // WHAT: a thousand more. WHY: a flood of files.
            assert!(!arrivals.room()); // WHAT: no room. WHY: never read.
        }
        assert_eq!(
            (arrivals.asking.len(), arrivals.left_out),
            (MOST_ASKING, 1000)
        ); // WHAT: eight asked, a thousand counted. WHY: one line says how many were left out.
        let _ = std::fs::remove_dir_all(&from); // WHAT: tidied. WHY: the folder was the test's.
    }

    #[test]
    fn a_plugin_opened_from_outside_is_never_shared_with_the_office() {
        assert_eq!(rule_for(Rule::HereAndOffice, true), Rule::HereAndOffice); // WHAT: an administrator, from the menu. WHY: shared, as the menu always did.
        assert_eq!(
            rule_for(Rule::HereAndOffice, false),
            Rule::HereShareFromMenu
        ); // WHAT: an administrator, double-clicking. WHY: this computer only; sharing is chosen from the menu.
        for rule in [Rule::Here, Rule::HereNotOffice, Rule::NotOnThisCopy] {
            // WHAT: every other rule. WHY: unchanged from outside.
            assert_eq!(rule_for(rule, false), rule); // WHAT: the same. WHY: none of them shares.
        }
        let incoming = plugin("takeoff-check", "Takeoff Check", "1.0.0", false); // WHAT: a plugin. WHY: the question below.
        let (text, _) =
            plugin_question(&incoming, &Fits::New, rule_for(Rule::HereAndOffice, false)); // WHAT: asked of an administrator who double-clicked. WHY: the case.
        assert!(!text.contains("shared with everyone"), "{text}"); // WHAT: not shared. WHY: the same.
        assert!(text.contains("on this computer only. To share it with everyone in your office, add it with Plugins"), "{text}");
        // WHAT: how to share it, said. WHY: the administrator knows where.
    }

    #[test]
    fn the_menu_adds_a_new_plugin_at_once_asks_before_an_update_and_never_downgrades() {
        let shelf = shelf_with(vec![plugin(
            "takeoff-check",
            "Takeoff Check",
            "1.2.0",
            false,
        )]); // WHAT: 1.2.0 here. WHY: the case.
        assert!(!menu_asks(&Fits::New)); // WHAT: a new plugin. WHY: the person just chose it.
        let newer = plugin("takeoff-check", "Takeoff Check", "1.3.0", false); // WHAT: a newer one. WHY: an update.
        let fits = plugins::fits(&shelf, &newer.manifest); // WHAT: where it fits. WHY: the same.
        assert!(plugin_not_asked(&newer, &fits).is_none() && menu_asks(&fits)); // WHAT: asked about. WHY: replacing a plugin somebody uses.
        let older = plugin("takeoff-check", "Takeoff Check", "1.1.0", false); // WHAT: an older one. WHY: a downgrade.
        assert!(plugin_not_asked(&older, &plugins::fits(&shelf, &older.manifest)).is_some());
        // WHAT: turned away with a line. WHY: never a downgrade, from the menu either.
    }
}
