//! The window's side of the server: signing in, browsing what is on it, and
//! being told when there is a new version.

use egui::{Color32, RichText};

use crate::app::App;
use crate::updatebox;
use crate::server::{Ask, Standing, Told};

/// The purpose the server gives a sync key.
pub const SYNC_PURPOSE: &str = "sync"; // WHAT: the word the server's POST /api/v1/me/keys takes. WHY: one spelling, used by the button and the label alike.

/// What a sync key can and cannot do, in plain words, shown when one is made
/// and on its button.
pub const SYNC_KEY_WORDS: &str = "A sync key is for a program that only keeps its own projects in step with this server. \
It can file projects under that program's own reference, put drawing sets up, archive them and read the markups back. \
It cannot see projects somebody made by hand, and it cannot add people, change roles, make or take back keys, \
turn on remote access, update the server, or change the license. It works only while the person who made it is \
an administrator. Take it back here when the program no longer needs it."; // WHAT: the sync key's powers and limits. WHY: whoever sets the program up should know what they are handing it.

/// A key's purpose as it is shown in the list of keys.
pub fn key_kind_words(purpose: &str) -> &'static str {
    match purpose {
        "assistant" => "assistant", // WHAT: an assistant key. WHY: reads the takeoffs for the assistant app, nothing else.
        SYNC_PURPOSE => "program, sync only", // WHAT: a sync key. WHY: tells it apart from a full program key when deciding which to take back.
        _ => "program", // WHAT: an integration key. WHY: the API as the person who made it.
    }
}

/// Where somebody is up to in getting connected.
///
/// There are four ways into a Hyperview server and a person should only ever
/// see the one that applies to them. Which one that is depends on facts the
/// server can be asked for, so it is asked rather than the person being made to
/// choose from a menu of things they have no way to judge.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Step {
    /// Picking a server: what answered on the network, or an address typed in.
    #[default]
    Choosing,
    /// It belongs to somebody and they have an account on it.
    SigningIn,
    /// Nobody has set it up. They are about to become its administrator.
    SettingUp,
    /// It belongs to somebody and is taking new accounts.
    Joining,
    /// They have just set one up. Here is what to tell everybody else.
    JustSetUp,
}

/// What is being typed into the connect window.
#[derive(Clone, Debug, Default)]
pub struct SigningIn {
    pub step: Step,
    pub base: String,
    pub email: String,
    pub password: String,
    /// Their own name. It goes in the Author column of every markup they make,
    /// which is why it is asked for here rather than guessed from an email
    /// address.
    pub person: String,
    /// What to call the installation, when they are the one setting it up.
    pub company: String,
    /// The join code, when they are joining somebody else's.
    pub code: String,
    /// What the server calls itself, once the address has been checked. The
    /// name only: this is what the seat remembers the server as.
    pub found: Option<String>,
    /// Which version it said it runs, shown beside the name while signing in
    /// and never saved with it. Saved together, the version a seat first met
    /// a server on was shown for good, however many times it updated since.
    pub found_version: Option<String>,
    /// What that check said about it.
    pub claimed: bool,
    pub joining: String,
    /// What answered on the network.
    pub servers: Vec<hub::discover::Found>,
    pub looking: bool,
    /// Nothing answered, so say so rather than leaving an empty box.
    pub looked: bool,
    pub error: Option<String>,
    pub waiting: bool,
    /// Set once a server has been claimed: the code to hand round, and what
    /// hosting it on this computer actually got them.
    pub join_code: String,
    pub standing: Option<String>,
    /// Whatever whoever built the installer left on the window. Somewhere to
    /// put "ask Lee in the office for a login" rather than leaving somebody
    /// stuck with nobody to ask.
    pub joined_note: String,
}

/// The server panel's forms.
#[derive(Default)]
pub struct OfficePanel {
    /// A project being started: (job number, name).
    pub new_project: Option<(String, String)>,
    pub joining: Option<hub::Joining>,
    pub updates: Option<hub::UpdateSettings>,
    pub fleet: Option<hub::FleetAccess>,
    /// What came of letting Fleet in, and the key when it has to be handed
    /// on by hand.
    pub fleet_note: Option<String>,
    pub fleet_key: Option<String>,
    pub fleet_address: String,
    /// What the server last said about reaching it from a jobsite.
    pub remote: Option<hub::Remote>,
    /// The two boxes: the token is cleared the moment it is sent.
    pub remote_token: String,
    pub remote_hostname: String,
    pub remote_note: Option<String>,
    /// Asked for once, the first time the Office section is opened.
    pub asked: bool,
    pub current_password: String,
    pub new_password: String,
    pub password_note: Option<String>,
    /// Keys for other programs, and a new one's name as it is typed.
    pub keys: Vec<hub::ApiKey>,
    pub key_name: String,
    /// Where the office tells other programs a takeoff changed, the address
    /// being typed, one just made (its secret shown once), and how a test went.
    pub notices: Vec<hub::ChangeNotice>,
    pub notice_url: String,
    pub new_notice: Option<hub::ChangeNotice>,
    pub notice_note: Option<String>,
    /// A key just made: shown until somebody copies it and closes it.
    pub new_key: Option<hub::ApiKey>,
    /// The server's address, for whoever is setting another program up.
    pub base: String,
    /// A license being pasted rather than picked off the disk. `None` until
    /// somebody asks for the box; the text they paste while it is open.
    pub pasting_license: Option<String>,
    /// Licenses found lying about — in Downloads, on the desktop, on the USB
    /// stick somebody carried one over on. Looked for once, not every frame.
    pub licenses_about: Option<Vec<FoundLicense>>,
    /// Somebody being given an account: name, email, password, role.
    pub new_person: Option<NewPerson>,
    /// Somebody about to be removed, by id. Removing an account is not a
    /// thing that should happen on one click of a small button.
    pub removing: Option<String>,
    /// Asked for once when the Office section is first opened; after that it
    /// comes back with every change.
    pub people_asked: bool,
    /// When this panel last asked the server what is on it.
    ///
    /// The list used to be asked for once, at sign-in, and never again. That
    /// is fine for the seat that made the project and wrong for everybody
    /// else: FabWire pushes a bid's drawings straight onto the server, and a
    /// seat with this panel open went on saying "Nothing on this server yet"
    /// until somebody quit the program. What you are looking at should be
    /// what is there.
    pub last_looked: Option<std::time::Instant>,
    /// The groups of people, and a new one's name as it is typed.
    pub groups: Vec<hub::Group>, // WHAT: every group and who is in it. WHY: Studio, Office, People ticks people into them.
    pub group_name: String, // WHAT: the name being typed. WHY: Make group.
    /// The project being looked at's folders and its files that aren't drawings.
    pub folders: Vec<hub::FolderInfo>, // WHAT: every folder this person sees, empty ones too. WHY: shown.
    pub files: Vec<hub::ProjectFile>, // WHAT: the files in them. WHY: shown after the drawings.
    pub folders_for: String, // WHAT: whose folders those are. WHY: a late answer for another project is dropped.
    /// A folder whose rule is being changed: (folder, the rule as it is being picked).
    pub changing_rule: Option<(String, hub::See)>, // WHAT: the folder and the rule being picked. WHY: changed only when Save is pressed.
}

/// An account being made for somebody, as it is being typed.
#[derive(Default)]
pub struct NewPerson {
    pub name: String,
    pub email: String,
    pub password: String,
    pub role: Option<hub::Role>,
}

/// A license file the program noticed, and who it is for.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundLicense {
    pub path: std::path::PathBuf,
    pub company: String,
    /// Where it turned up, in words: "Downloads", "the desktop", "E:".
    pub where_from: String,
}

/// What somebody did in the server panel this frame, done once the panel has
/// been drawn.
#[derive(Default)]
struct PanelActions {
    new_project: Option<(String, String)>,
    upload_to: Option<String>,
    share_chest: bool,
    office: bool,
    new_code: bool,
    /// How many days the join code should last, or 0 for forever.
    code_days: Option<u32>,
    channel: Option<String>,
    password: Option<(String, String)>,
    watch_fleet: bool,
    stop_fleet: bool,
    /// (token, hostname) -- turning remote access on.
    remote_on: Option<(String, String)>,
    remote_off: bool,
    /// Ask the server how the tunnel is doing.
    remote_look: bool,
    make_key: Option<String>,
    /// Make a sync key with this name.
    make_sync_key: Option<String>, // WHAT: Make a sync key pressed, with the name typed. WHY: acted on after the panel is drawn.
    revoke_key: Option<String>,
    add_notice: Option<String>,
    test_notice: Option<String>,
    remove_notice: Option<String>,
    add_license: bool,
    paste_license: bool,
    send_license: Option<String>,
    use_found_license: Option<std::path::PathBuf>,
    /// Ask the server what is on it again.
    look_again: bool,
    /// Ask who has an account here.
    people: bool,
    add_person: Option<(String, String, String, hub::Role)>,
    change_role: Option<(String, hub::Role)>,
    /// (id, name) -- the name so the sentence afterwards can use it, since
    /// the row it came from is gone by then.
    remove_person: Option<(String, String)>,
    /// (file id, name): a file to open with its own program.
    open_file: Option<(String, String)>, // WHAT: Open with its program pressed. WHY: the copy is fetched on the worker.
    /// (folder, rule): who sees a folder, changed by an administrator.
    folder_rule: Option<(String, hub::See)>, // WHAT: Save pressed on a folder's rule. WHY: sent to the server.
    make_group: Option<String>, // WHAT: Make group pressed, with the name. WHY: sent.
    /// (group, person, in): a tick in a group changed.
    change_group: Option<(String, String, bool)>, // WHAT: somebody ticked in or out. WHY: sent.
}

impl App {
    /// Everything the server thread has said since the last frame.
    pub fn drain_server(&mut self) {
        // Everything is taken off the channel first, so handling one message
        // is free to touch the whole app without still holding the link.
        let waiting: Vec<Told> = match self.link.as_ref() {
            Some(link) => link.from.try_iter().collect(),
            None => return,
        };
        for told in waiting {
            match told {
                Told::Found {
                    base,
                    name,
                    version,
                    claimed,
                    joining,
                    reachable_at,
                } => {
                    // Learned here, kept when they sign in. A seat only ever
                    // finds out about the outside address while it is on the
                    // inside, which is the right way round: nobody types it.
                    self.standing.reachable_at = reachable_at;
                    if let Some(signing) = self.signing_in.as_mut() {
                        signing.waiting = false;
                        signing.error = None;
                        signing.found = Some(name.clone());
                        signing.found_version = Some(version);
                        signing.base = base;
                        signing.claimed = claimed;
                        signing.joining = joining;
                        // Where to go next is the server's answer, not a
                        // question for somebody who has no way to know it. A
                        // server nobody has claimed wants setting up; one that
                        // is somebody's wants signing in to.
                        if signing.step == Step::Choosing {
                            signing.step = if claimed {
                                Step::SigningIn
                            } else {
                                if signing.company.trim().is_empty() {
                                    signing.company = name.clone();
                                }
                                Step::SettingUp
                            };
                        }
                    }
                }
                Told::Servers(list) => {
                    if let Some(signing) = self.signing_in.as_mut() {
                        signing.looking = false;
                        signing.looked = true;
                        signing.servers = list;
                    }
                }
                Told::Claimed {
                    base,
                    company,
                    join_code,
                } => {
                    if let Some(signing) = self.signing_in.as_mut() {
                        signing.base = base;
                        // What it is called from now on, not what it was called
                        // before anybody named it. The panel and the title bar
                        // both read this, and "New server" sitting there after
                        // somebody typed "Example Steel" is the kind of small lie
                        // that makes a program feel untrustworthy.
                        signing.found = Some(company.clone());
                        signing.found_version = None;
                        signing.company = company;
                        signing.join_code = join_code;
                        signing.step = Step::JustSetUp;
                    }
                }
                Told::SignedIn(session) => {
                    let base = self
                        .signing_in
                        .as_ref()
                        .map(|s| s.base.clone())
                        .unwrap_or_else(|| self.standing.base.clone());
                    self.standing = Standing {
                        base: base.clone(),
                        name: self
                            .signing_in
                            .as_ref()
                            .and_then(|s| s.found.clone())
                            .unwrap_or_else(|| self.standing.name.clone()),
                        who: Some(std::sync::Arc::new(session.user.clone())),
                        ..Default::default()
                    };
                    // Normally the window's work is done. Not when somebody
                    // has just set a server up: the code for everybody else is
                    // on that window, and closing it the instant they are
                    // signed in would throw it away before they read it.
                    if self.signing_in.as_ref().map(|s| s.step.clone()) != Some(Step::JustSetUp) {
                        self.signing_in = None;
                    } else if let Some(signing) = self.signing_in.as_mut() {
                        signing.waiting = false;
                        signing.password.clear();
                    }
                    self.status = format!("Signed in to {base} as {}.", session.user.name);
                    // A markup's Author column is whoever the server says this
                    // is, so six seats do not all save as "Administrator".
                    if !session.user.name.trim().is_empty() {
                        self.author = session.user.name.clone();
                    }
                    self.remember_server(&base, &session.token, &session.user.name);
                    // A license double-clicked before signing in has been
                    // waiting for this moment.
                    self.apply_waiting_license();
                    self.ask(Ask::Projects);
                    self.ask(Ask::Chests);
                    self.ask(Ask::Plugins);
                    self.ask(Ask::License);
                    self.ask(Ask::CheckForUpdate {
                        running: env!("CARGO_PKG_VERSION").to_string(),
                        channel: self.prefs.channel,
                        asked: false,
                    });
                    self.last_update_check = std::time::Instant::now();
                }
                Told::SignedOut => {
                    // What is known about updates outlives the session: a
                    // version already in place still takes over at the next
                    // start, signed in or not.
                    let kept = Standing {
                        update: self.standing.update.take(),
                        update_ready: self.standing.update_ready.take(),
                        update_refused: self.standing.update_refused.take(),
                        ..Standing::default()
                    };
                    self.standing = kept;
                    self.forget_server();
                }
                Told::Projects(list) => {
                    self.standing.busy = None;
                    self.standing.projects = list;
                }
                Told::People(list) => {
                    self.standing.busy = None;
                    self.standing.people = list;
                }
                Told::PeopleChanged { people, said } => {
                    self.standing.busy = None;
                    self.standing.people = people;
                    self.office.removing = None;
                    self.office.new_person = None;
                    self.status = said;
                }
                Told::Sets(list) => {
                    self.standing.busy = None;
                    self.standing.sets = list;
                }
                Told::Synced {
                    set,
                    revision,
                    markups,
                    sent,
                } => self.take_sync(set, revision, markups, sent),
                Told::Pushed { set, sent } => self.take_push(set, sent),
                Told::RemoteIs(remote) => {
                    self.office.remote_note = None;
                    if let Some(host) = remote.hostname.clone() {
                        self.office.remote_hostname = host;
                    }
                    self.office.remote = Some(*remote);
                }
                Told::Chests(list) => {
                    // What the office shares, every seat has: a shared chest
                    // this seat has not got is fetched now, once. Nobody has
                    // to find a "Use" button to have the office's tools.
                    let have = crate::profiles::local_digests();
                    for chest in list.iter().filter(|c| c.shared) {
                        if have.contains(&chest.digest.to_lowercase()) {
                            continue;
                        }
                        if self.chests_fetched.insert(chest.id.clone()) {
                            log::info!("fetching the office's tool chest {}", chest.name);
                            self.ask(Ask::FetchChest {
                                id: chest.id.clone(),
                                name: chest.name.clone(),
                            });
                        }
                    }
                    self.standing.chests = list;
                }
                Told::Plugins(list) => self.take_plugin_list(list),
                Told::PluginFetched { name } => {
                    self.reload_plugins();
                    match self.plugins.plugins.iter().find(|p| p.manifest.name == name) {
                        Some(_) => self.status = format!("The office's plugin '{name}' is in the Plugins menu now."),
                        None => {
                            if let Some((file, why)) = self.plugins.refused.last() {
                                log::warn!("office plugin {file} refused: {why}");
                            }
                        }
                    }
                }
                Told::PluginShared(said) => self.status = said,
                Told::PluginRan(ran) => {
                    if let Some(answer) = self.plugin_answer.take() {
                        let _ = answer.send(ran);
                    }
                }
                Told::ChestFetched { name, path } => {
                    self.status = format!("Tool chest '{name}' is on this computer now.");
                    log::info!("chest at {}", path.display());
                    // Load it now rather than making somebody restart, put
                    // together with whatever else this seat has.
                    match crate::profiles::load() {
                        Some(profile) => self.profile = Some(profile),
                        None => {
                            if let Some(profile) = std::fs::read(&path)
                                .ok()
                                .and_then(|bytes| chest::Profile::read(&bytes))
                            {
                                self.profile = Some(profile);
                            }
                        }
                    }
                }
                Told::Progress { what } => self.standing.busy = Some(what),
                Told::Downloaded { set, path, cached } => {
                    self.standing.busy = None;
                    self.status = if cached {
                        "Opening the copy already on this machine.".into()
                    } else {
                        "Downloaded. Opening…".into()
                    };
                    // Remembered so the tab knows where it came from once it
                    // has opened, and can sync its markups from then on.
                    self.opened_from_server = Some((path.clone(), set));
                    self.open(path);
                }
                Told::Takeoff(takeoff) => {
                    self.status = match takeoff.tons {
                        Some(tons) if takeoff.is_short() => format!(
                            "{tons:.3} tons — and {} measurement(s) left out for want of a scale.",
                            takeoff.left_out
                        ),
                        Some(tons) => format!("{tons:.3} tons on the server's copy."),
                        None => "Nothing on the server's copy carries a weight.".into(),
                    };
                }
                Told::Update(offer) => {
                    self.standing.pinned_to = offer.pinned_to.clone();
                    self.standing.update_looking = offer.looking;
                    // A newer offer than the one refused is a fresh chance.
                    if offer.release.as_ref().map(|r| &r.version)
                        != self.standing.update.as_ref().map(|r| &r.version)
                    {
                        self.standing.update_refused = None;
                    }
                    if offer.release.is_some() {
                        self.checking_for_update = false;
                        self.update_note = None;
                    }
                    self.standing.update = offer.release.clone();
                }
                Told::SyncFailed(_) => self.sync_asked = None,
                Told::NoticeMade(made) => {
                    self.office.notice_url.clear();
                    self.office.notice_note = None;
                    self.office.new_notice = Some(*made);
                }
                Told::NoticeTested(said) => self.office.notice_note = Some(said),
                Told::IntegrationKey(key) => {
                    self.office.key_name.clear();
                    self.office.new_key = Some(*key);
                }
                Told::AssistantConnected(words) => {
                    self.update_note = Some(words);
                }
                Told::UpdateNote(words) => {
                    self.checking_for_update = false;
                    self.update_note = Some(words);
                }
                Told::Office(office) => {
                    let office = *office;
                    self.office.joining = Some(office.joining);
                    self.office.updates = Some(office.updates);
                    self.office.fleet = Some(office.fleet);
                    self.office.keys = office.keys;
                    self.office.notices = office.notices;
                }
                Told::License { standing, added } => {
                    if added {
                        if let Some(l) = standing.as_deref() {
                            self.status = format!("License added. {}", l.about_line());
                        }
                        // A trial that had ended is sharing again: send what
                        // was waiting.
                        self.sync_asked = None;
                    }
                    // What the office bought decides whether this seat can
                    // reach the outside world at all.
                    hub::sealed::license_says(
                        standing.as_deref().and_then(|l| l.edition.as_deref()),
                    );
                    self.standing.license = standing.map(|b| *b);
                }
                Told::Copied { name, copy } => {
                    self.standing.busy = None;
                    self.status = format!(
                        "{} is yours to mark up. The issued set, {name}, is untouched.",
                        copy.name
                    );
                    // It belongs in the list beside the one it came from, and
                    // it opens, because the only reason to make one is to
                    // start working on it.
                    let (id, label) = (copy.id.clone(), copy.name.clone());
                    self.standing.sets.push(*copy);
                    self.ask(Ask::Open { set: id, name: label });
                }
                Told::FleetWatching { note, key } => {
                    self.office.fleet_note = Some(note);
                    self.office.fleet_key = key;
                    self.office.fleet_address = self.standing.base.clone();
                }
                Told::PasswordChanged => {
                    self.office.current_password.clear();
                    self.office.new_password.clear();
                    self.office.password_note = Some(
                        "Changed. Anywhere else you were signed in has been signed out.".into(),
                    );
                }
                Told::UpdateReady(version) => {
                    self.checking_for_update = false;
                    self.standing.update_refused = None;
                    self.status = format!(
                        "Excalibur View {version} is installed. Restart to use it, or it takes \
                         over the next time Excalibur View opens."
                    );
                    self.standing.update_ready = Some(version);
                }
                Told::UpdateRefused(why) => {
                    self.checking_for_update = false;
                    self.standing.update_refused = Some(why);
                }
                Told::Folders { project, folders, files } => {
                    if self.standing.looking_at.as_deref() == Some(project.as_str()) {
                        self.office.folders = folders; // WHAT: the project's folders. WHY: shown, the empty ones too.
                        self.office.files = files; // WHAT: its files. WHY: shown after the drawings.
                        self.office.folders_for = project; // WHAT: whose. WHY: drawn only for that project.
                    } // WHAT: an answer for a project no longer open is dropped. WHY: never shown under the wrong job.
                }
                Told::Groups(groups) => {
                    self.standing.busy = None; // WHAT: done. WHY: the spinner stops.
                    self.office.group_name.clear(); // WHAT: the box emptied. WHY: the group is made.
                    self.office.groups = groups; // WHAT: the groups now. WHY: shown.
                }
                Told::FileReady { path, name } => {
                    self.standing.busy = None; // WHAT: done. WHY: the spinner stops.
                    match crate::openwith::open(&path) {
                        Ok(()) => self.status = format!("Opened a copy of {name}. {}", crate::openwith::A_COPY), // WHAT: said, with the warning. WHY: a change saved in the copy goes nowhere.
                        Err(why) => self.error = Some(why), // WHAT: why it couldn't. WHY: said plainly.
                    }
                }
                Told::Said(said) => {
                    self.standing.busy = None; // WHAT: done. WHY: the spinner stops.
                    self.office.changing_rule = None; // WHAT: the rule picker closed. WHY: the change went.
                    self.status = said; // WHAT: the sentence. WHY: on the status line.
                }
                Told::Trouble(message) => {
                    self.standing.busy = None;
                    // The server says sharing is paused: find out where the
                    // office stands, so the panel says it once and plainly.
                    if message.contains("sharing new work") {
                        self.ask(Ask::License);
                    }
                    if let Some(signing) = self.signing_in.as_mut() {
                        signing.waiting = false;
                        signing.error = Some(message);
                    } else {
                        self.error = Some(message);
                    }
                }
            }
        }
    }

    /// Takes what the server had and puts it into the file.
    fn take_sync(
        &mut self,
        set: String,
        revision: u64,
        markups: Vec<(String, usize, String, bool)>,
        sent: Vec<String>,
    ) {
        use base64::Engine;
        self.sync_asked = None;
        // To the tab it belongs to, which may not be the one in front.
        let Some(at) = self
            .docs
            .iter()
            .position(|d| d.attached.as_ref().map(|a| a.set.as_str()) == Some(set.as_str()))
        else {
            return;
        };

        let theirs: Vec<(String, u32, annot::Markup, bool)> = markups
            .into_iter()
            .filter_map(|(name, page, dictionary, removed)| {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(dictionary.as_bytes())
                    .ok()?;
                let object = pdf::Reader::new(&bytes).object().ok()?;
                let dict = object.as_dict()?.clone();
                let markup = annot::Markup { dict, picture: None };
                // The server's own id is not the markup's name; the name in the
                // dictionary is, because that is what travels in the file.
                let name = match markup.name() {
                    it if !it.is_empty() => it,
                    _ => name,
                };
                Some((name, page as u32, markup, removed))
            })
            .collect();

        let ours = self.docs[at].names();
        let (add, remove, _) = crate::sync::plan(&theirs, &ours);
        let merged = self.docs[at].merge(add, remove);

        if let Some(attached) = self.docs[at].attached.as_mut() {
            attached.revision = revision;
            attached.sent.extend(sent);
            // Everything now in the file has been round, so nothing is sent
            // twice after a merge either.
            for (name, _, _, _) in &theirs {
                attached.sent.insert(name.clone());
            }
        }

        if let Some(said) = merged.says() {
            self.status = said;
        }
    }

    /// Records markups that reached the server when reading the set back
    /// afterwards did not.
    ///
    /// The revision is deliberately left alone: this machine has not seen
    /// anything new, so the next sync still asks from where it was. Only the
    /// names move, and only so they are not sent twice.
    pub fn take_push(&mut self, set: String, sent: Vec<String>) {
        let Some(at) = self
            .docs
            .iter()
            .position(|d| d.attached.as_ref().map(|a| a.set.as_str()) == Some(set.as_str()))
        else {
            return;
        };
        if let Some(attached) = self.docs[at].attached.as_mut() {
            attached.sent.extend(sent);
        }
    }

    pub fn ask(&self, ask: Ask) {
        if let Some(link) = self.link.as_ref() {
            let _ = link.to.send(ask);
        }
    }

    fn remember_server(&mut self, base: &str, token: &str, user: &str) {
        self.prefs.server = crate::server::Remembered {
            base: base.to_string(),
            token: token.to_string(),
            name: self.standing.name.clone(),
            user: user.to_string(),
            // Kept from the last health answer. A server that has not turned
            // remote access on says nothing here, and a seat that never
            // learned an outside address simply has none to fall back to.
            reachable_at: self.standing.reachable_at.clone(),
        };
        if let Err(e) = self.prefs.save() {
            log::warn!("could not keep the server details: {e}");
        }
    }

    fn forget_server(&mut self) {
        self.prefs.server = Default::default();
        let _ = self.prefs.save();
    }

    // ---- the connect window ----------------------------------------------
    //
    // The whole of a new company's first ten minutes happens in this one
    // window: find the server, set it up if nobody has, make an account if you
    // have not got one, sign in. Nobody types an address unless they want to,
    // and nobody runs anything.

    pub fn sign_in_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut signing) = self.signing_in.take() else {
            return;
        };
        let theme = self.chrome.theme;
        let mut keep = true;
        let mut check = false;
        let mut go = false;
        let mut look = false;
        let mut host_here = false;
        let mut pick: Option<String> = None;

        egui::Window::new("Connect to a server")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .max_width(700.0)
            .show(ctx, |ui| {
                ui.set_min_width(460.0);

                let painter = ui.painter();
                let mark = egui::Rect::from_min_size(
                    ui.next_widget_position() + egui::vec2(0.0, 2.0),
                    egui::vec2(44.0, 44.0),
                );
                ui::mark::draw(painter, mark, theme);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(56.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(heading(&signing.step)).strong());
                        ui.label(
                            RichText::new(explanation(&signing.step))
                                .color(theme.faint)
                                .size(11.0),
                        );
                    });
                });
                ui.add_space(14.0);

                match signing.step {
                    Step::Choosing => {
                        choosing(ui, theme, &mut signing, &mut look, &mut pick, &mut check,
                                 &mut host_here);
                    }
                    Step::SigningIn => {
                        where_it_is(ui, theme, &signing);
                        ui.add_space(10.0);
                        field(ui, theme, "Email", &mut signing.email, "you@yourcompany.com");
                        ui.add_space(8.0);
                        if secret(ui, theme, "Password", &mut signing.password) {
                            go = true;
                        }
                        if signing.joining == "code" || signing.joining == "open" {
                            ui.add_space(8.0);
                            if ui
                                .add(egui::Button::new(
                                    RichText::new("I haven't got an account yet").size(11.0),
                                )
                                .frame(false))
                                .clicked()
                            {
                                signing.step = Step::Joining;
                                signing.error = None;
                            }
                        }
                    }
                    Step::SettingUp => {
                        where_it_is(ui, theme, &signing);
                        ui.add_space(10.0);
                        field(
                            ui,
                            theme,
                            "What to call this server",
                            &mut signing.company,
                            "Your company's name",
                        );
                        note(
                            ui,
                            theme,
                            "It shows in everybody's title bar, so nobody marks up a drawing \
                             on the wrong one.",
                        );
                        ui.add_space(8.0);
                        field(ui, theme, "Your name", &mut signing.person, "First and last name");
                        note(ui, theme, "This goes in the Author column of every markup you make.");
                        ui.add_space(8.0);
                        field(ui, theme, "Email", &mut signing.email, "you@yourcompany.com");
                        ui.add_space(8.0);
                        if secret(ui, theme, "Password", &mut signing.password) {
                            go = true;
                        }
                        note(
                            ui,
                            theme,
                            "Twelve characters at least. Four unrelated words beats a short \
                             one with symbols in it.",
                        );
                    }
                    Step::Joining => {
                        where_it_is(ui, theme, &signing);
                        ui.add_space(10.0);
                        if signing.joining != "open" {
                            field(
                                ui,
                                theme,
                                "Join code",
                                &mut signing.code,
                                "beam-plate-weld-stud-truss-1a2b",
                            );
                            note(
                                ui,
                                theme,
                                "Whoever set the server up has this. Capitals and spaces do \
                                 not matter.",
                            );
                            ui.add_space(8.0);
                        }
                        field(ui, theme, "Your name", &mut signing.person, "Lee");
                        ui.add_space(8.0);
                        field(ui, theme, "Email", &mut signing.email, "you@yourcompany.com");
                        ui.add_space(8.0);
                        if secret(ui, theme, "Choose a password", &mut signing.password) {
                            go = true;
                        }
                        ui.add_space(8.0);
                        if ui
                            .add(
                                egui::Button::new(RichText::new("I already have an account").size(11.0))
                                    .frame(false),
                            )
                            .clicked()
                        {
                            signing.step = Step::SigningIn;
                            signing.error = None;
                        }
                    }
                    Step::JustSetUp => {
                        just_set_up(ui, theme, &signing);
                    }
                }

                if let Some(error) = &signing.error {
                    ui.add_space(8.0);
                    ui.colored_label(Color32::from_rgb(235, 120, 120), error);
                }

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    match signing.step {
                        Step::Choosing => {
                            let ready = !signing.base.trim().is_empty() && !signing.waiting;
                            if ui.add_enabled(ready, egui::Button::new("Continue")).clicked() {
                                check = true;
                            }
                        }
                        Step::JustSetUp => {
                            if ui.button("Done").clicked() {
                                keep = false;
                            }
                        }
                        _ => {
                            if ui
                                .add_enabled(ready_to_go(&signing), egui::Button::new(button(&signing.step)))
                                .clicked()
                            {
                                go = true;
                            }
                            if ui
                                .add(egui::Button::new(RichText::new("Back").size(11.0)).frame(false))
                                .clicked()
                            {
                                signing.step = Step::Choosing;
                                signing.error = None;
                                signing.password.clear();
                            }
                        }
                    }
                    if signing.step != Step::JustSetUp && ui.button("Cancel").clicked() {
                        keep = false;
                    }
                    if signing.waiting {
                        ui.add_space(8.0);
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new("Excalibur View keeps a session, never your password.")
                                .color(theme.faint)
                                .size(10.0),
                        );
                    });
                });
            });

        if let Some(base) = pick {
            signing.base = base;
            signing.found = None;
            signing.found_version = None;
            check = true;
        }
        if look {
            signing.looking = true;
            signing.servers.clear();
            signing.error = None;
            self.ask(Ask::Look);
        }
        if host_here {
            self.host_on_this_computer(&mut signing);
        }
        if check {
            signing.waiting = true;
            signing.error = None;
            self.ask(Ask::Check(signing.base.trim().to_string()));
        }
        if go {
            signing.waiting = true;
            signing.error = None;
            let base = signing.base.trim().to_string();
            match signing.step {
                Step::SettingUp => self.ask(Ask::Claim {
                    base,
                    setup: Box::new(hub::Setup {
                        company: signing.company.trim().to_string(),
                        name: signing.person.trim().to_string(),
                        email: signing.email.trim().to_string(),
                        password: signing.password.clone(),
                    }),
                }),
                Step::Joining => self.ask(Ask::Join {
                    base,
                    join: Box::new(hub::Join {
                        code: signing.code.trim().to_string(),
                        name: signing.person.trim().to_string(),
                        email: signing.email.trim().to_string(),
                        password: signing.password.clone(),
                    }),
                }),
                _ => self.ask(Ask::SignIn {
                    base,
                    email: signing.email.trim().to_string(),
                    password: signing.password.clone(),
                }),
            }
            // Out of memory as soon as it has gone. It is never written down.
            signing.password.clear();
        }
        if keep {
            self.signing_in = Some(signing);
        }
    }

    /// Serves the drawings from this machine, starting now.
    ///
    /// Two separate things, and they are told apart on purpose. Serving happens
    /// immediately and needs nothing of anybody. Surviving a reboot needs
    /// administrator rights, and if Windows says no the first thing still
    /// worked — so the person is told what they have rather than that it
    /// failed.
    fn host_on_this_computer(&mut self, signing: &mut SigningIn) {
        let name = if signing.company.trim().is_empty() {
            "Excalibur View".to_string()
        } else {
            signing.company.trim().to_string()
        };
        let folder = crate::host::default_data_folder();
        let port = hyperview_server::config::DEFAULT_PORT;

        match crate::host::start_here(&name, folder.clone(), port) {
            Ok(hosting) => {
                signing.base = hosting.url.clone();
                signing.error = None;
                signing.found = None;
                signing.found_version = None;
                let standing = crate::host::keep_serving(&folder, port, &name);
                signing.standing = Some(standing.says());
                self.status = hosting.tell_the_office();
                // Held for as long as the program runs: dropping it stops the
                // server, and a server that stopped because a struct went out
                // of scope is the kind of thing somebody spends a day on.
                self.hosting = Some(hosting);
                // Straight on to setting it up, since the person who just
                // pressed this is the one who is going to.
                signing.step = Step::SettingUp;
                signing.waiting = false;
            }
            Err(why) => {
                signing.error = Some(why);
                signing.waiting = false;
            }
        }
    }

    // ---- the panel -------------------------------------------------------

    pub fn server_panel(&mut self, ui: &mut egui::Ui) {
        let theme = self.chrome.theme;

        if !self.standing.signed_in() {
            ui.add_space(10.0);
            ui.vertical_centered(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(52.0, 52.0), egui::Sense::hover());
                ui::mark::draw(ui.painter(), rect, theme);
                ui.add_space(10.0);
                ui.label(RichText::new("Not connected").strong());
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Studio is where an office shares drawings and takeoff. Connect to your \
                         company's server for shared drawings, tool chests and takeoffs.",
                    )
                    .color(theme.faint)
                    .size(11.0),
                );
                ui.add_space(12.0);
                if ui.button("Connect…").clicked() {
                    self.begin_sign_in();
                }
                ui.add_space(10.0);
                ui.label(
                    RichText::new(
                        "No server yet? Excalibur View Office puts one on any computer in the \
                         office. Try it free for 30 days.",
                    )
                    .color(theme.faint)
                    .size(10.0),
                );
                if crate::edition::sells() {
                    ui.hyperlink_to(RichText::new("Get the server").size(11.0), hub::site::OFFICE_SERVER);
                }
            });
            return;
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.standing.name).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Sign out").clicked() {
                    self.ask(Ask::SignOut);
                }
            });
        });
        if let Some(who) = self.standing.who.clone() {
            ui.label(
                RichText::new(format!("{} · {}", who.name, role_name(who.role)))
                    .color(theme.faint)
                    .size(11.0),
            );
        }
        if let Some(pinned) = self.standing.pinned_to.clone() {
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!("This office is held on version {pinned}."))
                    .color(theme.faint)
                    .size(10.0),
            );
        }
        // Licensing is said to administrators, and to everybody else only
        // when it stops them sharing. Never a pop-up, never a countdown for
        // somebody who can do nothing about it.
        if let Some(license) = self.standing.license.clone() {
            let admin = self.standing.who.as_ref().map(|w| w.role) == Some(hub::Role::Admin);
            let urgent = !license.sharing || license.trial_days_left.is_some_and(|d| d <= 7);
            let show = !license.message.is_empty() && (admin && license.state != "founding" || !license.sharing);
            if show {
                ui.add_space(4.0);
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .corner_radius(4.0)
                    .fill(if urgent { theme.warn.gamma_multiply(0.15) } else { theme.hover })
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(&license.message)
                                .color(if urgent { theme.warn } else { theme.text })
                                .size(11.0),
                        );
                        // Every message an administrator is shown here has
                        // the same two ways forward: a license file, or the
                        // page that sells one.
                        if admin && license.state != "founding" && crate::edition::sells() {
                            let licensed = license.state == "licensed";
                            ui.horizontal(|ui| {
                                if ui.small_button("Add license file…").clicked() {
                                    self.pick_license();
                                }
                                ui.hyperlink_to(
                                    RichText::new(if licensed { "Renew or add users" } else { "Buy Office" }).size(11.0),
                                    if licensed {
                                        hub::site::add_url(license.license_id.as_deref())
                                    } else {
                                        hub::site::BUY.to_string()
                                    },
                                );
                            });
                        }
                    });
            }
        }
        if let Some(busy) = self.standing.busy.clone() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(busy).color(theme.faint).size(11.0));
            });
        }
        ui.separator();

        let mut open_project: Option<String> = None;
        let mut open_set: Option<(String, String)> = None;
        let mut copy_set: Option<(String, String)> = None;
        let mut fetch_chest: Option<(String, String)> = None;
        let mut back = false;
        let mut actions = PanelActions::default();
        let role = self.standing.who.as_ref().map(|w| w.role);
        let is_admin = role == Some(hub::Role::Admin);
        // Cloned because the panel is drawn with `self` borrowed mutably, and
        // a list of a shop's staff is a handful of short strings.
        let people = self.standing.people.clone();
        let me = self.standing.who.as_ref().map(|w| w.id.clone()).unwrap_or_default();
        let may_write = matches!(role, Some(hub::Role::Admin) | Some(hub::Role::Estimator));

        // While this panel is on screen it keeps itself current, because the
        // thing somebody is looking at should be what is on the server rather
        // than what was on it when they signed in. Ten seconds, and only
        // while the panel is open and somebody is signed in: a list of
        // projects is a few hundred bytes, and a seat already asks about an
        // open drawing every eight.
        const LOOK_AGAIN: std::time::Duration = std::time::Duration::from_secs(10);
        if self.standing.who.is_some() && !self.standing.base.is_empty() {
            let due = self.office.last_looked.is_none_or(|at| at.elapsed() >= LOOK_AGAIN);
            if due {
                actions.look_again = true;
            }
            ui.ctx().request_repaint_after(LOOK_AGAIN);
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                match self.standing.looking_at.clone() {
                    None => {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Projects").color(theme.faint).size(11.0));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if may_write && self.office.new_project.is_none() {
                                        if ui.small_button("New project").clicked() {
                                            self.office.new_project =
                                                Some((String::new(), String::new()));
                                        }
                                    }
                                    // This list keeps itself current on its
                                    // own. The button is for the moment
                                    // somebody has just pushed a bid across
                                    // from another program and does not want
                                    // to wait even ten seconds to see it.
                                    if ui
                                        .small_button("Refresh")
                                        .on_hover_text(
                                            "Ask the server what is on it now.",
                                        )
                                        .clicked()
                                    {
                                        actions.look_again = true;
                                    }
                                },
                            );
                        });
                        let mut close_form = false;
                        if let Some((number, name)) = self.office.new_project.as_mut() {
                            ui.add_space(4.0);
                            ui.add(
                                egui::TextEdit::singleline(number)
                                    .hint_text("Job number")
                                    .desired_width(f32::INFINITY),
                            );
                            ui.add(
                                egui::TextEdit::singleline(name)
                                    .hint_text("Job name")
                                    .desired_width(f32::INFINITY),
                            );
                            ui.horizontal(|ui| {
                                let ready = !number.trim().is_empty() && !name.trim().is_empty();
                                if ui.add_enabled(ready, egui::Button::new("Start it")).clicked() {
                                    actions.new_project =
                                        Some((number.trim().to_string(), name.trim().to_string()));
                                    close_form = true;
                                }
                                if ui.button("Cancel").clicked() {
                                    close_form = true;
                                }
                            });
                            ui.add_space(6.0);
                        }
                        if close_form {
                            self.office.new_project = None;
                        }
                        if self.standing.projects.is_empty() {
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("Nothing on this server yet.")
                                    .color(theme.faint)
                                    .size(11.0),
                            );
                        }
                        // As the office keeps its files: the active jobs, the
                        // bids by year, the lost ones, the finished jobs by
                        // year, each where the program it belongs to put it.
                        // A server where nothing has a place shows the list
                        // it always did.
                        let tree = crate::shelves::shelve(
                            &self.standing.projects,
                            |p| p.place.as_deref(),
                            crate::shelves::OFFICE,
                        );
                        if tree.shelves.is_empty() {
                            for project in &tree.items {
                                project_row(ui, theme, project, &mut open_project);
                            }
                        } else {
                            for shelf in &tree.shelves {
                                project_shelf(ui, theme, shelf, &mut open_project);
                            }
                            if !tree.items.is_empty() {
                                egui::CollapsingHeader::new(
                                    RichText::new(format!("Other projects ({})", tree.items.len()))
                                        .size(12.0),
                                )
                                .id_salt("shelf:")
                                .default_open(false)
                                .show(ui, |ui| {
                                    for project in &tree.items {
                                        project_row(ui, theme, project, &mut open_project);
                                    }
                                });
                            }
                        }

                        if !self.standing.chests.is_empty() || is_admin {
                            ui.add_space(12.0);
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Shared tool chests")
                                        .color(theme.faint)
                                        .size(11.0),
                                );
                                if is_admin {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .small_button("Share one")
                                                .on_hover_text(
                                                    "Put a tool chest on the server (.evtools, or a \
                                                     Revu .bpx or .btx), so everybody in the office \
                                                     can use it.",
                                                )
                                                .clicked()
                                            {
                                                actions.share_chest = true;
                                            }
                                        },
                                    );
                                }
                            });
                            for chest in &self.standing.chests {
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.label(RichText::new(&chest.name).size(12.0));
                                        ui.label(
                                            RichText::new(format!(
                                                "{} tools in {} sets",
                                                chest.tools, chest.sets
                                            ))
                                            .color(theme.faint)
                                            .size(10.0),
                                        );
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.small_button("Use").clicked() {
                                                fetch_chest =
                                                    Some((chest.id.clone(), chest.name.clone()));
                                            }
                                        },
                                    );
                                });
                                ui.separator();
                            }
                        }
                        ui.add_space(12.0);
                        ui.separator();
                        if is_admin {
                            self.office.base = self.standing.base.clone();
                            office_section(
                                ui,
                                theme,
                                &mut self.office,
                                self.standing.license.as_ref(),
                                &people,
                                &me,
                                &mut actions,
                            );
                        }
                        password_section(ui, theme, &mut self.office, &mut actions);
                    }
                    Some(project) => {
                        let response = ui.horizontal(|ui| {
                            let (mark, _) = ui
                                .allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                            arrow_back(ui.painter(), mark, theme.faint);
                            ui.label(RichText::new("Projects").color(theme.faint).size(11.0));
                        });
                        if response.response.interact(egui::Sense::click()).clicked() {
                            back = true;
                        }
                        if may_write {
                            ui.add_space(4.0);
                            if ui
                                .button("Add drawing sets…")
                                .on_hover_text(
                                    "Put PDFs from this computer on the server, in this \
                                     project. Everybody in the office sees them, and markups \
                                     made on them are shared.",
                                )
                                .clicked()
                            {
                                actions.upload_to = Some(project.clone());
                            }
                        }
                        ui.add_space(6.0);
                        if self.standing.sets.is_empty() {
                            ui.label(
                                RichText::new("No drawing sets in this project yet.")
                                    .color(theme.faint)
                                    .size(11.0),
                            );
                        }
                        // Every folder of the job as its own files have it,
                        // the empty ones too, each with its drawings and then
                        // its other files; the ones in no folder at the top.
                        let shown_folders: &[hub::FolderInfo] = if self.office.folders_for == project { &self.office.folders } else { &[] }; // WHAT: the folders, if they are this project's. WHY: never another job's.
                        let shown_files: &[hub::ProjectFile] = if self.office.folders_for == project { &self.office.files } else { &[] }; // WHAT: the files, likewise. WHY: likewise.
                        let mut things: Vec<InFolder> = self.standing.sets.iter().map(InFolder::Set).collect(); // WHAT: the drawings first. WHY: drawings before files in each folder.
                        things.extend(shown_files.iter().map(InFolder::File)); // WHAT: then the files. WHY: after the drawings.
                        things.extend(shown_folders.iter().map(|f| InFolder::Folder(f.folder.as_str()))); // WHAT: then every folder, so an empty one shows. WHY: the server lists them all.
                        let folders = crate::shelves::shelve(&things, InFolder::folder, &[]); // WHAT: put on shelves by folder. WHY: as the job's own files.
                        let mut view = FolderView {
                            rules: shown_folders,
                            groups: &self.office.groups,
                            admin: is_admin,
                            changing: &mut self.office.changing_rule,
                            open_set: &mut open_set,
                            copy_set: &mut copy_set,
                            open_file: &mut actions.open_file,
                            folder_rule: &mut actions.folder_rule,
                        }; // WHAT: what drawing a folder needs. WHY: one bundle for the recursive drawing below.
                        for thing in &folders.items {
                            view.thing(ui, theme, thing); // WHAT: what is in no folder. WHY: at the top, as before.
                        }
                        for shelf in &folders.shelves {
                            view.shelf(ui, theme, shelf); // WHAT: each folder. WHY: with who sees it.
                        }
                    }
                }
            });

        if actions.people {
            self.ask(Ask::People);
            self.ask(Ask::Groups); // WHAT: the groups too. WHY: shown under the people.
        }
        if let Some((name, email, password, role)) = actions.add_person.take() {
            self.standing.busy = Some("Making the account…".into());
            self.ask(Ask::AddPerson { name, email, password, role });
        }
        if let Some((id, role)) = actions.change_role.take() {
            self.standing.busy = Some("Changing what they can do…".into());
            self.ask(Ask::ChangeRole { id, role });
        }
        if let Some((id, name)) = actions.remove_person.take() {
            self.standing.busy = Some("Removing them…".into());
            self.ask(Ask::RemovePerson { id, name });
        }
        if actions.look_again {
            self.office.last_looked = Some(std::time::Instant::now());
            self.ask(Ask::Projects);
            self.ask(Ask::Chests);
            // And, if a project is open, what is in it -- a set issued again
            // from FabWire replaces the one on screen.
            if let Some(project) = self.standing.looking_at.clone() {
                self.ask(Ask::Sets(project));
            }
        }
        if back {
            self.standing.looking_at = None;
            self.standing.sets.clear();
        }
        if let Some(project) = open_project {
            self.standing.looking_at = Some(project.clone());
            self.standing.busy = Some("Looking…".into());
            self.office.folders.clear(); // WHAT: the last project's folders dropped. WHY: never shown under this one.
            self.office.files.clear(); // WHAT: and its files. WHY: likewise.
            self.ask(Ask::Sets(project));
            if is_admin {
                self.ask(Ask::Groups); // WHAT: the groups. WHY: an administrator picks from them for a folder.
            }
        }
        if let Some((file, name)) = actions.open_file.take() {
            self.standing.busy = Some(format!("Getting a copy of {name}…")); // WHAT: said while it comes down. WHY: a big file takes a moment.
            self.ask(Ask::OpenFile { file, name }); // WHAT: fetched and opened. WHY: Open with its program.
        }
        if let (Some((folder, see)), Some(project)) = (actions.folder_rule.take(), self.standing.looking_at.clone()) {
            self.standing.busy = Some("Changing who sees that folder…".into()); // WHAT: said. WHY: the spinner.
            self.ask(Ask::SetFolderRule { project, folder, see }); // WHAT: sent. WHY: the server keeps and enforces it.
        }
        if let Some(name) = actions.make_group.take() {
            self.standing.busy = Some("Making the group…".into()); // WHAT: said. WHY: the spinner.
            self.ask(Ask::MakeGroup(name)); // WHAT: sent. WHY: a new group.
        }
        if let Some((group, person, on)) = actions.change_group.take() {
            self.ask(Ask::ChangeGroup { group, person, on }); // WHAT: sent. WHY: decides the folders they see.
        }
        if let Some((set, name)) = copy_set {
            self.standing.busy = Some("Making a copy to take off on…".into());
            self.ask(Ask::CopySet { set, name });
        }
        if let Some((set, name)) = open_set {
            self.ask(Ask::Open { set, name });
        }
        if let Some((id, name)) = fetch_chest {
            self.ask(Ask::FetchChest { id, name });
        }
        if let Some((number, name)) = actions.new_project {
            self.standing.busy = Some("Starting the project…".into());
            self.ask(Ask::NewProject { number, name });
        }
        if let Some(project) = actions.upload_to {
            self.filing.many(
                crate::files::Choose::open_many()
                    .title("Put drawing sets on the server")
                    .filter("PDF drawings", &["pdf", "PDF"]),
                move |app, files| {
                    if !files.is_empty() {
                        app.ask(Ask::UploadSets { project, files });
                    }
                },
            );
        }
        if actions.share_chest {
            self.filing.one(
                crate::files::Choose::open()
                    .title("Share a tool chest with the office")
                    .filter("Tool chest", &[chest::native::EXTENSION, "bpx", "BPX", "btx", "BTX"]),
                |app, file| app.ask(Ask::ShareChest(file)),
            );
        }
        if actions.office {
            self.ask(Ask::Office);
            self.ask(Ask::License);
            self.ask(Ask::Remote);
        }
        if actions.add_license {
            self.pick_license();
        }
        if actions.paste_license {
            // The button says Cancel while the box is open, so one action does
            // both: open it empty, or throw away what is in it.
            self.office.pasting_license = match self.office.pasting_license {
                Some(_) => None,
                None => Some(String::new()),
            };
        }
        if let Some(text) = actions.send_license {
            self.office.pasting_license = None;
            self.ask(Ask::AddLicenseText(text));
        }
        if let Some(path) = actions.use_found_license {
            self.ask(Ask::AddLicense(path));
        }
        if let Some(days) = actions.code_days {
            self.ask(Ask::JoinCodeDays(days));
        }
        if actions.new_code {
            self.ask(Ask::NewJoinCode);
        }
        if let Some(channel) = actions.channel {
            self.ask(Ask::UpdateChannel(channel));
        }
        if actions.watch_fleet {
            self.office.fleet_note = Some("Letting Excalibur Fleet in…".into());
            self.office.fleet_key = None;
            self.ask(Ask::WatchWithFleet {
                name: self.standing.name.clone(),
                base: self.standing.base.clone(),
            });
        }
        if actions.stop_fleet {
            self.office.fleet_note = None;
            self.office.fleet_key = None;
            self.ask(Ask::StopFleet);
        }
        if let Some((token, hostname)) = actions.remote_on {
            self.office.remote_note = Some("Starting…".into());
            // The token leaves the boxes the moment it is sent. It is a
            // secret and there is no reason for it to sit on screen behind
            // whatever somebody opens next.
            self.office.remote_token.clear();
            self.ask(Ask::RemoteOn { token, hostname });
        }
        if actions.remote_off {
            self.office.remote_note = None;
            self.office.remote_token.clear();
            self.ask(Ask::RemoteOff);
        }
        if actions.remote_look {
            self.ask(Ask::Remote);
        }
        if let Some(name) = actions.make_key {
            self.ask(Ask::MakeIntegrationKey(name));
        }
        if let Some(name) = actions.make_sync_key {
            self.ask(Ask::MakeSyncKey(name)); // WHAT: ask the server for a sync key. WHY: Make a sync key was pressed.
        }
        if let Some(id) = actions.revoke_key {
            self.ask(Ask::RevokeKey(id));
        }
        if let Some(url) = actions.add_notice {
            self.ask(Ask::AddNotice(url));
        }
        if let Some(id) = actions.test_notice {
            self.office.notice_note = Some("Sending a test notice…".into());
            self.ask(Ask::TestNotice(id));
        }
        if let Some(id) = actions.remove_notice {
            self.ask(Ask::RemoveNotice(id));
        }
        if let Some((current, new)) = actions.password {
            self.office.password_note = None;
            self.ask(Ask::ChangePassword { current, new });
        }
    }

    pub fn begin_sign_in(&mut self) {
        let remembered = self.prefs.server.clone();
        // Whatever this seat used last, or failing that whatever the installer
        // set it up for. A seat that already knows its server skips straight to
        // the password box; a fresh one starts by looking round the network,
        // which is the case this whole window exists for.
        let base = self.joined.address(&remembered.base);
        let known = !base.trim().is_empty();
        // The machine's login name is a fine default for the Author column on a
        // markup, and a poor one for the name on an account: "creed" and
        // "Unknown" are both worse than the box being empty with "Sam" in
        // grey behind it. So it is offered only when it looks like a name
        // somebody chose.
        let person = match self.author.trim() {
            "" | "Unknown" => String::new(),
            name if name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) => {
                String::new()
            }
            name => name.to_string(),
        };
        self.signing_in = Some(SigningIn {
            base,
            person,
            joined_note: self.joined.note.clone(),
            ..Default::default()
        });
        if known {
            if let Some(signing) = self.signing_in.as_mut() {
                signing.waiting = true;
            }
            let base = self.joined.address(&remembered.base);
            self.ask(Ask::Check(base));
        } else {
            if let Some(signing) = self.signing_in.as_mut() {
                signing.looking = true;
            }
            self.ask(Ask::Look);
        }
    }

    /// A newer version, said as soon as it is known about, in a box in the
    /// middle of the window with what is new in it (see [`crate::updatebox`]).
    ///
    /// In place: Restart now, or Later leaves it to take over at the next
    /// start. Being fetched: a small card in the corner says so, so nobody
    /// wonders, and the box opens when it is ready. Out, but this copy cannot
    /// put it in place itself — a copy run from a download folder, or one whose
    /// update was refused — Download opens the page to get it from, with why.
    /// Nothing ever restarts on its own: a takeoff half done is never
    /// interrupted.
    pub fn update_box(&mut self, ctx: &egui::Context) {
        let running = env!("CARGO_PKG_VERSION");
        let offered = self
            .standing
            .update
            .clone()
            .filter(|r| hub::update::newer(&r.version, running));
        let ready = self
            .standing
            .update_ready
            .clone()
            .filter(|v| hub::update::newer(v, running));
        let version = match (&ready, &offered) {
            (Some(v), _) => Some(v.clone()),
            (None, Some(r)) => Some(r.version.clone()),
            (None, None) => None,
        }
        .filter(|v| self.dismissed_update.as_deref() != Some(v.as_str()));
        let theme = self.chrome.theme;
        let open_id = egui::Id::new("update-box-open");
        let was_open = ctx.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
        let mut open = false;
        if let Some(version) = version {
            let notes = offered
                .as_ref()
                .filter(|r| r.version == version)
                .map(|r| r.notes.clone())
                .unwrap_or_default();
            let refused = self.standing.update_refused.clone();
            let offer = if ready.is_some() {
                Some(updatebox::Offer::Ready)
            } else if let Some(why) = refused.as_deref() {
                Some(updatebox::Offer::Download { why: Some(why) })
            } else if !self.updates_itself {
                Some(updatebox::Offer::Download { why: None })
            } else {
                None
            };
            match offer {
                Some(offer) if updatebox::may_open(ctx, was_open) => {
                    open = true;
                    match updatebox::show(ctx, theme, &version, running, &notes, offer).0 {
                        updatebox::Choice::Open => {}
                        updatebox::Choice::Later => self.dismissed_update = Some(version),
                        updatebox::Choice::Download => {
                            let page = updatebox::download_page(Some(version.as_str())); // WHAT: the offered version's own release page. WHY: "latest" skips early releases, so an early-channel office was sent to the version it already had.
                            self.dismissed_update = Some(version);
                            if let Some(page) = page {
                                ctx.open_url(egui::OpenUrl::new_tab(page));
                            }
                        }
                        updatebox::Choice::Restart => self.restart_into_update(ctx),
                    }
                }
                // Waiting for a mouse button to be let go.
                Some(_) => {}
                // Still being fetched. Hiding the card hides the card only:
                // the box still opens when it is ready.
                None => {
                    let hidden = egui::Id::new(("update-getting-hidden", version.as_str()));
                    if !ctx.data(|d| d.get_temp::<bool>(hidden)).unwrap_or(false)
                        && updatebox::getting(ctx, theme, &version)
                    {
                        ctx.data_mut(|d| d.insert_temp(hidden, true));
                    }
                }
            }
        }
        ctx.data_mut(|d| d.insert_temp(open_id, open));
        if !open {
            self.update_answer(ctx);
        }
    }

    /// "Check for Updates" being answered, or its answer when there is
    /// nothing to install; also what Connect Assistant has to say.
    fn update_answer(&mut self, ctx: &egui::Context) {
        if !self.checking_for_update && self.update_note.is_none() {
            return;
        }
        if updatebox::answer(ctx, self.chrome.theme, self.update_note.as_deref()) {
            // Closed while still asking: the answer is still said when it
            // comes.
            self.checking_for_update = false;
            self.update_note = None;
        }
    }

    /// Everything open is saved, a new copy is started with the same
    /// drawings, and this window closes. The new copy waits for this one to
    /// let go of the window before it opens its own.
    pub fn restart_into_update(&mut self, ctx: &egui::Context) {
        self.save_all();
        let open: Vec<std::path::PathBuf> = self.docs.iter().map(|d| d.path.clone()).collect();
        match crate::install::restart(&open) {
            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(why) => {
                self.error = Some(format!(
                    "Excalibur View could not restart itself ({why}). Close it and open it again \
                     to use the new version."
                ))
            }
        }
    }

    /// Help → Connect Assistant.
    ///
    /// Signed in to an office server, the assistant gets a read-only key to the
    /// office's jobs as well as the window. Without one — one person on the
    /// free app — it gets the window alone: what is open, reading and looking
    /// at sheets, the drawing's takeoff, and proposals with a Fix button.
    pub fn connect_assistant(&mut self) {
        // The assistant runs on somebody else's computers. On a sealed seat that is
        // the whole of the objection, and there is no version of it that is
        // allowed.
        if hub::sealed::is_sealed() {
            self.error = Some(hub::sealed::refusal("The assistant"));
            return;
        }
        if !self.standing.signed_in() {
            self.update_note = Some(match crate::assistant::add_to_assistant_app() {
                // WHAT: what the person is told once the assistant app is connected. WHY: names the Help command that checks it, as the menu shows it.
                Ok(places) => format!(
                    "The assistant app can work alongside you in this window now: it sees what is open, reads \
                     and looks at sheets, totals the drawing's takeoff, and proposes markups and scales with a \
                     Fix button — only you draw. Quit the assistant app from the tray (right-click its icon, Quit — \
                     closing the window is not enough) and open it again, then ask it something like \"what's \
                     on this sheet?\" ({} settings file{} updated. Help → Test Assistant Connection checks it any \
                     time.) An office on Excalibur View Office signs in to its server and connects again to add \
                     its jobs.",
                    places.len(),
                    if places.len() == 1 { "" } else { "s" }
                ),
                Err(why) => {
                    self.error = Some(format!("The assistant could not be connected: {why}"));
                    return;
                }
            });
            return;
        }
        let computer = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "this computer".into());
        self.status = "Connecting the assistant…".into();
        self.ask(Ask::ConnectAssistant { computer });
    }

    /// Help → Check for Updates.
    pub fn check_for_updates_now(&mut self) {
        self.checking_for_update = true;
        self.update_note = None;
        self.dismissed_update = None;
        self.status = if self.standing.base.is_empty() {
            "Checking for updates…".into()
        } else {
            "Asking your server for updates…".into()
        };
        self.last_update_check = std::time::Instant::now();
        self.ask(Ask::CheckForUpdate {
            running: env!("CARGO_PKG_VERSION").to_string(),
            channel: self.prefs.channel,
            asked: true,
        });
    }
}

/// What an administrator looks after: the code people join with, and how the
/// office keeps up to date.
/// Who is on this server, and what an administrator can do about it.
///
/// The server has been able to change a role and remove a person since
/// 0.6.4, and for that whole time the only way to do either was to make the
/// request by hand. A shop foreman is not going to do that, so in practice
/// the shop could not take a departing employee's access away -- which is
/// not access control, it is a suggestion.
///
/// Three rules this screen keeps. Nothing about somebody else's account
/// changes on a single click of a small button. Your own row cannot be the
/// one you remove -- the server refuses it, and so does this, before you
/// find out the hard way. And every change is followed by the list as the
/// server now has it, rather than by this screen editing its own copy and
/// hoping.
/// Groups of people ("Shop", "Field"): make one, and tick who is in each. A
/// folder given to a group is seen by the people in it and by the office.
fn groups_section(ui: &mut egui::Ui, theme: ui::chrome::Theme, office: &mut OfficePanel, people: &[hub::User], actions: &mut PanelActions) {
    ui.add_space(10.0);
    ui.label(RichText::new("Groups").strong().size(12.0)); // WHAT: the heading. WHY: under People.
    ui.label(
        RichText::new("Give a project's folder to groups and only the people in them, and the office, see it. Viewers see only the folders given to everyone or to one of their groups; administrators and estimators see every folder.")
            .color(theme.faint)
            .size(10.0),
    ); // WHAT: what groups are for. WHY: said once, where they are made.
    for group in office.groups.clone() {
        ui.add_space(4.0);
        ui.label(RichText::new(format!("{} ({})", group.name, group.people.len())).size(12.0)); // WHAT: the group and how many are in it. WHY: shown.
        ui.horizontal_wrapped(|ui| {
            for person in people {
                let mut on = group.people.contains(&person.id); // WHAT: whether they are in it. WHY: the tick.
                if ui.checkbox(&mut on, &person.name).changed() {
                    actions.change_group = Some((group.id.clone(), person.id.clone(), on)); // WHAT: in or out. WHY: sent after drawing.
                }
            }
        });
    }
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut office.group_name).hint_text("Shop, Field…").desired_width(160.0)); // WHAT: a new group's name. WHY: typed.
        let ready = !office.group_name.trim().is_empty(); // WHAT: a name typed. WHY: a group needs one.
        if ui.add_enabled(ready, egui::Button::new("Make group")).clicked() {
            actions.make_group = Some(office.group_name.trim().to_string()); // WHAT: asked for. WHY: sent after drawing.
        }
    });
}

fn people_section(
    ui: &mut egui::Ui,
    theme: ui::chrome::Theme,
    office: &mut OfficePanel,
    people: &[hub::User],
    me: &str,
    actions: &mut PanelActions,
) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("People").strong().size(12.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if office.new_person.is_none() && ui.small_button("Add somebody").clicked() {
                office.new_person = Some(NewPerson::default());
            }
        });
    });

    if people.is_empty() {
        ui.label(RichText::new("Asking the server…").color(theme.faint).size(10.0));
        ui.spinner();
    }

    for person in people {
        let is_me = person.id == me;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(if is_me {
                    format!("{} — you", person.name)
                } else {
                    person.name.clone()
                }))
                ;
                ui.label(RichText::new(&person.email).color(theme.faint).size(10.0));
            });
        });
        ui.horizontal(|ui| {
            let mut role = person.role;
            egui::ComboBox::from_id_salt(format!("role-{}", person.id))
                .selected_text(role_name(role))
                .width(120.0)
                .show_ui(ui, |ui| {
                    for choice in
                        [hub::Role::Viewer, hub::Role::Estimator, hub::Role::Admin]
                    {
                        ui.selectable_value(&mut role, choice, role_name(choice));
                    }
                });
            if role != person.role {
                actions.change_role = Some((person.id.clone(), role));
            }
            if !is_me {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Remove").clicked() {
                        office.removing = Some(person.id.clone());
                    }
                });
            }
        });
        if office.removing.as_deref() == Some(person.id.as_str()) {
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!(
                    "Remove {}? They are signed out at once and their keys stop \
                     working. Their markups and the sets they uploaded stay.",
                    person.name
                ))
                .color(theme.warn)
                .size(10.0),
            );
            ui.horizontal(|ui| {
                if ui.button("Remove them").clicked() {
                    actions.remove_person = Some((person.id.clone(), person.name.clone()));
                }
                if ui.button("Cancel").clicked() {
                    office.removing = None;
                }
            });
        }
        ui.add_space(2.0);
        ui.separator();
    }

    let mut cancel_new = false;
    if let Some(new) = office.new_person.as_mut() {
        ui.add_space(6.0);
        field(ui, theme, "Name", &mut new.name, "As it should read on a markup");
        field(ui, theme, "Email", &mut new.email, "What they sign in with");
        let mut role = new.role.unwrap_or(hub::Role::Estimator);
        egui::ComboBox::from_id_salt("new-person-role")
            .selected_text(role_name(role))
            .width(120.0)
            .show_ui(ui, |ui| {
                for choice in [hub::Role::Viewer, hub::Role::Estimator, hub::Role::Admin] {
                    ui.selectable_value(&mut role, choice, role_name(choice));
                }
            });
        new.role = Some(role);
        ui.add(
            egui::TextEdit::singleline(&mut new.password)
                .password(true)
                .hint_text("Password — at least twelve characters")
                .desired_width(f32::INFINITY),
        );
        ui.label(
            RichText::new(
                "Four unrelated words is easier to type and far harder to guess \
                 than a short one with symbols in it.",
            )
            .color(theme.faint)
            .size(10.0),
        );
        let ready = !new.name.trim().is_empty()
            && new.email.contains('@')
            && new.password.chars().count() >= 12;
        ui.horizontal(|ui| {
            if ui.add_enabled(ready, egui::Button::new("Give them an account")).clicked() {
                actions.add_person = Some((
                    new.name.trim().to_string(),
                    new.email.trim().to_string(),
                    new.password.clone(),
                    role,
                ));
            }
            if ui.button("Cancel").clicked() {
                cancel_new = true;
            }
        });
    }
    if cancel_new {
        office.new_person = None;
    }
}

/// A project in the Studio's list: its number and name, and how many drawings.
fn project_row(
    ui: &mut egui::Ui,
    theme: ui::chrome::Theme,
    project: &hub::Project,
    open_project: &mut Option<String>,
) {
    let response = ui.add(
        egui::Button::new(RichText::new(how_it_reads(&project.number, &project.name)))
            .frame(false)
            .min_size(egui::vec2(ui.available_width(), 22.0)),
    );
    if response.clicked() {
        *open_project = Some(project.id.clone());
    }
    ui.label(
        RichText::new(format!(
            "{} drawing set{}",
            project.sets,
            if project.sets == 1 { "" } else { "s" }
        ))
        .color(theme.faint)
        .size(10.0),
    );
}

/// A shelf of projects -- `Bids`, and `2026` in it -- folded, with how many
/// are in it; the active jobs and the bids start open.
fn project_shelf(
    ui: &mut egui::Ui,
    theme: ui::chrome::Theme,
    shelf: &crate::shelves::Shelf<'_, hub::Project>,
    open_project: &mut Option<String>,
) {
    egui::CollapsingHeader::new(RichText::new(format!("{} ({})", shelf.name, shelf.count())).size(12.0))
        .id_salt(format!("shelf:{}", shelf.path))
        .default_open(crate::shelves::opens(&shelf.path))
        .show(ui, |ui| {
            for inner in &shelf.shelves {
                project_shelf(ui, theme, inner, open_project);
            }
            for project in &shelf.items {
                project_row(ui, theme, project, open_project);
            }
        });
}

/// A drawing set in an open project: open it, or take off on a copy.
fn set_row(
    ui: &mut egui::Ui,
    theme: ui::chrome::Theme,
    set: &hub::DrawingSet,
    open_set: &mut Option<(String, String)>,
    copy_set: &mut Option<(String, String)>,
) {
    let response = ui.add(
        egui::Button::new(RichText::new(&set.name))
            .frame(false)
            .min_size(egui::vec2(ui.available_width(), 22.0)),
    );
    if response.clicked() {
        *open_set = Some((set.id.clone(), set.name.clone()));
    }
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "{} sheets · {:.1} MB · {}",
                set.sheets,
                set.bytes as f64 / 1_048_576.0,
                set.uploaded_by
            ))
            .color(theme.faint)
            .size(10.0),
        );
        // Nobody takes off on the set everybody else
        // is reading. One button, and no upload: the
        // copy points at the same file.
        if ui
            .small_button("Take off on a copy")
            .on_hover_text(
                "A copy of this set to mark up, with the sheet numbers and \
                 the scales already on it. The issued set stays clean, and \
                 nothing is uploaded.",
            )
            .clicked()
        {
            *copy_set = Some((set.id.clone(), set.name.clone()));
        }
    });
    ui.separator();
}

/// A folder of a project's drawings, as it is in the job's own files.
/// One thing in a project's folders, for the Studio: a drawing set, a file
/// that isn't a drawing, or a folder, which may hold nothing at all.
enum InFolder<'a> {
    Set(&'a hub::DrawingSet), // WHAT: a drawing set. WHY: opened in Excalibur View.
    File(&'a hub::ProjectFile), // WHAT: any other file. WHY: opened with its own program.
    Folder(&'a str), // WHAT: a folder, by its path. WHY: so an empty one still shows.
}

impl InFolder<'_> {
    /// The folder it is in, or is.
    fn folder(&self) -> Option<&str> {
        match self {
            InFolder::Set(set) => set.folder.as_deref(), // WHAT: the set's folder. WHY: shelved there.
            InFolder::File(file) => file.folder.as_deref(), // WHAT: the file's folder. WHY: likewise.
            InFolder::Folder(path) => Some(path), // WHAT: the folder itself. WHY: makes its shelf.
        }
    }
}

/// How many drawings and files are in a folder, however deep, the folders
/// themselves not counted.
fn things_in(shelf: &crate::shelves::Shelf<'_, InFolder<'_>>) -> usize {
    shelf.items.iter().filter(|t| !matches!(t, InFolder::Folder(_))).count() + shelf.shelves.iter().map(things_in).sum::<usize>() // WHAT: drawings and files only. WHY: an empty folder says (0).
}

/// What drawing a project's folders needs, and what it hands back.
struct FolderView<'a> {
    rules: &'a [hub::FolderInfo], // WHAT: each folder's rule, from the server. WHY: shown on its header.
    groups: &'a [hub::Group], // WHAT: the office's groups. WHY: an administrator picks from them.
    admin: bool, // WHAT: whether this person may change who sees a folder. WHY: only an administrator may.
    changing: &'a mut Option<(String, hub::See)>, // WHAT: the folder whose rule is being picked. WHY: open until Save or Cancel.
    open_set: &'a mut Option<(String, String)>, // WHAT: a set to open. WHY: acted on after drawing.
    copy_set: &'a mut Option<(String, String)>, // WHAT: a set to copy. WHY: likewise.
    open_file: &'a mut Option<(String, String)>, // WHAT: a file to open with its program. WHY: likewise.
    folder_rule: &'a mut Option<(String, hub::See)>, // WHAT: a rule to send. WHY: likewise.
}

impl FolderView<'_> {
    /// The server's word on a folder, case and spacing aside.
    fn rule_of(&self, path: &str) -> Option<&hub::FolderInfo> {
        let plain = |p: &str| p.split('/').map(|x| x.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()).collect::<Vec<_>>().join("/"); // WHAT: a folder as compared. WHY: the server's spelling may differ in case.
        self.rules.iter().find(|f| plain(&f.folder) == plain(path)) // WHAT: its entry. WHY: its rule.
    }

    fn thing(&mut self, ui: &mut egui::Ui, theme: ui::chrome::Theme, thing: &InFolder<'_>) {
        match thing {
            InFolder::Set(set) => set_row(ui, theme, set, self.open_set, self.copy_set), // WHAT: a drawing, as before. WHY: opened in Excalibur View.
            InFolder::File(file) => file_row(ui, theme, file, self.open_file), // WHAT: a file. WHY: opened with its own program.
            InFolder::Folder(_) => {} // WHAT: nothing to draw. WHY: the folder is its shelf.
        }
    }

    fn shelf(&mut self, ui: &mut egui::Ui, theme: ui::chrome::Theme, shelf: &crate::shelves::Shelf<'_, InFolder<'_>>) {
        egui::CollapsingHeader::new(RichText::new(format!("{} ({})", shelf.name, things_in(shelf))).size(12.0))
            .id_salt(format!("folder:{}", shelf.path))
            .default_open(true)
            .show(ui, |ui| {
                self.who_sees(ui, theme, &shelf.path); // WHAT: who sees this folder. WHY: an office can tell at a glance what the field sees.
                for inner in &shelf.shelves {
                    self.shelf(ui, theme, inner); // WHAT: the folders in it. WHY: the tree as the job has it.
                }
                for thing in &shelf.items {
                    self.thing(ui, theme, thing); // WHAT: its drawings, then its files. WHY: drawings first.
                }
                if things_in(shelf) == 0 && shelf.shelves.is_empty() {
                    ui.label(RichText::new("Empty").color(theme.faint).size(10.5)); // WHAT: said. WHY: an empty folder shows as empty, not as missing.
                }
            });
    }

    /// The line under a folder's name: who sees it, and for an administrator
    /// a way to change it.
    fn who_sees(&mut self, ui: &mut egui::Ui, theme: ui::chrome::Theme, path: &str) {
        let Some(info) = self.rule_of(path).cloned() else { return }; // WHAT: the server's word on it. WHY: an older server says nothing, and nothing is shown.
        let picking = self.changing.as_ref().is_some_and(|(p, _)| p == path); // WHAT: whether its rule is being picked. WHY: the picker shows instead.
        if !picking {
            ui.horizontal_wrapped(|ui| {
                let mut said = format!("Who sees it: {}", info.see.words()); // WHAT: the rule in words. WHY: what decides.
                if info.own.is_none() {
                    said.push_str(" (from the folder it is in)"); // WHAT: inherited. WHY: so nobody changes the wrong folder.
                }
                ui.label(RichText::new(said).color(theme.faint).size(10.5)); // WHAT: shown. WHY: as above.
                if let (Some(by), Some(at)) = (&info.set_by, &info.set_at) {
                    let place = if info.set_in.as_deref() == Some("program") { "by the program" } else { "in Excalibur View" }; // WHAT: where it was set. WHY: a change in View lasts until the program sets that folder again.
                    ui.label(RichText::new(format!("set {place}, {by}, {}", at.get(..10).unwrap_or(at))).color(theme.faint).size(10.0)); // WHAT: who and when. WHY: recorded, and shown.
                }
                if self.admin && ui.small_button("Change…").on_hover_text("Choose who sees this folder and everything in it.").clicked() {
                    *self.changing = Some((path.to_string(), info.own.clone().unwrap_or(info.see.clone()))); // WHAT: the picker opened on its rule. WHY: starts from what it is.
                }
            });
            return;
        }
        let Some((_, rule)) = self.changing.as_mut() else { return }; // WHAT: the rule being picked. WHY: changed by the buttons below.
        ui.horizontal_wrapped(|ui| {
            if ui.radio(matches!(rule, hub::See::Everyone), "Everyone on the job").clicked() {
                *rule = hub::See::Everyone; // WHAT: everybody. WHY: picked.
            }
            if ui.radio(matches!(rule, hub::See::Office), "Office only").clicked() {
                *rule = hub::See::Office; // WHAT: administrators and estimators. WHY: picked.
            }
            if ui.radio(matches!(rule, hub::See::Groups(_)), "These groups").clicked() && !matches!(rule, hub::See::Groups(_)) {
                *rule = hub::See::Groups(Vec::new()); // WHAT: groups, none ticked yet. WHY: ticked next.
            }
        });
        if let hub::See::Groups(chosen) = rule {
            if self.groups.is_empty() {
                ui.label(RichText::new("No groups yet: make them in Studio, Office, People.").color(theme.faint).size(10.5)); // WHAT: said. WHY: there is nothing to tick.
            }
            ui.horizontal_wrapped(|ui| {
                for group in self.groups {
                    let mut on = chosen.iter().any(|g| g.eq_ignore_ascii_case(&group.name)); // WHAT: whether it is ticked. WHY: shown.
                    if ui.checkbox(&mut on, &group.name).changed() {
                        chosen.retain(|g| !g.eq_ignore_ascii_case(&group.name)); // WHAT: taken off. WHY: then put back if ticked.
                        if on {
                            chosen.push(group.name.clone()); // WHAT: ticked. WHY: the group sees it.
                        }
                    }
                }
            });
            ui.label(RichText::new("The office always sees it too.").color(theme.faint).size(10.0)); // WHAT: said. WHY: nobody wonders whether they locked themselves out.
        }
        let mut done = false; // WHAT: whether the picker closes. WHY: after Save or Cancel.
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                *self.folder_rule = Some((path.to_string(), rule.clone())); // WHAT: the rule to send. WHY: the server decides, and records who changed it.
                done = true; // WHAT: closes. WHY: sent.
            }
            if ui.button("Cancel").clicked() {
                done = true; // WHAT: closes. WHY: nothing changed.
            }
        });
        if done {
            *self.changing = None; // WHAT: closed. WHY: as above.
        }
    }
}

/// One file that isn't a drawing: its kind, name, size and when it was
/// saved, and **Open with its program**.
fn file_row(ui: &mut egui::Ui, theme: ui::chrome::Theme, file: &hub::ProjectFile, open_file: &mut Option<(String, String)>) {
    let kind = file.name.rsplit_once('.').map(|(_, e)| e.to_uppercase()).unwrap_or_else(|| "FILE".into()); // WHAT: its kind, by its extension. WHY: shown as a file window does.
    let size = if file.bytes >= 1_048_576 { format!("{:.1} MB", file.bytes as f64 / 1_048_576.0) } else { format!("{} KB", file.bytes.div_ceil(1024)) }; // WHAT: its size in words. WHY: shown.
    let saved = file.modified.as_deref().unwrap_or(&file.uploaded); // WHAT: when it was saved, or when it arrived. WHY: shown.
    let row = ui.horizontal(|ui| {
        ui.label(RichText::new(format!("[{kind}]")).color(theme.faint).size(10.0)); // WHAT: its kind. WHY: tells a spreadsheet from a letter.
        ui.label(RichText::new(&file.name).size(12.0)); // WHAT: its name. WHY: shown.
        ui.label(RichText::new(format!("{size} · {}", saved.get(..10).unwrap_or(saved))).color(theme.faint).size(10.0)); // WHAT: size and date. WHY: shown.
        if ui.small_button("Open with its program").on_hover_text(crate::openwith::A_COPY).clicked() {
            *open_file = Some((file.id.clone(), file.name.clone())); // WHAT: asked for. WHY: fetched on the worker.
        }
    });
    if row.response.interact(egui::Sense::click()).double_clicked() {
        *open_file = Some((file.id.clone(), file.name.clone())); // WHAT: a double click does the same. WHY: as in a file window.
    }
}

fn office_section(
    ui: &mut egui::Ui,
    theme: ui::chrome::Theme,
    office: &mut OfficePanel,
    license: Option<&hub::license::Standing>,
    people: &[hub::User],
    me: &str,
    actions: &mut PanelActions,
) {
    let shown = egui::CollapsingHeader::new(RichText::new("Office").color(theme.faint).size(11.0))
        .id_salt("office")
        .show(ui, |ui| {
            // License
            if let Some(license) = license {
                if office.licenses_about.is_none() && license.license_id.is_none() {
                    office.licenses_about = Some(licenses_lying_about());
                }
                license_section(
                    ui,
                    theme,
                    license,
                    &mut office.pasting_license,
                    office.licenses_about.as_deref().unwrap_or(&[]),
                    actions,
                );
                ui.add_space(10.0);
            }

            // Join code
            ui.label(RichText::new("Join code").strong().size(12.0));
            match office.joining.as_ref() {
                Some(joining) if joining.how == "code" && !joining.code.is_empty() => {
                    ui.label(RichText::new(&joining.code).monospace().size(18.0));
                    ui.label(
                        RichText::new(
                            "Anybody in the office opens Excalibur View, picks this server, presses \
                             \"I haven't got an account yet\" and types this.",
                        )
                        .color(theme.faint)
                        .size(10.0),
                    );
                }
                Some(joining) if joining.how == "open" => {
                    ui.label(
                        RichText::new("Anybody who can reach this server can make an account.")
                            .size(11.0),
                    );
                }
                Some(_) => {
                    ui.label(RichText::new("Nobody can make an account by themselves.").size(11.0));
                }
                None => {
                    ui.spinner();
                }
            }
            if ui
                .small_button("Make a new code")
                .on_hover_text(
                    "The old code stops working. It does nothing to the accounts \
                     people already have -- to deal with somebody leaving, remove \
                     them below.",
                )
                .clicked()
            {
                actions.new_code = true;
            }

            // How long the code lasts. One server-wide secret that never
            // expires is the weakest thing about a server with a public
            // address, so it can now be given a date -- and an administrator
            // who wants the old behaviour has to ask for it on purpose.
            if let Some(joining) = office.joining.as_ref() {
                if joining.how == "code" {
                    if joining.run_out {
                        ui.label(
                            RichText::new(
                                "This code has run out. Nobody can join with it until \
                                 you make a new one.",
                            )
                            .color(theme.warn)
                            .size(10.0),
                        );
                    } else if joining.until.is_empty() {
                        ui.label(
                            RichText::new(
                                "This code works forever. If it is ever going to be typed \
                                 from outside the shop, give it a date.",
                            )
                            .color(theme.faint)
                            .size(10.0),
                        );
                    } else {
                        ui.label(
                            RichText::new(format!("Stops working {}", &joining.until[..10.min(joining.until.len())]))
                                .color(theme.faint)
                                .size(10.0),
                        );
                    }
                    ui.horizontal(|ui| {
                        for (label, days) in [("30 days", 30u32), ("90 days", 90), ("Forever", 0)] {
                            if ui.small_button(label).clicked() {
                                actions.code_days = Some(days);
                            }
                        }
                    });
                }
            }

            people_section(ui, theme, office, people, me, actions);
            groups_section(ui, theme, office, people, actions); // WHAT: the groups, under the people. WHY: a folder can be given to a group, so the shop and the field see only theirs.

            // Updates
            ui.add_space(10.0);
            ui.label(RichText::new("Updates").strong().size(12.0));
            if let Some(updates) = office.updates.as_ref() {
                let mut early = updates.channel == "early";
                let was = early;
                ui.radio_value(&mut early, false, "Take new versions when they are released");
                ui.radio_value(&mut early, true, "Get them early, to try them here first");
                if early != was {
                    actions.channel = Some(if early { "early" } else { "stable" }.into());
                }
                let mut lines = vec![format!("This server is version {}.", updates.running)];
                if let Some(offering) = &updates.offering {
                    lines.push(format!("Offering version {offering} to every computer."));
                }
                match (&updates.checked, &updates.note) {
                    (Some(when), Some(note)) => {
                        let day = when.split('T').next().unwrap_or(when);
                        lines.push(format!("Last checked {day}: {note}."));
                    }
                    _ => lines.push("It has not checked for a new version yet.".into()),
                }
                ui.label(RichText::new(lines.join(" ")).color(theme.faint).size(10.0));
            } else {
                ui.spinner();
            }

            // Excalibur Fleet
            ui.add_space(10.0);
            ui.label(RichText::new("Excalibur Fleet").strong().size(12.0));
            match office.fleet.as_ref() {
                Some(fleet) if fleet.on => {
                    ui.label(
                        RichText::new(
                            "Fleet can see this server's health, disk and log, and who is \
                             signed in to it and from which computer — and nothing else: not a \
                             drawing, not a markup.",
                        )
                        .color(theme.faint)
                        .size(10.0),
                    );
                    if ui.small_button("Stop letting Fleet watch").clicked() {
                        actions.stop_fleet = true;
                    }
                }
                Some(_) => {
                    if ui
                        .small_button("Let Excalibur Fleet watch this server")
                        .on_hover_text(
                            "For whoever looks after this company's computers: backups, disk \
                             space and the server's log on one board. Not the drawings.",
                        )
                        .clicked()
                    {
                        actions.watch_fleet = true;
                    }
                }
                None => {}
            }
            if let Some(note) = &office.fleet_note {
                ui.label(RichText::new(note).color(theme.faint).size(10.0));
            }
            if let Some(key) = office.fleet_key.clone() {
                ui.label(RichText::new(&office.fleet_address).monospace().size(11.0));
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&key).monospace().size(11.0));
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(key.clone());
                    }
                });
            }

            // Reaching this server from a jobsite.
            ui.add_space(10.0);
            ui.label(RichText::new("Remote access").strong().size(12.0));
            match office.remote.as_ref() {
                Some(remote) if remote.sealed => {
                    // Not a button that fails. This server was bought to
                    // refuse outward connections and this is one of them.
                    ui.label(RichText::new(&remote.said).color(theme.faint).size(10.0));
                }
                Some(remote) if remote.on || remote.starting => {
                    ui.label(RichText::new(&remote.said).color(theme.faint).size(10.0));
                    if let Some(address) = remote.hostname.clone() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&address).monospace().size(11.0));
                            if ui.small_button("Copy").clicked() {
                                ui.ctx().copy_text(address.clone());
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        if ui.small_button("Turn off").clicked() {
                            actions.remote_off = true;
                        }
                        if ui.small_button("Check again").clicked() {
                            actions.remote_look = true;
                        }
                    });
                }
                Some(remote) => {
                    ui.label(
                        RichText::new(
                            "Seats find this server on the shop network by themselves. From a \
                             truck they cannot, because a broadcast does not cross a router. \
                             A Cloudflare tunnel fixes that, and it is yours: your account, \
                             your hostname, your token. Nothing is opened on your firewall.",
                        )
                        .color(theme.faint)
                        .size(10.0),
                    );
                    if !remote.said.is_empty() && !remote.off {
                        ui.label(RichText::new(&remote.said).color(theme.warn).size(10.0));
                    }
                    ui.add_space(4.0);
                    ui.label(RichText::new("Hostname").size(10.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut office.remote_hostname)
                            .hint_text("drawings.yourshop.com")
                            .desired_width(240.0),
                    );
                    ui.label(RichText::new("Tunnel token").size(10.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut office.remote_token)
                            .password(true)
                            .hint_text("the long one Cloudflare gave you")
                            .desired_width(240.0),
                    );
                    if ui.small_button("Turn on").clicked() {
                        actions.remote_on = Some((
                            office.remote_token.clone(),
                            office.remote_hostname.clone(),
                        ));
                    }
                }
                None => {}
            }
            if let Some(note) = &office.remote_note {
                ui.label(RichText::new(note).color(theme.faint).size(10.0));
            }

            // Other programs: FabWire and anything else that uses the API.
            ui.add_space(10.0);
            ui.label(RichText::new("Other programs").strong().size(12.0));
            ui.label(
                RichText::new(
                    "A key lets another system create projects, put drawing sets up and read \
                     back the markups, as you. No key can run this server: people, roles, keys \
                     and remote access need you signed in. Take a key back here when it is no \
                     longer wanted.",
                )
                .color(theme.faint)
                .size(10.0),
            );
            // WHAT: every kind of key, sync keys included. WHY: an administrator has to be able to see and take back each one a program holds.
            for key in office.keys.iter().filter(|k| ["integration", "assistant", "sync"].contains(&k.purpose.as_str())) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&key.name).size(11.0));
                    ui.label(
                        RichText::new(format!(
                            "{} · {}{}{}",
                            key_kind_words(&key.purpose), // WHAT: what kind of key, in words. WHY: tells assistant, program and sync-only keys apart when deciding which to take back.
                            key.person,
                            key.last_used
                                .as_deref()
                                .map(|u| format!(" · used {}", u.get(..10).unwrap_or(u)))
                                .unwrap_or_default(),
                            key.expires
                                .as_deref()
                                .map(|e| format!(" · ends {}", e.get(..10).unwrap_or(e))) // WHAT: the day a key with an end stops. WHY: so nobody is surprised when it does.
                                .unwrap_or_default()
                        ))
                        .color(theme.faint)
                        .size(10.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Take back").clicked() {
                            actions.revoke_key = Some(key.id.clone());
                        }
                    });
                });
            }
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut office.key_name)
                        .hint_text("FabWire")
                        .desired_width(140.0),
                );
                let name = if office.key_name.trim().is_empty() {
                    "FabWire".to_string()
                } else {
                    office.key_name.trim().to_string()
                };
                if ui.small_button("Make a key").on_hover_text("A key that can use the API as you: projects, drawing sets and markups, for a program you trust with all of that.").clicked() {
                    actions.make_key = Some(name.clone()); // WHAT: an integration key. WHY: as before.
                }
                if ui.small_button("Make a sync key").on_hover_text(SYNC_KEY_WORDS).clicked() {
                    actions.make_sync_key = Some(name); // WHAT: a sync key with the same name box. WHY: the narrower key for a program that only keeps its projects in step.
                }
            });
            ui.label(
                RichText::new("A sync key is narrower than a key: use it for a program that only keeps its own projects in step with this server.")
                    .color(theme.faint)
                    .size(10.0),
            ); // WHAT: which button is for what, in one line. WHY: so the narrower key is the one chosen when it will do.
            if let Some(made) = office.new_key.clone() {
                let key = made.key.clone().unwrap_or_default();
                ui.label(
                    RichText::new(format!(
                        "The key for {} ({}). It is shown this once — copy it into {}'s settings now.",
                        made.name, key_kind_words(&made.purpose), made.name
                    ))
                    .color(theme.warn)
                    .size(10.0),
                ); // WHAT: the key, what kind it is, and that it is shown once. WHY: there is no second look.
                if made.purpose == SYNC_PURPOSE {
                    ui.label(RichText::new(SYNC_KEY_WORDS).color(theme.faint).size(10.0)); // WHAT: what a sync key can and cannot do. WHY: plain words for whoever sets the program up.
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&key).monospace().size(11.0));
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(key.clone());
                    }
                    if ui.small_button("Done").clicked() {
                        office.new_key = None;
                    }
                });
                ui.label(
                    RichText::new(format!("Server address: {}", office.base))
                        .color(theme.faint)
                        .size(10.0),
                );
            }

            // Being told rather than asking: FabWire hears when a takeoff changes.
            ui.add_space(8.0);
            ui.label(RichText::new("Change notices").strong().size(11.0));
            ui.label(
                RichText::new(
                    "Tell another program whenever a drawing set's markups change, so it \
                     doesn't have to keep asking. Each notice says which set and job changed, \
                     signed so the program knows it came from here. No drawing or markup goes \
                     with it.",
                )
                .color(theme.faint)
                .size(10.0),
            );
            for notice in &office.notices {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&notice.url).size(11.0));
                    let how = match (&notice.last_status, &notice.last_at) {
                        (Some(status), Some(at)) => format!("{status} · {}", at.get(..16).unwrap_or(at)),
                        _ => "nothing sent yet".into(),
                    };
                    ui.label(RichText::new(how).color(theme.faint).size(10.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Stop").clicked() {
                            actions.remove_notice = Some(notice.id.clone());
                        }
                        if ui.small_button("Test").clicked() {
                            actions.test_notice = Some(notice.id.clone());
                        }
                    });
                });
            }
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut office.notice_url)
                        .hint_text("https://fabwire.example/hooks/excalibur")
                        .desired_width(260.0),
                );
                let url = office.notice_url.trim().to_string();
                if ui.add_enabled(!url.is_empty(), egui::Button::new("Add").small()).clicked() {
                    actions.add_notice = Some(url);
                }
            });
            if let Some(note) = &office.notice_note {
                ui.label(RichText::new(note).color(theme.faint).size(10.0));
            }
            if let Some(made) = office.new_notice.clone() {
                let secret = made.secret.clone().unwrap_or_default();
                ui.label(
                    RichText::new(
                        "The secret its notices are signed with. It is shown this once — copy \
                         it into the other program's settings now.",
                    )
                    .color(theme.warn)
                    .size(10.0),
                );
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&secret).monospace().size(11.0));
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(secret.clone());
                    }
                    if ui.small_button("Done").clicked() {
                        office.new_notice = None;
                    }
                });
            }
        });
    if shown.body_returned.is_some() && !office.asked {
        office.asked = true;
        actions.office = true;
        actions.people = true;
    }
    if shown.header_response.clicked() && shown.body_returned.is_none() {
        // Closed again. Next time it opens it asks again, so the code, the
        // update status and who is on the server are never a day stale.
        office.asked = false;
        office.removing = None;
    }
}

/// The places an emailed or carried license actually lands. Nobody files a
/// license somewhere sensible; it is in Downloads, or on the desktop, or on
/// the stick it was walked over on.
fn places_a_license_lands() -> Vec<(std::path::PathBuf, String)> {
    let mut places = Vec::new();
    if let Some(dirs) = directories::UserDirs::new() {
        if let Some(d) = dirs.download_dir() {
            places.push((d.to_path_buf(), "Downloads".to_string()));
        }
        if let Some(d) = dirs.desktop_dir() {
            places.push((d.to_path_buf(), "the desktop".to_string()));
        }
        if let Some(d) = dirs.document_dir() {
            places.push((d.to_path_buf(), "Documents".to_string()));
        }
    }
    // A stick carried from the computer that signs licenses to the one that
    // runs the server. C: is skipped: walking a whole system drive to find a
    // file nobody put there is not worth anybody's disk.
    #[cfg(windows)]
    for letter in 'D'..='Z' {
        let root = std::path::PathBuf::from(format!("{letter}:\\"));
        if root.is_dir() {
            places.push((root, format!("{letter}:")));
        }
    }
    places
}

/// Licenses sitting in those places, newest first. Only the name and the
/// company are read here — whether one is genuine is the server's to say.
fn licenses_lying_about() -> Vec<FoundLicense> {
    licenses_in(&places_a_license_lands())
}

fn licenses_in(places: &[(std::path::PathBuf, String)]) -> Vec<FoundLicense> {
    let mut found: Vec<(std::time::SystemTime, FoundLicense)> = Vec::new();
    for (place, where_from) in places {
        let Ok(entries) = std::fs::read_dir(place) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some(hub::license::EXTENSION) {
                continue;
            }
            // Big enough to be a license is small: anything larger is not one,
            // and is not worth reading off a stick to find out.
            let when = match entry.metadata() {
                Ok(m) if m.len() <= 8 * 1024 => m.modified().unwrap_or(std::time::UNIX_EPOCH),
                _ => continue,
            };
            let company = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|v| v.get("company")?.as_str().map(str::to_string))
                .unwrap_or_default();
            if company.is_empty() {
                continue;
            }
            found.push((when, FoundLicense { path, company, where_from: where_from.clone() }));
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.dedup_by(|a, b| a.1.company == b.1.company);
    found.into_iter().take(3).map(|(_, f)| f).collect()
}

/// Studio → Office → License: what the office has, and the three things an
/// administrator can do about it.
fn license_section(
    ui: &mut egui::Ui,
    theme: ui::chrome::Theme,
    license: &hub::license::Standing,
    pasting: &mut Option<String>,
    lying_about: &[FoundLicense],
    actions: &mut PanelActions,
) {
    ui.label(RichText::new("License").strong().size(12.0));
    ui.label(RichText::new(license.about_line()).size(11.0));
    let mut lines: Vec<String> = Vec::new();
    match license.state.as_str() {
        "licensed" | "founding" => {
            lines.push(format!("{} people have accounts on this server.", license.users));
            if license.state == "licensed" {
                lines.push(
                    "If updates are not renewed, everything keeps working on the version you \
                     have; new versions stop."
                        .into(),
                );
            }
        }
        "trial" => lines.push(
            "Everything works during the trial. After it, nothing is deleted: drawings and \
             markups stay readable and exportable, and sharing new work pauses until a license \
             is added."
                .into(),
        ),
        _ => {}
    }
    for line in lines {
        ui.label(RichText::new(line).color(theme.faint).size(10.0));
    }
    if !crate::edition::sells() {
        ui.label(
            RichText::new("The office's license is added on the server, or from Excalibur View on Windows.")
                .color(theme.faint)
                .size(10.0),
        );
        return;
    }
    // Before asking anybody to go and find a file: the one they were sent is
    // almost certainly already on this computer.
    for found in lying_about {
        ui.horizontal_wrapped(|ui| {
            if ui
                .small_button(format!("Add the license for {}", found.company))
                .on_hover_text(found.path.display().to_string())
                .clicked()
            {
                actions.use_found_license = Some(found.path.clone());
            }
            ui.label(
                RichText::new(format!("found in {}", found.where_from))
                    .color(theme.faint)
                    .size(10.0),
            );
        });
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .small_button("Add license file…")
            .on_hover_text("The .evlicense file that came by email. Checked on this server; nothing is sent anywhere.")
            .clicked()
        {
            actions.add_license = true;
        }
        // The same license, for whoever would rather not go hunting through
        // Downloads for an attachment. The server takes it as text either way.
        if ui
            .small_button(if pasting.is_some() { "Cancel paste" } else { "Paste license…" })
            .on_hover_text("Paste the license out of the email instead of saving the file first.")
            .clicked()
        {
            actions.paste_license = true;
        }
        if license.state != "founding" {
            ui.hyperlink_to(
                RichText::new(if license.state == "licensed" { "Renew or add users" } else { "Buy Office" }).size(11.0),
                // The program's own link rather than the server's: a server
                // from before the website moved names an old address. For a
                // licence it carries which one, so the purchase lands on it.
                if license.state == "licensed" {
                    hub::site::add_url(license.license_id.as_deref())
                } else {
                    hub::site::BUY.to_string()
                },
            );
        }
        if let Some(id) = &license.license_id {
            if ui.small_button("Copy license id").on_hover_text(id).clicked() {
                ui.ctx().copy_text(id.clone());
            }
        }
    });
    if let Some(text) = pasting.as_mut() {
        ui.add_space(4.0);
        ui.label(
            RichText::new("Paste everything between the curly brackets, brackets and all.")
                .color(theme.faint)
                .size(10.0),
        );
        ui.add(
            egui::TextEdit::multiline(text)
                .desired_rows(5)
                .desired_width(f32::INFINITY)
                .font(egui::TextStyle::Monospace)
                .hint_text("{ \"id\": \"evl_…\", \"company\": … }"),
        );
        let ready = !text.trim().is_empty();
        ui.horizontal(|ui| {
            if ui.add_enabled(ready, egui::Button::new("Add this license").small()).clicked() {
                actions.send_license = Some(text.clone());
            }
            if !ready {
                ui.label(RichText::new("Nothing pasted yet.").color(theme.faint).size(10.0));
            }
        });
    }
}

impl App {
    /// Asks for a license file and hands it to the server.
    pub fn pick_license(&mut self) {
        if !crate::edition::sells() {
            self.status = crate::edition::not_in_this_copy("Adding a license");
            return;
        }
        self.filing.one(
            crate::files::Choose::open()
                .title("Add an Excalibur View Office license")
                .filter("License", &[hub::license::EXTENSION]),
            |app, file| app.ask(Ask::AddLicense(file)),
        );
    }
}

/// Changing your own password.
fn password_section(
    ui: &mut egui::Ui,
    theme: ui::chrome::Theme,
    office: &mut OfficePanel,
    actions: &mut PanelActions,
) {
    egui::CollapsingHeader::new(RichText::new("My password").color(theme.faint).size(11.0))
        .id_salt("my-password")
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut office.current_password)
                    .password(true)
                    .hint_text("Current password")
                    .desired_width(f32::INFINITY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut office.new_password)
                    .password(true)
                    .hint_text("New password, twelve characters or more")
                    .desired_width(f32::INFINITY),
            );
            let ready = !office.current_password.is_empty()
                && office.new_password.chars().count() >= 12;
            if ui.add_enabled(ready, egui::Button::new("Change it")).clicked() {
                actions.password = Some((
                    std::mem::take(&mut office.current_password),
                    std::mem::take(&mut office.new_password),
                ));
            }
            if let Some(note) = &office.password_note {
                ui.label(RichText::new(note).color(theme.faint).size(10.0));
            }
        });
}

/// A tick, drawn. Two strokes, so it reads at any size and in any font.
fn tick(painter: &egui::Painter, rect: egui::Rect, colour: Color32) {
    let c = rect.center();
    let s = rect.width() * 0.34;
    let stroke = egui::Stroke::new((rect.width() * 0.13).max(1.2), colour);
    painter.line_segment(
        [
            egui::pos2(c.x - s, c.y + s * 0.1),
            egui::pos2(c.x - s * 0.25, c.y + s * 0.8),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x - s * 0.25, c.y + s * 0.8),
            egui::pos2(c.x + s, c.y - s * 0.8),
        ],
        stroke,
    );
}

/// An arrow pointing back the way you came.
fn arrow_back(painter: &egui::Painter, rect: egui::Rect, colour: Color32) {
    let c = rect.center();
    let s = rect.width() * 0.34;
    let stroke = egui::Stroke::new((rect.width() * 0.12).max(1.2), colour);
    painter.line_segment([egui::pos2(c.x - s, c.y), egui::pos2(c.x + s, c.y)], stroke);
    painter.line_segment(
        [
            egui::pos2(c.x - s, c.y),
            egui::pos2(c.x - s * 0.15, c.y - s * 0.7),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(c.x - s, c.y),
            egui::pos2(c.x - s * 0.15, c.y + s * 0.7),
        ],
        stroke,
    );
}

fn role_name(role: hub::Role) -> &'static str {
    match role {
        hub::Role::Admin => "administrator",
        hub::Role::Estimator => "estimator",
        hub::Role::Viewer => "viewer",
    }
}

// ---- the pieces the connect window is made of ------------------------------

fn heading(step: &Step) -> &'static str {
    match step {
        Step::Choosing => "Your company's Excalibur View server",
        Step::SigningIn => "Sign in",
        Step::SettingUp => "Set this server up",
        Step::Joining => "Make yourself an account",
        Step::JustSetUp => "That's it — you're set up",
    }
}

fn explanation(step: &Step) -> &'static str {
    match step {
        Step::Choosing => "Drawings, shared tool chests and takeoffs, in one place.",
        Step::SigningIn => "With the email address and password you already have.",
        Step::SettingUp => "Nobody has set this one up yet, so you will be its administrator.",
        Step::Joining => "You will need the join code from whoever set the server up.",
        Step::JustSetUp => "Here is what to tell everybody else in the office.",
    }
}

fn button(step: &Step) -> &'static str {
    match step {
        Step::SettingUp => "Set it up",
        Step::Joining => "Create my account",
        _ => "Sign in",
    }
}

fn ready_to_go(signing: &SigningIn) -> bool {
    if signing.waiting || signing.base.trim().is_empty() {
        return false;
    }
    let typed = !signing.email.trim().is_empty() && !signing.password.is_empty();
    match signing.step {
        Step::SigningIn => typed,
        Step::SettingUp => {
            typed && !signing.person.trim().is_empty() && !signing.company.trim().is_empty()
        }
        Step::Joining => {
            typed
                && !signing.person.trim().is_empty()
                && (signing.joining == "open" || !signing.code.trim().is_empty())
        }
        _ => false,
    }
}

/// The address, always visible. Somebody about to type a password should be
/// able to see where it is going — including when they picked the server off a
/// list rather than typing it.
fn where_it_is(ui: &mut egui::Ui, theme: ui::chrome::Theme, signing: &SigningIn) {
    ui.horizontal(|ui| {
        let (mark, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
        tick(ui.painter(), mark, theme.accent_text);
        let said = match (&signing.found, &signing.found_version) {
            (Some(name), Some(version)) => format!("{name} · version {version}  ·  {}", signing.base),
            (Some(name), None) => format!("{name}  ·  {}", signing.base),
            (None, _) => signing.base.clone(),
        };
        ui.label(RichText::new(said).color(theme.accent_text).size(11.0));
    });
    if let Some(standing) = &signing.standing {
        ui.add_space(4.0);
        ui.label(RichText::new(standing).color(theme.faint).size(10.0));
    }
}

fn field(ui: &mut egui::Ui, theme: ui::chrome::Theme, label: &str, into: &mut String, hint: &str) {
    ui.label(RichText::new(label).color(theme.faint).size(11.0));
    ui.add(
        egui::TextEdit::singleline(into)
            .hint_text(hint)
            .desired_width(f32::INFINITY),
    );
}

/// Returns true when Enter was pressed in it, because a password box is the
/// last one on every one of these forms and Enter is what people press.
fn secret(ui: &mut egui::Ui, theme: ui::chrome::Theme, label: &str, into: &mut String) -> bool {
    ui.label(RichText::new(label).color(theme.faint).size(11.0));
    let box_ = ui.add(
        egui::TextEdit::singleline(into)
            .password(true)
            .desired_width(f32::INFINITY),
    );
    box_.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

fn note(ui: &mut egui::Ui, theme: ui::chrome::Theme, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).color(theme.faint).size(10.0));
}

/// Picking a server: what answered on the network, an address typed by hand, or
/// this computer volunteering to be one.
fn choosing(
    ui: &mut egui::Ui,
    theme: ui::chrome::Theme,
    signing: &mut SigningIn,
    look: &mut bool,
    pick: &mut Option<String>,
    check: &mut bool,
    host_here: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("On this network").color(theme.faint).size(11.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(!signing.looking, egui::Button::new(RichText::new("Look again").size(10.0)).frame(false))
                .clicked()
            {
                *look = true;
            }
        });
    });
    ui.add_space(4.0);

    if signing.looking {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("Looking…").color(theme.faint).size(11.0));
        });
    } else if signing.servers.is_empty() {
        ui.label(
            RichText::new(if signing.looked {
                "No Excalibur View server on this network yet. This is not your file server — \
                 it is an Excalibur View one, and somebody has to set a machine up as it. \
                 Double-click Hyperview-Server.exe on whichever machine should hold \
                 the drawings, or type its address below if it is on another network."
            } else {
                "Press Look again to see what is on this network."
            })
            .color(theme.faint)
            .size(11.0),
        );
    } else {
        for found in &signing.servers {
            let line = found.as_line();
            let response = ui.add(
                egui::Button::new(RichText::new(line).size(12.0))
                    .frame(true)
                    .min_size(egui::vec2(ui.available_width(), 26.0)),
            );
            if response.clicked() {
                *pick = Some(found.answer.url.clone());
            }
        }
    }

    ui.add_space(12.0);
    ui.label(RichText::new("Or its address").color(theme.faint).size(11.0));
    let address = ui.add(
        egui::TextEdit::singleline(&mut signing.base)
            .hint_text("https://drawings.yourcompany.com")
            .desired_width(f32::INFINITY),
    );
    if address.changed() {
        signing.found = None;
        signing.found_version = None;
    }
    if address.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
        && !signing.base.trim().is_empty()
    {
        *check = true;
    }

    if !signing.joined_note.trim().is_empty() {
        ui.add_space(6.0);
        ui.label(RichText::new(&signing.joined_note).color(theme.faint).size(11.0));
    }

    ui.add_space(14.0);
    ui.separator();
    ui.add_space(8.0);
    if crate::platform::tablet() {
        // A tablet joins a server; it never is one. It sleeps, it leaves the
        // building, and the office would lose its drawings with it.
        ui.label(
            RichText::new(
                "An office's Excalibur View server is set up once, from Excalibur View on \
                 Windows or a Mac, on whichever machine keeps the drawings. Ask whoever \
                 looks after it for its address.",
            )
            .color(theme.faint)
            .size(11.0),
        );
        return;
    }
    ui.label(
        RichText::new("Haven't got a server yet?")
            .color(theme.faint)
            .size(11.0),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(
            "The drawings should live on whatever machine the company already keeps \
             things on. Double-click Hyperview-Server.exe there once, and every desk \
             finds it by itself.",
        )
        .color(theme.faint)
        .size(10.0),
    );
    ui.add_space(6.0);
    if ui
        .add(egui::Button::new(RichText::new("Or host them on this computer").size(11.0)))
        .on_hover_text(
            "For a shop with no server, or for trying it out. This machine serves the \
             drawings, and has to be on for anybody else to open them — which is why a \
             real server is the better answer when there is one.",
        )
        .clicked()
    {
        *host_here = true;
    }
}

/// What to tell the office, once a server has been set up.
fn just_set_up(ui: &mut egui::Ui, theme: ui::chrome::Theme, signing: &SigningIn) {
    ui.label(RichText::new(&signing.company).strong().size(15.0));
    if let Some(standing) = &signing.standing {
        ui.add_space(4.0);
        ui.label(RichText::new(standing).color(theme.faint).size(11.0));
    }
    ui.add_space(12.0);

    ui.label(RichText::new("Everybody else").color(theme.faint).size(11.0));
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "They open Excalibur View, the server is already in the list, and they press \
             \"I haven't got an account yet\". Then they type this:",
        )
        .size(11.0),
    );
    ui.add_space(8.0);

    // Selectable, because it is going to be copied into an email or read out
    // across an office, and a code somebody has to retype from a screenshot is
    // a code somebody gets wrong.
    let mut code = signing.join_code.clone();
    ui.add(
        egui::TextEdit::singleline(&mut code)
            .font(egui::TextStyle::Monospace)
            .desired_width(f32::INFINITY),
    );
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "You can change this any time, which is what to do when somebody leaves. \
             It decides who may ask for an account, not what protects one.",
        )
        .color(theme.faint)
        .size(10.0),
    );
    ui.add_space(10.0);
    ui.label(RichText::new("The address, if they ever need it").color(theme.faint).size(11.0));
    let mut address = signing.base.clone();
    ui.add(
        egui::TextEdit::singleline(&mut address)
            .font(egui::TextStyle::Monospace)
            .desired_width(f32::INFINITY),
    );
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn a_sync_key_is_asked_for_by_the_servers_word_and_labelled_sync_only() {
        assert_eq!(SYNC_PURPOSE, "sync"); // WHAT: the word the server takes. WHY: POST /api/v1/me/keys {"purpose":"sync"}; any other word makes a different key or none.
        assert_eq!(key_kind_words(SYNC_PURPOSE), "program, sync only"); // WHAT: how it is shown. WHY: told apart from a full program key.
        assert_eq!(key_kind_words("integration"), "program"); // WHAT: an integration key. WHY: unchanged.
        assert_eq!(key_kind_words("assistant"), "assistant"); // WHAT: an assistant key. WHY: unchanged.
        for limit in ["cannot see projects somebody made by hand", "cannot add people", "only while the person who made it is"] {
            assert!(SYNC_KEY_WORDS.contains(limit), "the words leave out: {limit}"); // WHAT: each limit said. WHY: whoever hands the key to a program should know what it cannot do.
        }
    }

    fn write(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::File::create(&path).unwrap().write_all(body.as_bytes()).unwrap();
        path
    }

    fn a_license(company: &str) -> String {
        format!(
            r#"{{"id":"evl_1","company":"{company}","edition":"office","users":6,
                "updates_through":"2027-09-22","issued":"2026-09-22","key":"mesafab-2026",
                "signature":"00"}}"#
        )
    }

    fn a_folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hv-found-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_license_left_in_downloads_is_offered() {
        let dir = a_folder("downloads");
        write(&dir, "Example Builders.evlicense", &a_license("Example Builders"));
        let found = licenses_in(&[(dir.clone(), "Downloads".into())]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].company, "Example Builders");
        assert_eq!(found[0].where_from, "Downloads");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_license_carried_over_on_a_stick_is_offered_too() {
        let stick = a_folder("stick");
        write(&stick, "Example Steel, Inc.evlicense", &a_license("Example Steel, Inc."));
        let found = licenses_in(&[(stick.clone(), "E:".into())]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].where_from, "E:");
        let _ = std::fs::remove_dir_all(&stick);
    }

    #[test]
    fn nothing_else_in_the_folder_is_mistaken_for_one() {
        let dir = a_folder("mixed");
        write(&dir, "Structural.pdf", "%PDF-1.7");
        write(&dir, "notes.txt", "call the shop");
        write(&dir, "broken.evlicense", "this is not json");
        write(&dir, "nameless.evlicense", r#"{"id":"evl_2","users":3}"#);
        assert!(licenses_in(&[(dir.clone(), "Downloads".into())]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_company_twice_is_offered_once() {
        let dir = a_folder("twice");
        write(&dir, "Example Builders.evlicense", &a_license("Example Builders"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        let newer = write(&dir, "Example Builders (1).evlicense", &a_license("Example Builders"));
        let found = licenses_in(&[(dir.clone(), "Downloads".into())]);
        assert_eq!(found.len(), 1, "one company, one offer");
        assert_eq!(found[0].path, newer, "the one they were sent most recently");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_that_is_not_there_is_not_a_problem() {
        let missing = std::env::temp_dir().join("hv-no-such-folder-at-all");
        assert!(licenses_in(&[(missing, "Downloads".into())]).is_empty());
    }

    #[test]
    fn something_far_too_big_is_left_alone() {
        // A stick with a disk image on it that happens to end .evlicense
        // should not be read into memory to find out.
        let dir = a_folder("big");
        write(&dir, "huge.evlicense", &"x".repeat(9 * 1024));
        assert!(licenses_in(&[(dir.clone(), "E:".into())]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}


/// How a project reads in the list: the name first, and the job number only
/// when it is one.
///
/// A bid pushed from FabWire without a job number used to arrive named after
/// its internal id -- `J-1776359550436`, a millisecond timestamp wearing a J
/// -- and the list is sorted and scanned by that column. A number nobody
/// recognises is worse than no number: it fills the space where the thing
/// somebody is looking for should be.
fn how_it_reads(number: &str, name: &str) -> String {
    let number = number.trim();
    let name = name.trim();
    if name.is_empty() {
        return number.to_string();
    }
    if number.is_empty() || looks_made_up(number) {
        return name.to_string();
    }
    format!("{number}  {name}")
}

/// A "number" that is really an internal id: a long run of digits, with or
/// without a letter and a dash in front of it.
///
/// Job numbers are short and people say them out loud. Nobody has a job 2575
/// and a job 1776359550436 in the same shop.
fn looks_made_up(number: &str) -> bool {
    let digits = number.trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '-');
    digits.len() >= 10 && digits.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod what_a_project_is_called {
    use super::{how_it_reads, looks_made_up};

    #[test]
    fn a_real_job_number_leads() {
        assert_eq!(how_it_reads("2575", "Hilltop Water Tower"), "2575  Hilltop Water Tower");
    }

    #[test]
    fn a_timestamp_wearing_a_j_is_dropped() {
        assert_eq!(how_it_reads("J-1776359550436", "Elm Theater"), "Elm Theater");
        assert_eq!(how_it_reads("1776359550436", "Elm Theater"), "Elm Theater");
    }

    #[test]
    fn no_number_at_all_is_fine() {
        assert_eq!(how_it_reads("", "Elm Theater"), "Elm Theater");
        assert_eq!(how_it_reads("   ", "Elm Theater"), "Elm Theater");
    }

    #[test]
    fn a_project_with_only_a_number_still_shows_it() {
        assert_eq!(how_it_reads("2575", ""), "2575");
    }

    #[test]
    fn short_numbers_are_never_mistaken_for_ids() {
        // Shops really do use these shapes.
        for real in ["2575", "JOB-2575", "24-114", "J-1042", "2026-07"] {
            assert!(!looks_made_up(real), "{real} should read as a job number");
        }
        for made_up in ["1776359550436", "J-1776359550436", "hv1727041234567"] {
            assert!(looks_made_up(made_up), "{made_up} should not");
        }
    }
}
