//! Preferences: the handful of choices that change how the program behaves,
//! kept in a file beside the program so all six seats can be set up the same
//! way, or each one differently, as the office prefers.

use std::path::PathBuf;

// A setting missing from the file takes its usual value, the rest are kept:
// a file written by an older version, or trimmed by hand, must never cost
// somebody every other setting and their sign-in.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Draw measurement captions into the file itself.
    ///
    /// Revu does not. It stores the words and paints them in its own viewer, so
    /// a drawing marked up in Revu shows the lines but not the numbers in
    /// Acrobat, in a browser, or on a general contractor's screen. Hyperview
    /// draws them in by default, because a takeoff whose numbers only one
    /// program can see is half a takeoff.
    pub captions_in_file: bool,
    /// The name that goes on a markup's Author column.
    pub author: String,
    /// Smallest fraction of an inch to report: 16 means sixteenths.
    pub denominator: u32,
    /// Scroll wheel zooms rather than scrolls.
    pub wheel_zooms: bool,
    /// Units for anything typed in, and for what gets read back.
    pub units: crate::units::Units,
    /// Which stream of releases this seat follows.
    #[serde(default)]
    pub channel: hub::update::Channel,
    /// Drawings opened lately, newest first. Paths only — a list of names with
    /// no way to open them would be decoration.
    #[serde(default)]
    pub recent: Vec<std::path::PathBuf>,
    /// Draw a rule along the top and left edge, in the sheet's own units.
    #[serde(default)]
    pub rulers: bool,
    /// Cross-hairs across the whole window, for lining a markup up with
    /// something on the other side of a sheet.
    #[serde(default)]
    pub crosshair: bool,
    /// Pull a point being drawn onto the drawing's own line-work.
    #[serde(default = "yes")]
    pub snap_to_content: bool,
    /// Pull a point being drawn onto an existing markup's corners and ends.
    #[serde(default = "yes")]
    pub snap_to_markup: bool,
    /// Show each markup's text in a label over the sheet. Off unless somebody
    /// turns it on: a drawing set out of Revu carries hundreds of markups
    /// whose text is already drawn by the markup itself, or is a note nobody
    /// needs spread across the sheet. A selected markup always shows its own.
    #[serde(default)]
    pub markup_text: bool,
    /// Draw a grid over the sheet.
    #[serde(default)]
    pub grid: bool,
    /// Pull a point being drawn onto the nearest grid line.
    #[serde(default)]
    pub snap_to_grid: bool,
    /// How far apart the grid lines are, in the sheet's own units. Twelve
    /// inches at the sheet's scale, by default, because that is the spacing a
    /// fabricator reads a drawing in.
    #[serde(default = "one_foot")]
    pub grid_spacing: f64,
    /// The last server signed in to, and a session token for it. Never a
    /// password: Hyperview does not keep one.
    #[serde(default)]
    pub server: crate::server::Remembered,
    /// The stock length the shop buys, in the drawing's primary unit.
    ///
    /// Zero means nobody has said, and nothing is nested until somebody does.
    /// There is deliberately no default mill length in this program: twenty,
    /// forty and sixty feet are all ordinary, they differ by shape and by
    /// supplier, and a cut list that guessed would be one that quietly bought
    /// the wrong steel.
    #[serde(default)]
    pub stock_length: f64,
    /// The saw kerf, taken off once for every cut after the first on a stick.
    #[serde(default = "an_eighth")]
    pub kerf: f64,
    /// The shortest drop worth putting back in the rack.
    #[serde(default = "two_feet")]
    pub worth_keeping: f64,
    /// The increment cut lengths are rounded **up** to.
    #[serde(default = "an_inch")]
    pub cut_step: f64,
    /// How big the controls are drawn on a tablet, as a multiple of their
    /// desktop size. Nought is the tablet's own default.
    #[serde(default)]
    pub control_size: f32,
    /// The Classic look: grey, with raised buttons and sunken boxes.
    #[serde(default)]
    pub classic: bool,
    /// Simple mode: the toolbars put away and one short bar of everyday tools
    /// in their place. `None` until somebody chooses, which means on for a
    /// tablet and off for a desktop.
    #[serde(default)]
    pub simple: Option<bool>, // WHAT: simple mode, or not yet chosen. WHY: a first run on a tablet starts simple; a choice made is kept.
    /// The quick tools down the side, in order, by command. `None` until
    /// somebody chooses, which means the usual six.
    #[serde(default)]
    pub quick_tools: Option<Vec<String>>, // WHAT: this computer's quick tools. WHY: each person keeps the few they use all day.
    /// Whether the quick tools are shown.
    #[serde(default = "yes")]
    pub quick_tools_shown: bool, // WHAT: the side bar on or off. WHY: somebody who never uses it can put it away.
    /// The printer chosen last, by name, on a computer that prints through
    /// CUPS (Linux and the Mac). Empty for the system's default.
    #[serde(default)]
    pub printer: String, // WHAT: the printer last printed to. WHY: the next print goes there without asking again.
    /// The paper chosen last on that printer, by CUPS's own name for it.
    #[serde(default)]
    pub paper: String, // WHAT: the paper last printed on. WHY: kept with the printer, so a plot goes on the roll it went on before.
    /// The shop-and-field layout on this computer even for the office (docs/TABLET-MANIFEST.md).
    #[serde(default)]
    pub tablet_mode: bool, // WHAT: "Tablet mode" turned on here. WHY: a desktop in the shop shows the tablet's screens; iPads, Android and shop or field accounts get them anyway
    /// The bright outdoor look on the tablet screens.
    #[serde(default)]
    pub tablet_bright: bool, // WHAT: the light look. WHY: one button in the top bar; kept so a field tablet stays bright
    /// The released sheets this person has opened, by set, for the NEW badge.
    #[serde(default)]
    pub tablet_seen: Vec<String>, // WHAT: sets opened on this tablet. WHY: a sheet shows NEW until it is opened here
}

/// The sizes a tablet's controls can be drawn at, with their names.
pub const CONTROL_SIZES: [(f32, &str); 3] = [(1.0, "Standard"), (1.15, "Larger"), (1.3, "Largest")];

impl Prefs {
    /// How big a tablet draws its controls. A point on an Android tablet is a
    /// 160th of an inch and on an iPad a 132nd, against about a 96th on a
    /// desktop screen, so the same controls come out smaller under a finger
    /// than they are under a mouse; they start a size up to make up for it.
    pub fn control_scale(&self) -> f32 {
        if self.control_size > 0.0 {
            return self.control_size.clamp(0.8, 1.6);
        }
        if cfg!(target_os = "android") {
            1.15
        } else if crate::platform::tablet() {
            1.1
        } else {
            1.0
        }
    }
}

fn yes() -> bool {
    true
}

fn one_foot() -> f64 {
    12.0
}

fn an_inch() -> f64 {
    takeoff::shoplist::INCH
}

fn an_eighth() -> f64 {
    takeoff::shoplist::INCH / 8.0
}

fn two_feet() -> f64 {
    2.0
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            captions_in_file: true,
            author: crate::app::whoami(),
            denominator: 16,
            wheel_zooms: true,
            units: crate::units::Units::FeetInches,
            channel: hub::update::Channel::Stable,
            server: Default::default(),
            recent: Vec::new(),
            rulers: false,
            crosshair: false,
            markup_text: false,
            snap_to_content: true,
            snap_to_markup: true,
            grid: false,
            snap_to_grid: false,
            grid_spacing: one_foot(),
            stock_length: 0.0,
            kerf: an_eighth(),
            worth_keeping: two_feet(),
            cut_step: an_inch(),
            control_size: 0.0,
            classic: false,
            simple: None, // WHAT: not chosen yet. WHY: the device decides until somebody does.
            quick_tools: None, // WHAT: not chosen yet. WHY: the usual six until somebody picks their own.
            quick_tools_shown: true, // WHAT: the side bar shown. WHY: it is how somebody finds out it is there.
            printer: String::new(), // WHAT: no printer chosen. WHY: the system's default until one is.
            paper: String::new(), // WHAT: no paper chosen. WHY: the printer's own until one is.
            tablet_mode: false, // WHAT: off. WHY: the office's desktop keeps its folder tree unless asked
            tablet_bright: false, // WHAT: dark. WHY: the default indoors
            tablet_seen: Vec::new(), // WHAT: nothing opened yet. WHY: every released sheet starts NEW
        }
    }
}

impl Prefs {
    /// Whether simple mode is on: as chosen, or on a first run, on for a
    /// tablet and off for a desktop.
    pub fn simple_on(&self, tablet: bool) -> bool {
        self.simple.unwrap_or(tablet) // WHAT: the choice, or the device's default. WHY: a five-row toolbar on a tablet leaves little room for the drawing.
    }
}

/// How many drawings to remember. Long enough to cover a week, short enough
/// that the menu is still a list rather than a history.
pub const REMEMBER: usize = 12;

impl Prefs {
    /// Puts a drawing at the top of the recent list.
    ///
    /// Deduplicated on the way in, because opening the same set four times in a
    /// morning should not fill the menu with it.
    pub fn opened(&mut self, path: &std::path::Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(REMEMBER);
    }

    /// The ones that are still there. A drawing somebody has moved or deleted
    /// should drop off the menu rather than sit in it failing to open.
    pub fn recent_that_exist(&self) -> Vec<std::path::PathBuf> {
        self.recent.iter().filter(|p| p.exists()).cloned().collect()
    }

    /// How this shop buys and cuts, for the cut list.
    pub fn buying(&self) -> takeoff::shoplist::Buying {
        takeoff::shoplist::Buying {
            step: if self.cut_step > 0.0 { self.cut_step } else { an_inch() },
            stock: self.stock_length.max(0.0),
            kerf: self.kerf.max(0.0),
            worth_keeping: self.worth_keeping.max(0.0),
        }
    }

    pub fn drawing(&self) -> annot::appearance::Draw {
        annot::appearance::Draw {
            captions: self.captions_in_file,
        }
    }

    /// Where the settings live: beside the program if that is writable, which
    /// is how a shared install keeps the whole office consistent, otherwise in
    /// this user's own application data.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn path() -> Option<PathBuf> {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let beside = dir.join("hyperview.json");
                if beside.exists() {
                    return Some(beside);
                }
            }
        }
        Some(folder()?.join(FILE)) // WHAT: this user's settings file. WHY: none beside the program
    }

    pub fn load() -> Prefs {
        let Some(path) = Prefs::path() else {
            return Prefs::default();
        };
        if let Some(old) = earlier_folder() {
            match hub::office::move_once(&old.join(FILE), &path) {
                Ok(true) => log::info!("settings moved from {} to {}", old.display(), path.display()), // WHAT: noted. WHY: the move is visible in the log
                Ok(false) => {} // WHAT: nothing to move. WHY: a new install, moved before, or newer settings already there
                Err(e) => log::warn!("couldn't move the settings from {}: {e}", old.display()), // WHAT: said. WHY: the program still starts, with its usual settings
            }
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Prefs::default();
        };
        // A settings file somebody has hand-edited into nonsense should not
        // stop the program opening a drawing.
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let Some(path) = Prefs::path() else {
            return Err("Nowhere to keep the settings on this machine.".into());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// The settings file's name.
const FILE: &str = "hyperview.json"; // WHAT: one spelling. WHY: the same name in every place it has lived

/// The folder this user's settings (and signatures) live in, or `None` when
/// the system names no home at all.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn folder() -> Option<PathBuf> {
    folder_for(std::env::var_os("APPDATA").map(PathBuf::from), std::env::var_os("HOME").map(PathBuf::from), cfg!(target_os = "ios")) // WHAT: from this system's own variables. WHY: worked out by the rule below
}

/// Where settings live, given `%APPDATA%`, `$HOME`, and whether this is an
/// iPad or iPhone.
///
/// On iOS `$HOME` is the app's sandbox container, and Apple's File System
/// Programming Guide lets an app make folders only inside its `Documents`,
/// `Library` and `tmp` folders, with "configuration files" put in
/// `Library/Application Support`. A `.config` folder at the container's top
/// is outside all three. Android is already right: the app sets `$HOME` to
/// its own writable files folder before anything runs (`mobile/src/android.rs`).
fn folder_for(appdata: Option<PathBuf>, home: Option<PathBuf>, ios: bool) -> Option<PathBuf> {
    if ios {
        return home.map(|h| h.join("Library").join("Application Support").join("Excalibur Hyperview")); // WHAT: Library/Application Support. WHY: where Apple says an app's own settings go
    }
    appdata.or_else(|| home.map(|h| h.join(".config"))).map(|base| base.join("Excalibur Hyperview")) // WHAT: %APPDATA% on Windows, ~/.config elsewhere. WHY: unchanged for every desktop and for Android
}

/// Where an earlier build kept the settings, when that was somewhere else:
/// `$HOME/.config` on iOS, before 0.7.1.
pub fn earlier_folder() -> Option<PathBuf> {
    if !cfg!(target_os = "ios") {
        return None; // WHAT: nothing moved. WHY: only iOS changed place
    }
    folder_for(None, std::env::var_os("HOME").map(PathBuf::from), false) // WHAT: ~/.config/Excalibur Hyperview. WHY: the old rule, for the one-time move
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captions_go_into_the_file_unless_somebody_says_otherwise() {
        let p = Prefs::default();
        assert!(p.captions_in_file);
        assert!(p.drawing().captions);
    }

    #[test]
    fn turning_captions_off_writes_what_revu_writes() {
        let p = Prefs {
            captions_in_file: false,
            ..Prefs::default()
        };
        assert_eq!(p.drawing(), annot::appearance::Draw::like_revu());
    }

    #[test]
    fn a_settings_file_full_of_nonsense_falls_back_rather_than_failing() {
        let broken: Prefs = serde_json::from_str("{\"captions_in_file\": \"yes please\"}")
            .unwrap_or_default();
        assert_eq!(broken, Prefs::default());
    }

    #[test]
    fn a_kept_server_is_a_session_and_never_a_password() {
        let mut p = Prefs::default();
        p.server = crate::server::Remembered {
            base: "https://drawings.example.com".into(),
            token: "a-session-token".into(),
            name: "Example Steel".into(),
            user: "Sam".into(),
            reachable_at: None,
        };
        let text = serde_json::to_string(&p).unwrap();
        assert!(text.contains("a-session-token"));
        assert!(!text.to_lowercase().contains("\"password\""));
    }

    #[test]
    fn a_settings_file_from_before_servers_existed_still_loads() {
        // Somebody upgrading has a file with none of the newer fields in it.
        let old = r#"{"captions_in_file": true, "author": "Sam",
                      "denominator": 16, "wheel_zooms": true, "units": "FeetInches"}"#;
        let p: Prefs = serde_json::from_str(old).expect("it should still load");
        assert_eq!(p.author, "Sam");
        assert_eq!(p.server.base, "");
        assert_eq!(p.channel, hub::update::Channel::Stable);
    }

    #[test]
    fn simple_mode_starts_on_for_a_tablet_and_off_for_a_desktop_until_somebody_chooses() {
        let fresh = Prefs::default(); // WHAT: a first run. WHY: nothing chosen yet.
        assert!(fresh.simple_on(true)); // WHAT: a tablet. WHY: starts simple.
        assert!(!fresh.simple_on(false)); // WHAT: a desktop. WHY: starts with the full toolbars.
        let chosen = Prefs { simple: Some(false), ..Prefs::default() }; // WHAT: somebody turned it off. WHY: their choice beats the device's.
        assert!(!chosen.simple_on(true)); // WHAT: off on a tablet. WHY: as chosen.
        let chosen = Prefs { simple: Some(true), ..Prefs::default() }; // WHAT: somebody turned it on. WHY: the same the other way.
        assert!(chosen.simple_on(false)); // WHAT: on on a desktop. WHY: as chosen.
    }

    #[test]
    fn a_settings_file_from_before_simple_mode_keeps_everything_and_shows_the_quick_tools() {
        let old = r#"{"author": "Sam", "classic": true}"#; // WHAT: a 0.6.23 file. WHY: none of the new fields in it.
        let p: Prefs = serde_json::from_str(old).expect("it should still load"); // WHAT: read it. WHY: an upgrade must keep the settings.
        assert!(p.classic); // WHAT: an old setting kept. WHY: nothing lost.
        assert_eq!(p.simple, None); // WHAT: simple mode not chosen. WHY: so the device decides.
        assert_eq!(p.quick_tools, None); // WHAT: no quick tools chosen. WHY: the usual six.
        assert!(p.quick_tools_shown); // WHAT: shown. WHY: the default, the same as a fresh install.
        assert!(p.printer.is_empty()); // WHAT: no printer kept. WHY: the default printer until one is chosen.
    }

    #[test]
    fn an_ipad_keeps_its_settings_in_application_support_and_desktops_are_unchanged() {
        let home = PathBuf::from("/var/mobile/Containers/Data/Application/APP"); // WHAT: an iOS app's container, as $HOME. WHY: the iPad case
        assert_eq!(folder_for(None, Some(home.clone()), true), Some(home.join("Library/Application Support/Excalibur Hyperview"))); // WHAT: Library/Application Support. WHY: an app may only make folders in Documents, Library and tmp
        assert_eq!(folder_for(None, Some(PathBuf::from("/home/sam")), false), Some(PathBuf::from("/home/sam/.config/Excalibur Hyperview"))); // WHAT: Linux, and Android (whose $HOME is its files folder). WHY: unchanged
        assert_eq!(folder_for(Some(PathBuf::from(r"C:\Users\sam\AppData\Roaming")), Some(PathBuf::from("/x")), false), Some(PathBuf::from(r"C:\Users\sam\AppData\Roaming").join("Excalibur Hyperview"))); // WHAT: Windows. WHY: unchanged
        assert_eq!(folder_for(None, None, true), None); // WHAT: no home at all. WHY: nowhere, rather than a guess
    }

    #[test]
    fn settings_kept_by_an_earlier_ipad_build_are_moved_once_and_never_over_newer_ones() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()); // WHAT: a fresh name. WHY: tests run side by side
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("prefs-move-{n}-{}", std::process::id())); // WHAT: a folder under the build's own. WHY: never a shared temporary folder
        let old = folder_for(None, Some(base.clone()), false).unwrap().join(FILE); // WHAT: where 0.7.0 kept it on iOS. WHY: the old place, by the old rule
        let new = folder_for(None, Some(base.clone()), true).unwrap().join(FILE); // WHAT: where it goes now. WHY: the new place, by the iOS rule
        std::fs::create_dir_all(old.parent().unwrap()).unwrap(); // WHAT: the old folder. WHY: an earlier build made it
        std::fs::write(&old, r#"{"server":{"base":"https://office.example.com","token":"kept","name":"Example Steel","user":"Sam"}}"#).unwrap(); // WHAT: an old file with a sign-in. WHY: what must survive
        std::fs::create_dir_all(new.parent().unwrap()).unwrap(); // WHAT: the new folder. WHY: newer settings go in it first
        std::fs::write(&new, "{}").unwrap(); // WHAT: newer settings in the new place. WHY: must never be overwritten
        assert!(!hub::office::move_once(&old, &new).unwrap(), "newer settings win"); // WHAT: not moved. WHY: never over newer
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "{}"); // WHAT: unchanged. WHY: likewise
        std::fs::remove_file(&new).unwrap(); // WHAT: the new place empty. WHY: the move itself, next
        assert!(hub::office::move_once(&old, &new).unwrap(), "moved"); // WHAT: moved. WHY: nothing in the new place
        let p: Prefs = serde_json::from_str(&std::fs::read_to_string(&new).unwrap()).unwrap(); // WHAT: read back. WHY: the sign-in came across
        assert_eq!((p.server.base.as_str(), p.server.token.as_str()), ("https://office.example.com", "kept")); // WHAT: the same server and token. WHY: an upgrade keeps them
        assert!(!old.exists(), "no second copy of the sign-in left behind"); // WHAT: removed. WHY: security review L3
        let _ = std::fs::remove_dir_all(&base); // WHAT: tidied. WHY: a test leaves nothing
    }

    #[test]
    fn settings_survive_a_round_trip() {
        let mut p = Prefs::default();
        p.denominator = 8;
        p.captions_in_file = false;
        p.author = "Sam".into();
        let text = serde_json::to_string(&p).unwrap();
        let back: Prefs = serde_json::from_str(&text).unwrap();
        assert_eq!(p, back);
    }
}

#[cfg(test)]
mod recent_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_newest_drawing_is_at_the_top() {
        let mut prefs = Prefs::default();
        prefs.opened(&PathBuf::from("/a.pdf"));
        prefs.opened(&PathBuf::from("/b.pdf"));
        assert_eq!(prefs.recent[0], PathBuf::from("/b.pdf"));
        assert_eq!(prefs.recent[1], PathBuf::from("/a.pdf"));
    }

    #[test]
    fn opening_the_same_set_again_moves_it_up_rather_than_listing_it_twice() {
        let mut prefs = Prefs::default();
        prefs.opened(&PathBuf::from("/a.pdf"));
        prefs.opened(&PathBuf::from("/b.pdf"));
        prefs.opened(&PathBuf::from("/a.pdf"));
        assert_eq!(prefs.recent.len(), 2);
        assert_eq!(prefs.recent[0], PathBuf::from("/a.pdf"));
    }

    #[test]
    fn the_list_does_not_grow_for_ever() {
        let mut prefs = Prefs::default();
        for n in 0..40 {
            prefs.opened(&PathBuf::from(format!("/{n}.pdf")));
        }
        assert_eq!(prefs.recent.len(), REMEMBER);
        assert_eq!(prefs.recent[0], PathBuf::from("/39.pdf"));
    }

    #[test]
    fn a_drawing_that_has_been_moved_drops_off_the_menu() {
        let mut prefs = Prefs::default();
        prefs.opened(&PathBuf::from("/no/such/drawing.pdf"));
        assert_eq!(prefs.recent.len(), 1, "it is still remembered");
        assert!(
            prefs.recent_that_exist().is_empty(),
            "but it is not offered"
        );
    }

    #[test]
    fn snapping_is_on_for_somebody_who_has_never_chosen() {
        // Drawing a takeoff line that does not land on the drawing's own
        // line-work is the commonest way a measurement comes out wrong.
        let prefs = Prefs::default();
        assert!(prefs.snap_to_content);
        assert!(prefs.snap_to_markup);
    }

    #[test]
    fn a_settings_file_missing_settings_keeps_the_ones_it_has() {
        let text = r#"{ "server": { "base": "http://shop:8714", "token": "t", "name": "Example Steel", "user": "sam" },
                        "author": "Sam" }"#;
        let prefs: Prefs = serde_json::from_str(text).expect("a partial file still reads");
        assert_eq!(prefs.server.base, "http://shop:8714", "the sign-in survives");
        assert_eq!(prefs.author, "Sam");
        assert_eq!(prefs.denominator, Prefs::default().denominator, "the rest take their usual values");
    }
}
