//! Talking to a Hyperview server, without ever making the window wait.
//!
//! Everything here happens on its own thread and reports back through a
//! channel. A drawing set is sixty megabytes and a shop's network is a shop's
//! network; a viewer that stops painting while a file comes down is a viewer
//! somebody stops using.
//!
//! The whole thing is optional. Hyperview opens a drawing off a disk and saves
//! markups into it with no server anywhere, and a server that has gone away
//! mid-session costs the user nothing but the sync.

use std::path::PathBuf;
use std::time::Duration;
use std::sync::Arc;

use crossbeam_channel::{unbounded, Receiver, Sender};
use hub::update::{Channel, Release, UpdateOffer};
use hub::{Chest, Client, DrawingSet, Project, Session, Takeoff, User};

/// What the window asks for.
pub enum Ask {
    /// Check an address is a Hyperview server before anybody types a password
    /// into it, so a wrong address fails at the address.
    Check(String),
    /// Shout on the local network and see what answers. This is what saves an
    /// estimator having to find somebody who knows an IP address.
    Look,
    /// Set up a server nobody has set up yet, and be its administrator.
    Claim {
        base: String,
        setup: Box<hub::Setup>,
    },
    /// Make an account on somebody else's server with their join code.
    Join {
        base: String,
        join: Box<hub::Join>,
    },
    SignIn {
        base: String,
        email: String,
        password: String,
    },
    /// Sign in again with a token kept from last time.
    Resume {
        base: String,
        token: String,
        /// Where the same server answers from outside the shop, if it ever
        /// said. Tried only when the shop's own address does not answer.
        reachable_at: Option<String>,
    },
    SignOut,
    /// How this server can be reached from a jobsite, and turning it on or off.
    Remote,
    RemoteOn { token: String, hostname: String },
    RemoteOff,
    Projects,
    /// Everybody with an account on this server. Administrators only.
    People,
    /// Give somebody an account.
    AddPerson {
        name: String,
        email: String,
        password: String,
        role: hub::Role,
    },
    /// Change what somebody is allowed to do.
    ChangeRole {
        id: String,
        role: hub::Role,
    },
    /// Take somebody's account away.
    RemovePerson {
        id: String,
        name: String,
    },
    Sets(String),
    /// Fetch a drawing into the cache and open it.
    Open {
        set: String,
        name: String,
    },
    Takeoff(String),
    /// Send markups up, and bring down whatever has happened since.
    Sync {
        set: String,
        since: u64,
        /// Each one as (page, the annotation dictionary base64).
        sending: Vec<(usize, String)>,
        /// Asked by the clock rather than by somebody: a failure is written
        /// in the log, not put in front of anybody every few seconds.
        quiet: bool,
    },
    Chests,
    /// Pull a shared tool chest down beside the program.
    FetchChest {
        id: String,
        name: String,
    },
    /// What plugins the office hands out.
    Plugins,
    /// Pull one of the office's plugins down. It is checked when it is
    /// loaded, not here.
    FetchPlugin {
        id: String,
        name: String,
    },
    /// Hand a plugin file to the whole office. Administrators only.
    SharePlugin(PathBuf),
    /// Stop handing a plugin out. Administrators only.
    RemovePlugin(String),
    /// Have the server run one of the office's plugins, for a copy that
    /// doesn't run plugins itself.
    RunPlugin { id: String, input: plugin_api::Input },
    /// An administrator making a key for another system, such as FabWire.
    MakeIntegrationKey(String),
    /// An administrator making a sync key: for a program that only keeps its
    /// own projects in step with the server.
    MakeSyncKey(String), // WHAT: asks for a key of purpose "sync". WHY: the narrower key a syncing program should hold, which Studio could not make before.
    RevokeKey(String),
    /// Tell another program at this address whenever a takeoff changes.
    AddNotice(String),
    /// Send that address a test notice now.
    TestNotice(String),
    RemoveNotice(String),
    /// Make an assistant key for this computer and put Hyperview on the assistant
    /// app's list of connectors.
    ConnectAssistant { computer: String },
    /// Whether a newer version has been put where this program runs from
    /// while it was open — somebody double-clicked a newer download. Nothing
    /// is asked of any server.
    NoticeInstalled,
    CheckForUpdate {
        running: String,
        channel: Channel,
        /// Somebody pressed "Check for Updates" and is waiting to be told,
        /// whatever the answer is. Otherwise only news is news.
        asked: bool,
    },
    /// Start a project on the server.
    NewProject {
        number: String,
        name: String,
    },
    /// Put drawing sets from this computer on the server, in a project.
    UploadSets {
        project: String,
        files: Vec<PathBuf>,
    },
    /// Share a tool chest from this computer with the whole office.
    ShareChest(PathBuf),
    /// What an administrator looks after: the join code, updates and Fleet.
    Office,
    /// Where the office stands with Excalibur View Office.
    License,
    /// Add a license file to the office server. Administrators only.
    /// A copy of a drawing set to take off on, leaving the issued set clean.
    CopySet {
        set: String,
        name: String,
    },
    AddLicense(PathBuf),
    /// The same license, pasted rather than picked off the disk.
    AddLicenseText(String),
    /// Let Excalibur Fleet watch this server, handing it the key directly
    /// when Fleet runs on this computer.
    WatchWithFleet {
        name: String,
        base: String,
    },
    StopFleet,
    /// Throw the join code away and make a new one.
    NewJoinCode,
    /// How many days the join code lasts from now. 0 means forever.
    JoinCodeDays(u32),
    /// `stable` or `early`.
    UpdateChannel(String),
    ChangePassword {
        current: String,
        new: String,
    },
    Quit,
    /// A file that isn't a drawing: save a copy and open it with its program.
    OpenFile {
        file: String, // WHAT: the file's id. WHY: downloaded by it.
        name: String, // WHAT: its name. WHY: the copy keeps it, so the system knows its kind.
    },
    /// Say who sees one folder of a project. Administrators only.
    SetFolderRule {
        project: String, // WHAT: the project. WHY: the folder is one of its.
        folder: String, // WHAT: the folder. WHY: the one changed.
        see: hub::See, // WHAT: the new rule. WHY: sent.
    },
    /// Every group of people. Administrators only.
    Groups,
    /// Make a group.
    MakeGroup(String),
    /// Put somebody in a group, or take them out.
    ChangeGroup {
        group: String, // WHAT: the group's id. WHY: the one changed.
        person: String, // WHAT: the person's id. WHY: who.
        on: bool, // WHAT: in or out. WHY: one ask for both.
    },
}

/// What comes back.
pub enum Told {
    /// A drawing set was copied to take off on.
    Copied {
        /// What the set it came from is called.
        name: String,
        copy: Box<hub::DrawingSet>,
    },
    /// The address is a server, and this is what it calls itself.
    Found {
        base: String,
        name: String,
        version: String,
        /// Where the same server answers from outside the shop, when it has
        /// turned remote access on and checked that the address works.
        reachable_at: Option<String>,
        /// False on a server nobody has set up yet, which is the one to offer
        /// to set up rather than to ask somebody to sign in to.
        claimed: bool,
        /// How somebody without an account gets one: code, open or closed.
        joining: String,
    },
    /// What answered on the local network.
    Servers(Vec<hub::discover::Found>),
    /// A server has been set up, and this is the code for everybody else.
    Claimed {
        base: String,
        company: String,
        join_code: String,
    },
    /// Markups reached the server, but reading the set back afterwards did
    /// not. The push happened, so the window has to be told which names went
    /// -- otherwise the next sync sends them a second time and the server
    /// ends up holding two of each.
    Pushed {
        set: String,
        sent: Vec<String>,
    },
    SignedIn(Box<Session>),
    SignedOut,
    /// What the server says about reaching it from outside the shop.
    RemoteIs(Box<hub::Remote>),
    Projects(Vec<Project>),
    /// Everybody with an account here.
    People(Vec<hub::User>),
    /// What just happened to somebody's account, and the list as it stands
    /// afterwards -- sent together, so there is never a moment where the
    /// screen says somebody was removed and still shows their row.
    PeopleChanged {
        people: Vec<hub::User>,
        said: String,
    },
    Sets(Vec<DrawingSet>),
    /// The drawing is on this machine and ready to open.
    Downloaded {
        set: String,
        path: PathBuf,
        /// It was already here, so nothing came down the wire.
        cached: bool,
    },
    Progress {
        what: String,
    },
    Takeoff(Box<Takeoff>),
    /// What the server had, and where this machine is now up to.
    Synced {
        set: String,
        revision: u64,
        /// (name, page, dictionary base64, removed)
        markups: Vec<(String, usize, String, bool)>,
        /// Names this machine just sent, so it does not send them again.
        sent: Vec<String>,
    },
    Chests(Vec<Chest>),
    ChestFetched {
        name: String,
        path: PathBuf,
    },
    Plugins(Vec<hub::PluginInfo>),
    PluginFetched {
        name: String,
    },
    /// Said once a plugin has gone to or come off the office's list.
    PluginShared(String),
    /// What a plugin the server ran for this copy found.
    PluginRan(Result<plugin_api::Output, String>),
    Update(Box<UpdateOffer>),
    /// The join code, how the server is keeping up to date, and Fleet.
    Office(Box<Office>),
    /// A notice address was made; its secret is in it, this once.
    NoticeMade(Box<hub::ChangeNotice>),
    /// How a test notice went.
    NoticeTested(String),
    /// Where the office stands with its license. `None` from a server that
    /// is older than licensing. `added` when a file was just added.
    License {
        standing: Option<Box<hub::license::Standing>>,
        added: bool,
    },
    /// Fleet has been let in. `key` is set only when there was no Fleet on
    /// this computer to hand it to, so somebody has to be told it.
    FleetWatching {
        note: String,
        key: Option<String>,
    },
    PasswordChanged,
    /// A newer version has been checked and put in place. It takes over the
    /// next time the program is opened.
    UpdateReady(String),
    /// A newer version was offered and not installed, and why.
    UpdateRefused(String),
    /// A key for another system, made: shown once, here, and never again.
    IntegrationKey(Box<hub::ApiKey>),
    /// The assistant app has been told about Hyperview. The sentence to show.
    AssistantConnected(String),
    /// A sync of this set did not go through. Only said when it was asked
    /// quietly; otherwise it arrives as `Trouble`.
    SyncFailed(String),
    /// The answer to somebody pressing "Check for Updates" when there is
    /// nothing to install: up to date, held, still coming, or unreachable.
    UpdateNote(String),
    /// Something did not work. Already a sentence somebody can act on.
    Trouble(String),
    /// A project's folders and its files that aren't drawings, for the Studio.
    Folders {
        project: String, // WHAT: whose. WHY: a late answer for another project is ignored.
        folders: Vec<hub::FolderInfo>, // WHAT: every folder this person sees, empty ones too. WHY: shown.
        files: Vec<hub::ProjectFile>, // WHAT: the files in them. WHY: shown after the drawings.
    },
    /// Every group of people. Administrators only.
    Groups(Vec<hub::Group>),
    /// A copy of a file saved, ready to open with its own program.
    FileReady {
        path: PathBuf, // WHAT: where the copy is. WHY: handed to the system.
        name: String, // WHAT: what it is called. WHY: said.
    },
    /// A sentence for the status line.
    Said(String),
}

/// What an administrator looks after, in one answer.
pub struct Office {
    pub joining: hub::Joining,
    pub updates: hub::UpdateSettings,
    pub fleet: hub::FleetAccess,
    pub keys: Vec<hub::ApiKey>,
    /// Where the office tells other programs a takeoff changed.
    pub notices: Vec<hub::ChangeNotice>,
}

pub struct Link {
    pub to: Sender<Ask>,
    pub from: Receiver<Told>,
}

/// Where downloaded drawings are kept. Named by their digest, so opening the
/// same set on Tuesday costs nothing.
pub fn cache_folder() -> PathBuf {
    directories::ProjectDirs::from("com", "Excalibur", "Hyperview")
        .map(|d| d.cache_dir().join("drawings"))
        .unwrap_or_else(|| std::env::temp_dir().join("hyperview-drawings"))
}

/// Where a shared tool chest lands: the `profiles` folder the program already
/// reads at start-up, so one pulled from the server behaves exactly like one
/// somebody copied in by hand.
pub fn profiles_folder() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("profiles")))
        .unwrap_or_else(|| PathBuf::from("profiles"))
}

/// Starts the worker.
pub fn start(repaint: egui::Context) -> Link {
    let (to_worker, asks) = unbounded::<Ask>();
    let (told, from_worker) = unbounded::<Told>();

    std::thread::Builder::new()
        .name("hyperview-server".into())
        .spawn(move || {
            let mut client: Option<Client> = None;
            let mut last_kept_look: Option<std::time::Instant> = None; // WHAT: when the kept drawings were last checked. WHY: every few minutes, not on every ask.
            let mut kept_for: Option<String> = None; // WHAT: the sign-in they were checked for. WHY: a new sign-in checks at once.
            let say = |told: &Sender<Told>, message: Told| {
                let _ = told.send(message);
                repaint.request_repaint();
            };

            while let Ok(ask) = asks.recv() {
                match ask {
                    Ask::Quit => break,

                    Ask::Check(base) => {
                        let probe = Client::new(&base);
                        match probe.health() {
                            Ok(health) => say(
                                &told,
                                Told::Found {
                                    base,
                                    name: health.name,
                                    version: health.version,
                                    claimed: health.claimed,
                                    joining: health.joining,
                                    reachable_at: health.reachable_at,
                                },
                            ),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        }
                    }

                    Ask::Look => {
                        // Two and a half seconds. Long enough for a machine
                        // that was asleep to wake its network card up, short
                        // enough that nobody wonders whether they clicked it.
                        let found = hub::discover::look(Duration::from_millis(2500));
                        say(&told, Told::Servers(found));
                    }

                    Ask::Claim { base, setup } => {
                        let mut fresh = Client::new(&base);
                        match fresh.claim(&setup) {
                            Ok(claimed) => {
                                let session = claimed.session.clone();
                                client = Some(fresh);
                                say(
                                    &told,
                                    Told::Claimed {
                                        base,
                                        company: claimed.company,
                                        join_code: claimed.join_code,
                                    },
                                );
                                say(&told, Told::SignedIn(Box::new(session)));
                            }
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        }
                    }

                    Ask::Join { base, join } => {
                        let mut fresh = Client::new(&base);
                        match fresh.join(&join) {
                            Ok(session) => {
                                client = Some(fresh);
                                say(&told, Told::SignedIn(Box::new(session)));
                            }
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        }
                    }

                    Ask::SignIn {
                        base,
                        email,
                        password,
                    } => {
                        let mut fresh = Client::new(&base);
                        match fresh.sign_in(&email, &password) {
                            Ok(session) => {
                                client = Some(fresh);
                                say(&told, Told::SignedIn(Box::new(session)));
                            }
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        }
                    }

                    Ask::Resume { base, token, reachable_at } => {
                        // The shop's own address first, always: it is faster,
                        // it is on their own network, and the drawings never
                        // leave the building. The public one is what a truck
                        // falls back to, not what an office uses.
                        let mut tried = vec![base.clone()];
                        if let Some(outside) = reachable_at {
                            let outside = if outside.contains("://") {
                                outside
                            } else {
                                format!("https://{outside}")
                            };
                            if outside != base {
                                tried.push(outside);
                            }
                        }
                        let mut resumed = None;
                        for address in &tried {
                            let fresh = Client::new(address).with_token(&token);
                            // A kept token is checked before it is trusted,
                            // so a week-old one fails here rather than in the
                            // middle of saving somebody's takeoff.
                            if let Ok(user) = fresh.me() {
                                resumed = Some((fresh, user, address.clone()));
                                break;
                            }
                        }
                        match resumed {
                            Some((fresh, user, address)) => {
                                if address != base {
                                    // Worth a line in the log: somebody
                                    // wondering why the office feels slow
                                    // should be able to find out that they
                                    // are coming in from outside.
                                    eprintln!(
                                        "the shop network did not answer; reached {address} instead"
                                    );
                                }
                                let session = Session {
                                    token,
                                    expires_in: 0,
                                    user,
                                    api_version: hub::API_VERSION,
                                };
                                client = Some(fresh);
                                say(&told, Told::SignedIn(Box::new(session)));
                            }
                            None => say(&told, Told::SignedOut),
                        }
                    }

                    Ask::SignOut => {
                        client = None;
                        say(&told, Told::SignedOut);
                    }

                    Ask::Remote => match connected(&client) {
                        // Asked whenever the Office panel opens. A server
                        // that cannot answer just now is not worth a message
                        // box -- the section simply does not appear.
                        Err(_) => {}
                        Ok(client) => match client.remote() {
                            Ok(remote) => say(&told, Told::RemoteIs(Box::new(remote))),
                            Err(_) => {}
                        },
                    },
                    Ask::RemoteOn { token, hostname } => match connected(&client) {
                        Err(e) => say(&told, Told::Trouble(e.to_string())),
                        Ok(client) => match client.remote_on(&token, &hostname) {
                            Ok(remote) => say(&told, Told::RemoteIs(Box::new(remote))),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },
                    Ask::RemoteOff => match connected(&client) {
                        Err(e) => say(&told, Told::Trouble(e.to_string())),
                        Ok(client) => match client.remote_off() {
                            Ok(remote) => say(&told, Told::RemoteIs(Box::new(remote))),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },
                    Ask::Projects => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.projects() {
                            Ok(list) => say(&told, Told::Projects(list)),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::People => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.people() {
                            Ok(list) => say(&told, Told::People(list)),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::AddPerson { name, email, password, role } => {
                        match connected(&client) {
                            Err(message) => say(&told, Told::Trouble(message)),
                            Ok(client) => {
                                match client.add_person(&name, &email, &password, role) {
                                    Ok(person) => people_now(
                                        client,
                                        &told,
                                        format!(
                                            "{} has an account here, as {}.",
                                            person.name,
                                            role_said(person.role)
                                        ),
                                    ),
                                    Err(e) => say(&told, Told::Trouble(e.to_string())),
                                }
                            }
                        }
                    }

                    Ask::ChangeRole { id, role } => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.change_role(&id, role) {
                            Ok(person) => people_now(
                                client,
                                &told,
                                format!("{} is now {}.", person.name, role_said(person.role)),
                            ),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::RemovePerson { id, name } => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.remove_person(&id) {
                            // The server takes their sessions and keys with
                            // them, and what they made stays. Worth saying,
                            // because the second half is the half somebody
                            // removing a colleague is anxious about.
                            Ok(_) => people_now(
                                client,
                                &told,
                                format!(
                                    "{name} has been removed. Their markups and sets stay.",
                                ),
                            ),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::Sets(project) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.sets(&project) {
                            Ok(list) => {
                                say(&told, Told::Sets(list));
                                folders_now(client, &told, &project); // WHAT: its folders and other files too. WHY: the Studio shows every folder, the empty ones and the files after the drawings.
                            }
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::Open { set, name } => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => {
                            say(
                                &told,
                                Told::Progress {
                                    what: format!("Looking for {name}…"),
                                },
                            );
                            match fetch(client, &set, &name, &told) {
                                Ok((path, cached)) => {
                                    say(&told, Told::Downloaded { set, path, cached })
                                }
                                Err(message) => say(&told, Told::Trouble(message)),
                            }
                        }
                    },

                    Ask::Takeoff(set) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.takeoff(&set) {
                            Ok(t) => say(&told, Told::Takeoff(Box::new(t))),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::Sync {
                        set,
                        since,
                        sending,
                        quiet,
                    } => match connected(&client) {
                        // No server is not a problem. The markups are already
                        // in the file on disk; the sync catches up later.
                        Err(_) => {}
                        Ok(client) => {
                            let push: Vec<hub::NewMarkup> = sending
                                .iter()
                                .map(|(page, dictionary)| hub::NewMarkup {
                                    page: *page,
                                    dictionary: dictionary.clone(),
                                    replaces: None,
                                })
                                .collect();
                            // Sent first, so what comes back already includes
                            // them and this machine learns their names.
                            let sent: Vec<String> = if push.is_empty() {
                                Vec::new()
                            } else {
                                match client.push_markups(&set, &push) {
                                    Ok(page) => {
                                        page.markups.iter().map(|m| m.id.clone()).collect()
                                    }
                                    Err(e) => {
                                        say(&told, sync_failed(&set, quiet, e.to_string()));
                                        continue;
                                    }
                                }
                            };
                            match client.markups(&set, since) {
                                Ok(page) => say(
                                    &told,
                                    Told::Synced {
                                        set,
                                        revision: page.revision,
                                        markups: page
                                            .markups
                                            .into_iter()
                                            .map(|m| (m.id, m.page, m.dictionary, m.removed))
                                            .collect(),
                                        sent,
                                    },
                                ),
                                Err(e) => {
                                    // The signal went between the push and the
                                    // read-back -- the trailer case. What was
                                    // sent is on the server whether or not we
                                    // got to read it, so record it before
                                    // saying anything went wrong.
                                    if !sent.is_empty() {
                                        say(
                                            &told,
                                            Told::Pushed {
                                                set: set.clone(),
                                                sent,
                                            },
                                        );
                                    }
                                    say(&told, sync_failed(&set, quiet, e.to_string()));
                                }
                            }
                        }
                    },

                    // Asked on sign-in and now and then after. A server that
                    // cannot answer just now is not worth a message box.
                    Ask::Chests => match connected(&client) {
                        Err(_) => {}
                        Ok(client) => match client.chests() {
                            Ok(list) => say(&told, Told::Chests(list)),
                            Err(e) => log::info!("could not list the office's tool chests: {e}"),
                        },
                    },

                    Ask::Plugins => match connected(&client) {
                        Err(_) => {}
                        Ok(client) => match client.plugins() {
                            Ok(list) => say(&told, Told::Plugins(list)),
                            Err(e) => log::info!("could not list the office's plugins: {e}"),
                        },
                    },

                    Ask::FetchPlugin { id, name } => match connected(&client) {
                        Err(_) => {}
                        Ok(client) => match client.plugin_file(&id) {
                            Ok(bytes) => {
                                let folder = crate::plugins::office_folder();
                                let path = folder.join(format!("{}.{}", safe(&id), crate::plugins::EXTENSION));
                                match std::fs::create_dir_all(&folder)
                                    .and_then(|_| std::fs::write(&path, &bytes))
                                {
                                    Ok(()) => say(&told, Told::PluginFetched { name }),
                                    Err(e) => log::warn!("could not keep plugin {id}: {e}"),
                                }
                            }
                            Err(e) => log::warn!("could not fetch plugin {id}: {e}"),
                        },
                    },

                    Ask::RunPlugin { id, input } => match connected(&client) {
                        Err(message) => say(&told, Told::PluginRan(Err(message))),
                        Ok(client) => {
                            // Its own thread: a plugin can think for a minute,
                            // and nothing else should wait behind it.
                            let client = client.clone();
                            let told = told.clone();
                            let repaint = repaint.clone();
                            std::thread::Builder::new()
                                .name(format!("plugin {id} on the server"))
                                .spawn(move || {
                                    let ran = match client.run_plugin(&id, &input) {
                                        Ok(plugin_api::Answer::Done(output)) => Ok(output),
                                        Ok(plugin_api::Answer::Failed(why)) => Err(why),
                                        Err(e) => Err(e.to_string()),
                                    };
                                    let _ = told.send(Told::PluginRan(ran));
                                    repaint.request_repaint();
                                })
                                .ok();
                        }
                    },

                    Ask::SharePlugin(path) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match std::fs::read(&path) {
                            Err(e) => say(&told, Told::Trouble(format!("{}: {e}", path.display()))),
                            Ok(bytes) => match client.upload_plugin(&bytes) {
                                Ok(shared) => {
                                    say(
                                        &told,
                                        Told::PluginShared(format!(
                                            "{} {} is shared with the office. Every seat picks it up within a few minutes.",
                                            shared.name, shared.version
                                        )),
                                    );
                                    if let Ok(list) = client.plugins() {
                                        say(&told, Told::Plugins(list));
                                    }
                                }
                                Err(e) => say(&told, Told::Trouble(e.to_string())),
                            },
                        },
                    },

                    Ask::RemovePlugin(id) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.remove_plugin(&id) {
                            Ok(_) => {
                                say(
                                    &told,
                                    Told::PluginShared(
                                        "Taken off the office's list. Each seat drops it the next time it looks.".into(),
                                    ),
                                );
                                if let Ok(list) = client.plugins() {
                                    say(&told, Told::Plugins(list));
                                }
                            }
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::FetchChest { id, name } => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.chest_file(&id) {
                            Ok(bytes) => {
                                let folder = profiles_folder();
                                let extension = if chest::native::is_native(&bytes) {
                                    chest::native::EXTENSION
                                } else {
                                    "bpx"
                                };
                                let path = folder.join(format!("{}.{extension}", safe(&name)));
                                match std::fs::create_dir_all(&folder)
                                    .and_then(|_| std::fs::write(&path, &bytes))
                                {
                                    Ok(()) => say(&told, Told::ChestFetched { name, path }),
                                    Err(e) => say(
                                        &told,
                                        Told::Trouble(format!(
                                            "Could not keep that tool chest: {e}"
                                        )),
                                    ),
                                }
                            }
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::MakeSyncKey(name) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)), // WHAT: not signed in. WHY: said, as for every other key.
                        Ok(client) => match client.make_key(crate::serverui::SYNC_PURPOSE, &name) {
                            Ok(key) => {
                                say(&told, Told::IntegrationKey(Box::new(key))); // WHAT: the new key, to show once. WHY: the same place an integration key is shown.
                                office(client, &told); // WHAT: the key list again. WHY: the new key appears in it.
                            }
                            Err(e) => say(&told, Told::Trouble(e.to_string())), // WHAT: the server's refusal. WHY: passed on as it is (only an administrator may make one).
                        },
                    },
                    Ask::MakeIntegrationKey(name) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.make_key("integration", &name) {
                            Ok(key) => {
                                say(&told, Told::IntegrationKey(Box::new(key)));
                                office(client, &told);
                            }
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::RevokeKey(id) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.revoke_key(&id) {
                            Ok(_) => office(client, &told),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::AddNotice(url) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.add_notice(&url) {
                            Ok(made) => {
                                say(&told, Told::NoticeMade(Box::new(made)));
                                office(client, &told);
                            }
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },
                    Ask::TestNotice(id) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => {
                            let said = match client.test_notice(&id) {
                                Ok(status) if status == "delivered" => "The test notice was delivered.".to_string(),
                                Ok(status) => format!("The test notice wasn't taken: {status}."),
                                Err(e) => e.to_string(),
                            };
                            say(&told, Told::NoticeTested(said));
                            office(client, &told);
                        }
                    },
                    Ask::RemoveNotice(id) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.remove_notice(&id) {
                            Ok(_) => office(client, &told),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::ConnectAssistant { .. } if hub::sealed::is_sealed() => {
                        say(&told, Told::Trouble(hub::sealed::refusal("The assistant")));
                    }
                    Ask::ConnectAssistant { computer } => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => {
                            let name = format!("Assistant on {computer}");
                            let made = client
                                .make_key("assistant", &name)
                                .map_err(|e| e.to_string())
                                .and_then(|k| k.key.ok_or_else(|| "the server made no key".to_string()));
                            let server_name = client
                                .health()
                                .map(|h| h.name)
                                .unwrap_or_default();
                            let result = made.and_then(|key| {
                                crate::assistant::keep(&crate::assistant::Connection {
                                    base: client.base().to_string(),
                                    key,
                                    name: server_name.clone(),
                                })?;
                                crate::assistant::add_to_assistant_app()
                            });
                            match result {
                                Ok(places) => say(
                                    &told,
                                    // WHAT: what the person is told once the assistant app is connected. WHY: names the Help command that checks it, as the menu shows it.
                                    Told::AssistantConnected(format!(
                                        "The assistant app can read {}'s takeoffs now, and work \
                                         alongside you in this window — it proposes, only you \
                                         draw. Quit the assistant app from the tray (right-click its \
                                         icon, Quit — closing the window is not enough) and open \
                                         it again, then ask it something like \"what's on this \
                                         sheet?\" ({} settings file{} updated. Help → Test Assistant \
                                         Connection checks it any time.)",
                                        if server_name.is_empty() { "the office" } else { &server_name },
                                        places.len(),
                                        if places.len() == 1 { "" } else { "s" }
                                    )),
                                ),
                                Err(why) => say(&told, Told::Trouble(format!("The assistant could not be connected: {why}"))),
                            }
                        }
                    },

                    Ask::NoticeInstalled => {
                        let told = told.clone();
                        let repaint = repaint.clone();
                        std::thread::Builder::new()
                            .name("hyperview-installed".into())
                            .spawn(move || {
                                if let Some(version) = crate::install::waiting_version() {
                                    let _ = told.send(Told::UpdateReady(version));
                                    repaint.request_repaint();
                                }
                            })
                            .ok();
                    }

                    Ask::CheckForUpdate { asked, .. } if !crate::edition::updates_itself() => {
                        if asked {
                            say(
                                &told,
                                Told::Trouble("This copy is kept up to date by the App Store.".into()),
                            );
                        }
                    }
                    Ask::CheckForUpdate {
                        running,
                        channel,
                        asked,
                    } if hub::sealed::is_sealed() => {
                        // A sealed office is updated by its administrator from
                        // a signed file, not by this program reaching out. Say
                        // so only when somebody asked; the check that runs by
                        // itself at start-up stays quiet.
                        if asked {
                            say(&told, Told::Trouble(hub::sealed::refusal("Checking for updates")));
                        }
                        let _ = (running, channel);
                    }
                    Ask::CheckForUpdate {
                        running,
                        channel,
                        asked,
                    } => {
                        // Its own thread: fetching a new version takes a
                        // minute on a slow line, and drawings must not wait
                        // behind it.
                        let client = client.clone();
                        let told = told.clone();
                        let repaint = repaint.clone();
                        std::thread::Builder::new()
                            .name("hyperview-update".into())
                            .spawn(move || {
                                let tell = |message: Told| {
                                    let _ = told.send(message);
                                    repaint.request_repaint();
                                };
                                check_for_update(client.as_ref(), &running, channel, asked, &tell)
                            })
                            .ok();
                    }

                    Ask::NewProject { number, name } => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.create_project(&number, &name) {
                            Ok(_) => match client.projects() {
                                Ok(list) => say(&told, Told::Projects(list)),
                                Err(e) => say(&told, Told::Trouble(e.to_string())),
                            },
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::UploadSets { project, files } => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => {
                            let mut failed = Vec::new();
                            for path in &files {
                                let filename = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "drawing.pdf".into());
                                let name = path
                                    .file_stem()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| filename.clone());
                                let bytes = match std::fs::read(path) {
                                    Ok(bytes) => bytes,
                                    Err(e) => {
                                        failed.push(format!("{filename}: {e}"));
                                        continue;
                                    }
                                };
                                say(
                                    &told,
                                    Told::Progress {
                                        what: format!(
                                            "Putting {filename} on the server ({:.0} MB)…",
                                            bytes.len() as f64 / 1_048_576.0
                                        ),
                                    },
                                );
                                if let Err(e) = client.upload_set(&project, &name, &filename, &bytes) {
                                    failed.push(format!("{filename}: {e}"));
                                }
                            }
                            match client.sets(&project) {
                                Ok(list) => say(&told, Told::Sets(list)),
                                Err(e) => say(&told, Told::Trouble(e.to_string())),
                            }
                            if let Ok(list) = client.projects() {
                                say(&told, Told::Projects(list));
                            }
                            if !failed.is_empty() {
                                say(
                                    &told,
                                    Told::Trouble(format!(
                                        "Not put on the server:\n{}",
                                        failed.join("\n")
                                    )),
                                );
                            }
                        }
                    },

                    Ask::ShareChest(path) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => {
                            let filename = path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| "chest.bpx".into());
                            let name = path
                                .file_stem()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| filename.clone());
                            // The office gets Excalibur View's own file, whatever it
                            // was picked as: a Revu file is converted on the way.
                            let read = std::fs::read(&path).map(|bytes| {
                                if chest::native::is_native(&bytes) {
                                    (filename.clone(), bytes)
                                } else {
                                    match chest::Profile::read(&bytes) {
                                        Some(profile) => (
                                            format!("{name}.{}", chest::native::EXTENSION),
                                            chest::native::write(&profile, &format!("Revu: {filename}")),
                                        ),
                                        None => (filename.clone(), bytes),
                                    }
                                }
                            });
                            match read {
                                Err(e) => say(&told, Told::Trouble(format!("{filename}: {e}"))),
                                Ok((filename, bytes)) => match client.upload_chest(&name, &filename, &bytes) {
                                    Ok(_) => match client.chests() {
                                        Ok(list) => say(&told, Told::Chests(list)),
                                        Err(e) => say(&told, Told::Trouble(e.to_string())),
                                    },
                                    Err(e) => say(&told, Told::Trouble(e.to_string())),
                                },
                            }
                        }
                    },

                    Ask::Office => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => office(client, &told),
                    },

                    // Asked quietly: a server that cannot say is simply not
                    // shown as licensed or not, never an error in anybody's face.
                    Ask::License => {
                        if let Ok(client) = connected(&client) {
                            if let Ok(standing) = client.license() {
                                say(&told, Told::License { standing: standing.map(Box::new), added: false });
                            }
                        }
                    }

                    Ask::CopySet { set, name } => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.copy_set(&set, "") {
                            Ok(copy) => {
                                say(&told, Told::Copied { name: name.clone(), copy: Box::new(copy) });
                            }
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },
                    Ask::AddLicense(path) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match std::fs::read_to_string(&path) {
                            Err(e) => say(&told, Told::Trouble(format!("{}: {e}", path.display()))),
                            Ok(text) => match client.add_license(&text) {
                                Ok(standing) => say(
                                    &told,
                                    Told::License { standing: Some(Box::new(standing)), added: true },
                                ),
                                Err(e) => say(&told, Told::Trouble(e.to_string())),
                            },
                        },
                    },
                    Ask::AddLicenseText(text) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.add_license(text.trim()) {
                            Ok(standing) => say(
                                &told,
                                Told::License { standing: Some(Box::new(standing)), added: true },
                            ),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    // Fleet watches a server from outside the company.
                    Ask::WatchWithFleet { .. } | Ask::StopFleet if hub::sealed::is_sealed() => {
                        say(&told, Told::Trouble(hub::sealed::refusal("Excalibur Fleet")));
                    }
                    Ask::WatchWithFleet { name, base } => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.change_fleet_access(true) {
                            Ok(access) => {
                                let key = access.key.unwrap_or_default();
                                match crate::fleetlink::register(&name, &base, &key) {
                                    Ok(status) => say(
                                        &told,
                                        Told::FleetWatching {
                                            note: format!(
                                                "Excalibur Fleet is watching this server, as \
                                                 \"{}\" ({status} on the board).",
                                                crate::fleetlink::board_name(&name)
                                            ),
                                            key: None,
                                        },
                                    ),
                                    Err(why) => say(
                                        &told,
                                        Told::FleetWatching {
                                            note: format!(
                                                "{why} Give whoever looks after this company's \
                                                 computers this address and key. The key is \
                                                 shown once."
                                            ),
                                            key: Some(key),
                                        },
                                    ),
                                }
                                office(client, &told);
                            }
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::StopFleet => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.change_fleet_access(false) {
                            Ok(_) => office(client, &told),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::JoinCodeDays(days) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => {
                            match client.change_joining_fully(None, None, false, Some(days)) {
                                Ok(_) => office(client, &told),
                                Err(e) => say(&told, Told::Trouble(e.to_string())),
                            }
                        }
                    },
                    Ask::NewJoinCode => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.change_joining(Some("code"), None, true) {
                            Ok(_) => office(client, &told),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::UpdateChannel(channel) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.change_update_channel(&channel) {
                            Ok(_) => office(client, &told),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::ChangePassword { current, new } => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)),
                        Ok(client) => match client.change_password(&current, &new) {
                            Ok(_) => say(&told, Told::PasswordChanged),
                            Err(e) => say(&told, Told::Trouble(e.to_string())),
                        },
                    },

                    Ask::OpenFile { file, name } => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)), // WHAT: not signed in. WHY: said.
                        Ok(client) => match client.file_bytes(&file) {
                            Ok(bytes) => match crate::openwith::save_copy(&file, &name, &bytes) {
                                Ok(path) => say(&told, Told::FileReady { path, name }), // WHAT: the copy, ready to hand to its program. WHY: the window opens it, or offers it on a tablet.
                                Err(message) => say(&told, Told::Trouble(message)), // WHAT: it couldn't be saved. WHY: said.
                            },
                            Err(e) => say(&told, Told::Trouble(e.to_string())), // WHAT: refused or unreachable. WHY: said.
                        },
                    },

                    Ask::SetFolderRule { project, folder, see } => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)), // WHAT: not signed in. WHY: said.
                        Ok(client) => match client.set_folder_rule(&project, &folder, &see) {
                            Ok(answer) => {
                                let notes: Vec<String> = answer["notes"].as_array().map(|n| n.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(); // WHAT: what the server wants said. WHY: a rule wider than its parent, a group made empty.
                                let said = if notes.is_empty() { format!("{folder}: {}.", see.words()) } else { notes.join(" ") }; // WHAT: the sentence for the status line. WHY: the person sees what happened.
                                folders_now(client, &told, &project); // WHAT: the folders read again. WHY: the Studio shows the new rule.
                                say(&told, Told::Said(said)); // WHAT: the sentence. WHY: shown.
                            }
                            Err(e) => say(&told, Told::Trouble(e.to_string())), // WHAT: refused. WHY: said.
                        },
                    },

                    Ask::Groups => match connected(&client) {
                        Err(_) => {} // WHAT: not signed in. WHY: the section just doesn't show.
                        Ok(client) => {
                            if let Ok(groups) = client.groups() {
                                say(&told, Told::Groups(groups)); // WHAT: the groups. WHY: Studio, Office, People.
                            } // WHAT: an older server has none. WHY: the section stays empty rather than showing an error.
                        }
                    },

                    Ask::MakeGroup(name) => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)), // WHAT: not signed in. WHY: said.
                        Ok(client) => match client.make_group(&name).and_then(|_| client.groups()) {
                            Ok(groups) => {
                                say(&told, Told::Groups(groups)); // WHAT: the groups now. WHY: the new one shows.
                                say(&told, Told::Said(format!("Made the group {name}. Tick the people in it."))); // WHAT: what to do next. WHY: an empty group gives nobody anything.
                            }
                            Err(e) => say(&told, Told::Trouble(e.to_string())), // WHAT: refused. WHY: said.
                        },
                    },

                    Ask::ChangeGroup { group, person, on } => match connected(&client) {
                        Err(message) => say(&told, Told::Trouble(message)), // WHAT: not signed in. WHY: said.
                        Ok(client) => match client.change_group(&group, &person, on).and_then(|_| client.groups()) {
                            Ok(groups) => say(&told, Told::Groups(groups)), // WHAT: the groups now. WHY: the tick shows.
                            Err(e) => say(&told, Told::Trouble(e.to_string())), // WHAT: refused. WHY: said.
                        },
                    },
                }
                if let Some(signed_in) = client.as_ref() {
                    let fresh_sign_in = kept_for.as_deref() != signed_in.token(); // WHAT: whether this is a sign-in not yet checked for. WHY: somebody else signing in on this seat must not open the last person's copies.
                    if fresh_sign_in || last_kept_look.is_none_or(|at: std::time::Instant| at.elapsed() >= KEPT_LOOK_EVERY) {
                        last_kept_look = Some(std::time::Instant::now()); // WHAT: looked now. WHY: not again for a while.
                        kept_for = signed_in.token().map(str::to_string); // WHAT: the sign-in checked for. WHY: as above.
                        let gone = look_at_kept(signed_in); // WHAT: kept copies this person may no longer see, taken off. WHY: an administrator's change reaches this computer too.
                        if gone > 0 {
                            say(&told, Told::Said(format!("Took {gone} drawing(s) off this computer that you can no longer see on the server."))); // WHAT: said. WHY: nobody wonders where a drawing went.
                        }
                    }
                }
            }
        })
        .expect("a thread for the server");

    Link {
        to: to_worker,
        from: from_worker,
    }
}

fn sync_failed(set: &str, quiet: bool, why: String) -> Told {
    if quiet {
        log::info!("markups for {set} did not sync: {why}");
        Told::SyncFailed(set.to_string())
    } else {
        Told::Trouble(why)
    }
}

/// How long a seat leaves between asking whether there is a newer version.
///
/// Signed in to its company's server, a quarter of an hour: the question goes
/// no further than the office, and the server looks at the publisher's feed
/// for everybody. On its own, an hour, because then it is GitHub being asked,
/// which answers sixty questions an hour from any one office's address. When
/// the server said it was still fetching one, two minutes.
pub fn update_interval(signed_in: bool, server_looking: bool) -> std::time::Duration {
    let minutes = match (signed_in, server_looking) {
        (true, true) => 2,
        (true, false) => 15,
        (false, _) => 60,
    };
    std::time::Duration::from_secs(minutes * 60)
}

/// Asks whether there is a newer Hyperview and, when this copy looks after
/// itself, fetches it, checks it against the signing key compiled into this
/// program, and puts it in place for the next start.
///
/// A seat signed in to its company's server asks that server, always: the
/// administrator may be holding the office on one version. A seat with no
/// server asks the publisher's feed itself.
///
/// `asked` is somebody pressing "Check for Updates". They are told the answer
/// whatever it is; otherwise only a newer version is worth saying anything
/// about.
fn check_for_update(
    client: Option<&Client>,
    running: &str,
    channel: Channel,
    asked: bool,
    told: &dyn Fn(Told),
) {
    // One at a time. Two checks racing would download the same file twice
    // and put it in place twice.
    static CHECKING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = CHECKING.lock();

    let say = told;
    let note = |words: String| {
        log::info!("update check: {words}");
        if asked {
            say(Told::UpdateNote(words));
        }
    };
    // Something newer may be in place already: an update fetched earlier, or
    // a newer download somebody double-clicked while this window was open.
    let waiting = crate::install::waiting_version();
    if let Some(version) = waiting.clone() {
        say(Told::UpdateReady(version));
    }
    enum From {
        Server,
        Feed(hub::feed::Feed, hub::feed::Found),
    }
    let (offer, from) = match client {
        Some(client) => {
            let answer = if asked {
                client.update_offer_now(running, channel, platform())
            } else {
                client.update_offer(running, channel, platform())
            };
            match answer {
                Ok(offer) => (offer, From::Server),
                Err(e) => return note(format!("Your server could not be asked about updates: {e}")),
            }
        }
        None => {
            let url = crate::trust::HOME_FEED.trim();
            if url.is_empty() {
                return note("This copy was built without an update page to read.".into());
            }
            let feed = hub::feed::Feed::new(url);
            match feed.newest(channel) {
                // Its own platform's build, not whichever one the manifest
                // happens to name first. A release with nothing for this Mac
                // is "nothing new", the same as no release at all.
                Ok(Some(found))
                    if found
                        .published
                        .app_for(platform())
                        .is_some_and(|r| hub::update::newer(&r.version, running)) =>
                {
                    let release = found.published.app_for(platform()).cloned();
                    let offer = UpdateOffer {
                        release,
                        pinned_to: None,
                        required: false,
                        looking: false,
                    };
                    (offer, From::Feed(feed, found))
                }
                Ok(_) => return note(format!("Excalibur View {running} is the newest version.")),
                // Quietly unless somebody asked. A seat that cannot reach the
                // internet is an ordinary Tuesday on a shop floor.
                Err(e) => return note(format!("The update page could not be reached: {e}")),
            }
        }
    };
    let release = offer.release.clone();
    let looking = offer.looking;
    let pinned = offer.pinned_to.clone();
    say(Told::Update(Box::new(offer)));
    let Some(release) = release else {
        if let Some(version) = waiting {
            return note(format!(
                "Excalibur View {version} is installed and takes over when Excalibur View restarts."
            ));
        }
        return note(match pinned {
            Some(version) if version != running => format!(
                "Your administrator is holding the office on version {version}."
            ),
            Some(_) => format!(
                "Your administrator is holding the office on {running}, which is this one."
            ),
            None if looking => "Your server is fetching a new version. It is offered here \
                                the moment it has it."
                .into(),
            None => format!("Excalibur View {running} is the newest version."),
        });
    };
    log::info!("update check: version {} is on offer", release.version);
    if !crate::install::updates_itself() {
        return;
    }
    if crate::install::already_waiting(&release) {
        return say(Told::UpdateReady(release.version));
    }
    // Never back over something newer already in place.
    if let Some(version) = waiting.filter(|v| !hub::update::newer(&release.version, v)) {
        return say(Told::UpdateReady(version));
    }
    let bytes = match &from {
        From::Server => client
            .map(|c| c.download_update(&release.download).map_err(|e| e.to_string()))
            .unwrap_or_else(|| Err("no server".into())),
        From::Feed(feed, found) => match found.url(&release.download) {
            Some(url) => feed.fetch(url),
            None => Err(format!("the release has no file called {}", release.download)),
        },
    };
    match bytes.and_then(|bytes| crate::install::put_in_place(&release, &bytes)) {
        Ok(()) => {
            log::info!("update check: version {} is in place", release.version);
            say(Told::UpdateReady(release.version))
        }
        Err(why) => {
            log::warn!("update not installed: {why}");
            say(Told::UpdateRefused(why));
        }
    }
}

fn office(client: &Client, told: &Sender<Told>) {
    let fleet = client.fleet_access().unwrap_or_default();
    let keys = client.keys().unwrap_or_default();
    // A server from before change notices simply has none.
    let notices = client.notices().unwrap_or_default();
    match (client.joining(), client.update_settings()) {
        (Ok(joining), Ok(updates)) => {
            let _ = told.send(Told::Office(Box::new(Office {
                joining,
                updates,
                fleet,
                keys,
                notices,
            })));
        }
        (Err(e), _) | (_, Err(e)) => {
            let _ = told.send(Told::Trouble(e.to_string()));
        }
    }
}

/// Sends what just happened together with the list as it now stands.
///
/// Asking the server again rather than editing the list here is deliberate:
/// the server is what decides, and a screen that guesses at the answer is a
/// screen that will eventually be wrong about who can do what.
fn people_now(client: &Client, told: &Sender<Told>, said: String) {
    let _ = match client.people() {
        Ok(people) => told.send(Told::PeopleChanged { people, said }),
        // The change worked; only the re-read did not. Say so, rather than
        // reporting a failure that did not happen.
        Err(e) => told.send(Told::Trouble(format!("{said} The list did not come back: {e}"))),
    };
}

/// A role as somebody would say it out loud.
fn role_said(role: hub::Role) -> &'static str {
    match role {
        hub::Role::Viewer => "a viewer",
        hub::Role::Estimator => "an estimator",
        hub::Role::Admin => "an administrator",
    }
}

/// How often a signed-in seat asks about the drawings it keeps.
const KEPT_LOOK_EVERY: Duration = Duration::from_secs(5 * 60); // WHAT: five minutes. WHY: soon enough that a folder taken away goes from a tablet the same morning, rarely enough to cost nothing.

/// Asks the server about every drawing this seat keeps, and takes off the
/// ones its person may no longer see. How many went.
fn look_at_kept(client: &Client) -> usize {
    let folder = cache_folder(); // WHAT: where the copies are. WHY: checked.
    let gone = crate::kept::forget_unseen(&folder, true, |set| match client.set(set) {
        Ok(_) => crate::kept::Seen::Yes, // WHAT: still theirs. WHY: the server answered it.
        Err(hub::client::Trouble::Refused(problem)) if problem.error == "not_found" => crate::kept::Seen::No, // WHAT: not found for this person. WHY: taken away, or never theirs.
        Err(_) => crate::kept::Seen::CantTell, // WHAT: anything else. WHY: never taken off on a guess.
    }); // WHAT: asked, and the copies taken off. WHY: an administrator's change reaches this computer too.
    for path in &gone {
        log::info!("took {} off this computer: its person can no longer see it", path.display()); // WHAT: written in the log. WHY: so a missing file can be explained.
    }
    gone.len()
}

/// A project's folders and files read again, and told. An older server that
/// has neither is told as none, without a message.
fn folders_now(client: &Client, told: &Sender<Told>, project: &str) {
    let folders = client.folders(project).unwrap_or_default(); // WHAT: its folders. WHY: an older server has no such route, which is no folders.
    let files = client.files(project).unwrap_or_default(); // WHAT: its files. WHY: likewise.
    let _ = told.send(Told::Folders { project: project.to_string(), folders, files }); // WHAT: told. WHY: the Studio shows them.
}

fn connected(client: &Option<Client>) -> Result<&Client, String> {
    client
        .as_ref()
        .ok_or_else(|| "Not signed in to a server.".to_string())
}

/// Downloads a drawing into the cache, or finds it already there.
fn fetch(
    client: &Client,
    set: &str,
    name: &str,
    told: &Sender<Told>,
) -> Result<(PathBuf, bool), String> {
    let details = client.set(set).map_err(|e| e.to_string())?;
    // Asked for by id alone (a link, the assistant), it goes by the set's own name:
    // that is what the tab, the window's title and the assistant call it.
    let name = if name.trim().is_empty() || name == set { details.name.as_str() } else { name };
    let stem = name
        .strip_suffix(".pdf")
        .or_else(|| name.strip_suffix(".PDF"))
        .unwrap_or(name);
    let folder = cache_folder();
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    // Named by digest and by what it is called, so the cache is both
    // collision-free and something a person can look through.
    let path = folder.join(format!("{}-{}.pdf", &details.digest[..12], safe(stem)));

    if path.exists() {
        // Trust it only if it really is those bytes. A half-written cache file
        // opening as a drawing set is a worse morning than downloading again.
        if let Ok(bytes) = std::fs::read(&path) {
            use sha2::{Digest, Sha256};
            let have: String = Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            if have == details.digest {
                crate::kept::remember(&folder, set, &path); // WHAT: written down against its set. WHY: taken off again if its person stops being allowed it.
                return Ok((path, true));
            }
        }
    }

    let _ = told.send(Told::Progress {
        what: format!("Downloading {name} ({:.0} MB)…", details.bytes as f64 / 1_048_576.0),
    });
    let bytes = client
        .download(set, None)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "The server sent nothing.".to_string())?;

    let temp = path.with_extension("part");
    std::fs::write(&temp, &bytes).map_err(|e| format!("{}: {e}", temp.display()))?;
    std::fs::rename(&temp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    crate::kept::remember(&folder, set, &path); // WHAT: written down against its set. WHY: as above.
    Ok((path, false))
}

/// What this build is, for asking the server about updates.
///
/// The same string the feed publishes under and the same string that goes
/// into the signature, because a seat asking for one name and a release
/// carrying another is a seat that never updates and never says why.
pub fn platform() -> &'static str {
    hub::feed::APP_PLATFORM
}

/// Makes a name safe to be part of a filename.
pub(crate) fn safe(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '-'
            }
        })
        .take(60)
        .collect::<String>()
        .trim()
        .to_string()
}

/// What the window remembers about a server between sessions.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Remembered {
    pub base: String,
    /// A session token. Not a password — Hyperview never keeps one of those.
    pub token: String,
    pub name: String,
    pub user: String,
    /// Where the same server answers from outside the shop, when it told us.
    ///
    /// This is the whole of the jobsite feature as a seat sees it: somebody
    /// who used Excalibur View in the office opens it in a truck and it
    /// works. They never type an address, because the server mentioned this
    /// one while they were on its own network.
    #[serde(default)]
    pub reachable_at: Option<String>,
}

/// A server's name as a seat remembered it, without what older seats wrote
/// after it.
///
/// Before 0.6.6 the connect window saved "Example steel Shop Server · version 0.6.3"
/// as the server's name, so the version a seat first met a server on stayed on
/// screen for good, however many times the server updated since. The name is
/// everything before that marker; a name that never had one is left alone.
pub fn plain_name(remembered: &str) -> String {
    match remembered.find(" · version ") {
        Some(at) => remembered[..at].trim_end().to_string(),
        None => remembered.to_string(),
    }
}

/// Who is signed in, as the window sees it.
#[derive(Clone, Debug, Default)]
pub struct Standing {
    pub base: String,
    /// What the company calls this installation.
    pub name: String,
    /// Where this server also answers from outside the shop, when it has
    /// said so. Learned from the health answer and kept with the rest of
    /// what a seat remembers about its server.
    pub reachable_at: Option<String>,
    pub who: Option<Arc<User>>,
    pub projects: Vec<Project>,
    pub sets: Vec<DrawingSet>,
    pub chests: Vec<Chest>,
    /// Everybody with an account here, as the server last listed them.
    /// Administrators only; empty for everybody else because the server
    /// refuses the question rather than answering it thinly.
    pub people: Vec<hub::User>,
    /// The plugins the office hands out, as the server last listed them.
    pub plugins: Vec<hub::PluginInfo>,
    pub looking_at: Option<String>,
    pub busy: Option<String>,
    /// An update is on offer. Nothing is installed mid-session.
    pub update: Option<Release>,
    /// A version that has been put in place and takes over at the next start.
    pub update_ready: Option<String>,
    /// Why the last update on offer was not installed, when it was not.
    pub update_refused: Option<String>,
    /// The server was still fetching a new version when it last answered, so
    /// it is asked again in a minute or two rather than half an hour.
    pub update_looking: bool,
    pub pinned_to: Option<String>,
    /// The office's Excalibur View Office license, as the server last said.
    /// `None` until it has said, and from a server older than licensing.
    pub license: Option<hub::license::Standing>,
}

impl Standing {
    pub fn signed_in(&self) -> bool {
        self.who.is_some()
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_remembered_name_loses_the_version_older_seats_saved_with_it() {
        assert_eq!(super::plain_name("Example steel Shop Server · version 0.6.3"), "Example steel Shop Server");
        assert_eq!(super::plain_name("Example steel Shop Server"), "Example steel Shop Server");
        assert_eq!(super::plain_name("Smith · Sons"), "Smith · Sons");
        assert_eq!(super::plain_name(""), "");
    }

    use super::*;

    #[test]
    fn a_name_with_awkward_characters_in_it_still_makes_a_filename() {
        assert_eq!(safe("S-200 / Foundation"), "S-200 - Foundation");
        assert_eq!(safe("../../etc/passwd"), "------etc-passwd");
        assert!(!safe("a\\b:c*d?e").contains(['\\', ':', '*', '?']));
    }

    #[test]
    fn a_very_long_name_is_cut_rather_than_refused() {
        let long = "W".repeat(500);
        assert!(safe(&long).len() <= 60);
    }

    #[test]
    fn what_is_remembered_about_a_server_is_never_a_password() {
        let remembered = Remembered {
            base: "https://drawings.example.com".into(),
            token: "abc123".into(),
            name: "Example Steel".into(),
            user: "Sam".into(),
            reachable_at: None,
        };
        let text = serde_json::to_string(&remembered).unwrap();
        assert!(!text.contains("password"));
        let back: Remembered = serde_json::from_str(&text).unwrap();
        assert_eq!(back, remembered);
    }


    #[test]
    fn a_seat_only_falls_back_to_an_address_the_server_gave_it() {
        // Nobody types this. A seat learns the outside address while it is on
        // the inside, from the server's own health answer, or it has none --
        // which is the ordinary case and not a problem.
        let plain = Remembered {
            base: "http://192.168.1.20:8714".into(),
            token: "abc123".into(),
            name: "Example Steel".into(),
            user: "Sam".into(),
            reachable_at: None,
        };
        assert!(plain.reachable_at.is_none());

        // And one that was given one keeps it across a restart, or the
        // estimator in the truck is back to typing addresses.
        let away = Remembered {
            reachable_at: Some("drawings.example.com".into()),
            ..plain.clone()
        };
        let back: Remembered = serde_json::from_str(&serde_json::to_string(&away).unwrap()).unwrap();
        assert_eq!(back.reachable_at.as_deref(), Some("drawings.example.com"));
    }

    #[test]
    fn an_older_server_that_says_nothing_about_an_outside_address_still_reads() {
        // Most servers have not turned remote access on, and older ones do
        // not know the field exists. Neither may stop a seat signing in.
        let text = r#"{"base":"http://192.168.1.20:8714","token":"t","name":"Example Steel","user":"Sam"}"#;
        let remembered: Remembered = serde_json::from_str(text).unwrap();
        assert!(remembered.reachable_at.is_none());
    }
    #[test]
    fn nobody_is_signed_in_to_begin_with() {
        assert!(!Standing::default().signed_in());
    }
}
