//! The types that go over the wire.

use serde::{Deserialize, Serialize};

// ---- who -----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    /// What goes in a markup's Author column.
    pub name: String,
    pub email: String,
    pub role: Role,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Can read drawings and takeoffs. Cannot mark up.
    Viewer,
    /// The ordinary seat: mark up, measure, save.
    Estimator,
    /// Also manages people, chests, profiles and which version everyone runs.
    Admin,
}

impl Role {
    /// The word a server reads this role back from.
    ///
    /// One function, because a server that does not recognise the word does
    /// not refuse it -- it hands out an ordinary seat. So a spelling mistake
    /// here would not be an error anybody sees; it would be an administrator
    /// quietly becoming an estimator, and nobody finding out until the day
    /// they needed to remove somebody.
    pub fn word(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Estimator => "estimator",
            Role::Admin => "admin",
        }
    }

    pub fn may_write(self) -> bool {
        !matches!(self, Role::Viewer)
    }

    pub fn may_administer(self) -> bool {
        matches!(self, Role::Admin)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignIn {
    pub email: String,
    pub password: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    /// Seconds from now.
    pub expires_in: u64,
    pub user: User,
    /// What the server speaks, so a client can say plainly that it is too old
    /// rather than failing in some interesting way halfway through a takeoff.
    pub api_version: u32,
}

// ---- what --------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    /// The job number, as the shop says it.
    pub number: String,
    pub name: String,
    pub created: String,
    pub sets: usize,
    /// Who this project belongs to in another program — `fabwire:bid:2630`.
    /// Asking to create a project with a reference that already has one
    /// answers with that one, so a retried push never makes a second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// When it was put away, if it has been. An archived project keeps every
    /// drawing, markup and quantity; it is only left off the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<String>,
    /// Where it sits in the Studio's list, as folders: `Bids/2026`,
    /// `Active Jobs`, `Past Jobs/2025`. The program it belongs to says, and
    /// moves it as a bid is won or lost and a job finishes. None: with the
    /// other projects, as every project was before places.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewProject {
    pub number: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Where it goes in the Studio's list; asked for again with the same
    /// reference, it moves there. Left out, it stays where it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
}

/// A drawing set: one PDF, however many sheets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrawingSet {
    pub id: String,
    pub project: String,
    pub name: String,
    /// Whatever the file was called when it arrived.
    pub filename: String,
    /// SHA-256 of the PDF as uploaded. A client that already has these bytes
    /// does not download them again.
    pub digest: String,
    pub bytes: u64,
    pub sheets: usize,
    pub uploaded: String,
    pub uploaded_by: String,
    /// Goes up by one every time markups change. A client asks for everything
    /// since the revision it last saw.
    pub revision: u64,
    /// The set this one replaced: the same drawing, issued again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    /// The set that replaced this one. A superseded set keeps its markups, but
    /// its quantities are no longer the job's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    /// Set on an upload that turned out to be a drawing the project already
    /// has, byte for byte: nothing new was made, and this is the one there.
    #[serde(default, skip_serializing_if = "is_false")]
    pub already_here: bool,
    /// How many markups were brought forward from the set this one replaced.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub carried_forward: usize,
    /// Why some were not, in words, when some were not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carry_note: Option<String>,
    /// The folder it came from in the job's own files, as folders:
    /// `10 SHOP DRAWINGS/10.4 FOR FABRICATION`. The Studio shows a project's
    /// sets in these, and a drawing issued again replaces only the one of the
    /// same name in the same folder. None: the project's own, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    /// Zero-based, the way the file counts.
    pub page: usize,
    /// The number in the title block: S-200.
    pub number: String,
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub rotation: i32,
    /// How the sheet reads: `1/4" = 1'-0"`. Absent means no scale is set, and
    /// that is never the same as a scale of zero.
    pub scale: Option<String>,
}

// ---- markups -------------------------------------------------------------

/// One markup, as it is in the file.
///
/// `dictionary` is the PDF annotation dictionary, serialised exactly as it
/// would be written into the drawing and base64'd for transport. Everything
/// else on this struct is derived from it and is there so an integration does
/// not have to parse PDF to answer an ordinary question.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Markup {
    pub id: String,
    pub page: usize,
    pub subject: String,
    /// Length, Area, Count, Volume, Markup and so on.
    pub kind: String,
    /// What the markup says on the sheet: `40'-0"`. Empty when the sheet has
    /// no scale, because there is no honest number to put here.
    pub caption: String,
    pub author: String,
    pub created: String,
    pub colour: String,
    /// The annotation dictionary, base64. This is the markup; the rest is a
    /// convenience.
    pub dictionary: String,
    /// The revision this markup arrived in.
    pub revision: u64,
    /// Set when the markup has been taken off the drawing. Removals travel as
    /// records rather than as absences, so a client syncing from an old
    /// revision learns what went away.
    pub removed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewMarkup {
    pub page: usize,
    /// The annotation dictionary, base64.
    pub dictionary: String,
    /// Replaces this markup rather than adding one, when it is already there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarkupPage {
    pub markups: Vec<Markup>,
    /// The revision the caller is now up to date with.
    pub revision: u64,
}

// ---- takeoff -------------------------------------------------------------

/// One measured markup. The numbers are in the sheet's own units, and `scaled`
/// says whether they mean anything.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TakeoffRow {
    pub markup: String,
    pub page: usize,
    pub sheet: String,
    pub subject: String,
    pub kind: String,
    pub caption: String,
    pub author: String,
    /// True when the sheet this came off has a scale. When it is false every
    /// measured field below is `null`, and the row is left out of the totals.
    pub scaled: bool,
    pub count: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length_feet: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_square_feet: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume_cubic_feet: Option<f64>,
    /// Worked from the unit weight on the tool that drew it — his number, out
    /// of his own tool chest, never a section table this program looked up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pounds: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TakeoffGroup {
    pub subject: String,
    pub kind: String,
    pub picks: usize,
    pub count: f64,
    pub length_feet: f64,
    pub area_square_feet: f64,
    pub volume_cubic_feet: f64,
    /// Null rather than zero when nothing in the group carried a unit weight.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pounds: Option<f64>,
    pub sheets: Vec<String>,
    /// Markups in this group that had no scale and were left out.
    pub left_out: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Takeoff {
    pub set: String,
    pub revision: u64,
    pub rows: Vec<TakeoffRow>,
    pub groups: Vec<TakeoffGroup>,
    pub picks: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pounds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tons: Option<f64>,
    /// Measurements left out of every total above because their sheet has no
    /// scale. **Check this.** A total with this above zero is short, and the
    /// sheets that need a scale are named below.
    pub left_out: usize,
    pub sheets_without_a_scale: Vec<String>,
}

impl Takeoff {
    /// True when something was left out, which means the totals are short.
    pub fn is_short(&self) -> bool {
        self.left_out > 0
    }
}

// ---- shared tool chests and profiles -------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chest {
    pub id: String,
    pub name: String,
    pub filename: String,
    pub digest: String,
    pub bytes: u64,
    pub tools: usize,
    pub sets: usize,
    pub uploaded: String,
    /// Every seat gets this one automatically.
    pub shared: bool,
}

/// A plugin the office hands every seat: its own tools, signed by their
/// publisher and run sandboxed. See `plugin_api`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    /// The plugin's own id, which is also how the office knows it: adding a
    /// newer version of one replaces the older.
    pub id: String,
    pub name: String,
    pub version: String,
    /// SHA-256 of the plugin file.
    pub digest: String,
    pub bytes: u64,
    pub uploaded: String,
    #[serde(default)]
    pub uploaded_by: String,
    /// Which trusted key signed it.
    #[serde(default)]
    pub key: String,
    /// What it says it can do, read by the server in the sandbox: its
    /// commands and settings. What a copy that doesn't run plugins itself
    /// builds its Plugins menu from. Absent from an older server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<plugin_api::Manifest>,
}

// ---- errors --------------------------------------------------------------

/// What a failure looks like on the wire. One shape for everything, so an
/// integration can handle errors in one place.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    /// A stable short name: `not_found`, `unauthorised`, `too_old`.
    pub error: String,
    /// A sentence a person can act on.
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Problem {}

impl Problem {
    pub fn new(error: &str, message: impl Into<String>) -> Problem {
        Problem {
            error: error.to_string(),
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub api_version: u32,
    /// The server's own build.
    pub version: String,
    /// What the company calls this installation, shown in the client's title
    /// bar so nobody marks up a drawing on the wrong server.
    pub name: String,
    /// Whether anybody has set this server up yet. A server with no accounts on
    /// it is waiting for the first person in the shop; one that is claimed is
    /// somebody's, and the only thing to do with it is sign in.
    ///
    /// Absent on an older server, which reads as claimed — the safe way round.
    /// A client that guessed the other way would offer to take over a server
    /// that already belongs to a company.
    #[serde(default = "yes")]
    pub claimed: bool,
    /// How somebody without an account gets one: `code`, `open` or `closed`.
    /// Absent on an older server, which means an administrator adds people.
    #[serde(default)]
    pub joining: String,
    /// Where this server can also be reached from outside the shop, when the
    /// company has turned remote access on and the address has been checked.
    ///
    /// A seat that signs in on the shop network keeps this, and uses it when
    /// the broadcast finds nothing -- which is what being in a truck looks
    /// like. Absent on an older server and on every server that has not
    /// turned it on, which is most of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reachable_at: Option<String>,
}

/// What an administrator is told about reaching this server from a jobsite.
///
/// Flattened booleans rather than a tagged enum on purpose: a panel asks
/// "is it on?" and a client built against an older server must not fall over
/// because a newer one invented a state it has never heard of.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Remote {
    /// Nobody has turned it on. The ordinary state, and not a problem.
    #[serde(default)]
    pub off: bool,
    /// Sealed, and refusing -- which is what the edition is for.
    #[serde(default)]
    pub sealed: bool,
    #[serde(default)]
    pub starting: bool,
    /// On, and the address answered when the server asked for it from the
    /// outside. A connector that started is not the same thing.
    #[serde(default)]
    pub on: bool,
    /// The hostname, when there is one. Never the token: a screen that can
    /// show a secret is a screen somebody can photograph.
    #[serde(default)]
    pub hostname: Option<String>,
    /// The sentence to put on screen, worked out by the server so every
    /// client says the same thing.
    #[serde(default)]
    pub said: String,
}

/// One invitation: a code for one person, used once.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Invitation {
    /// The secret. This is the whole of what gets sent to somebody.
    pub code: String,
    pub email: String,
    #[serde(default)]
    pub name: String,
    pub role: String,
    /// When it stops working, RFC 3339.
    pub expires: String,
    /// True when it has been used or has run out -- either way it is spent.
    #[serde(default)]
    pub used: bool,
}

fn yes() -> bool {
    true
}

// ---- the first person in, and everybody after ----------------------------

/// Claiming a server nobody has claimed yet. This works exactly once: the
/// moment it succeeds there is an account on the server, and it is refused
/// from then on.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Setup {
    /// What the company wants this installation called.
    pub company: String,
    /// The first administrator.
    pub name: String,
    pub email: String,
    pub password: String,
}

/// What comes back from claiming one: a session, so the person who set it up is
/// already signed in, and the code to hand to everybody else.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Claimed {
    pub session: Session,
    pub company: String,
    /// What the rest of the office types to make their own accounts.
    pub join_code: String,
}

/// Somebody in the office making their own account.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Join {
    pub code: String,
    pub name: String,
    pub email: String,
    pub password: String,
}

/// How a server keeps itself and its seats up to date, as an administrator
/// sees it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UpdateSettings {
    /// The server's own version.
    pub running: String,
    /// `stable`, or `early` for an office that tries new versions first.
    pub channel: String,
    /// When the publisher's feed was last asked, RFC 3339. Absent if never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<String>,
    /// What came of it, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The newest viewer the server is offering its seats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offering: Option<String>,
}

/// Whether Excalibur Fleet may watch this server.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FleetAccess {
    pub on: bool,
    /// Only when it has just been turned on, and never again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Where the office server tells another program that a drawing set's
/// takeoff changed (`notices` in the server).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ChangeNotice {
    pub id: String,
    /// Where the notices go: FabWire's address for them, say.
    pub url: String,
    pub created: String,
    #[serde(default)]
    pub created_by: String,
    /// How the last one went: `delivered`, or what went wrong.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_at: Option<String>,
    /// What every notice is signed with. Given once, when it is made, and
    /// never again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

/// A key for a program rather than a person: an assistant reading the
/// takeoffs, or another system — FabWire — using the API.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: String,
    /// What somebody called it: "the assistant on the estimator's PC", "FabWire".
    pub name: String,
    /// `assistant`, `integration` or `sync`.
    pub purpose: String,
    /// Whose it is: it can do what they can do, and no more.
    pub person: String,
    pub made: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<String>,
    /// The key itself, only in the answer that made it, and never again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// When it stops working by itself; absent for a key that never does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>, // WHAT: the key's end, if it was given one. WHY: an older server never sends it, so it is optional both ways.
}

/// Asking for a key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewKey {
    pub purpose: String,
    pub name: String,
    /// Stop working this many days from now. Left out, the key never runs out
    /// on its own, which is how every key worked before keys could end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_days: Option<u32>, // WHAT: an optional lifetime for the new key. WHY: off unless asked, so no program that already makes keys is changed.
}

/// A person changing their own password.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangePassword {
    pub current: String,
    pub new: String,
}

/// How a server lets people in, as an administrator sees it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Joining {
    /// `code` — anybody with the code; `open` — anybody who can reach it;
    /// `closed` — an administrator adds people and nobody else.
    pub how: String,
    /// Only shown to an administrator. Empty for anybody else.
    #[serde(default)]
    pub code: String,
    /// What a new account gets. `viewer`, `markup` or `admin`.
    pub role: String,
    /// When the code stops working, as an RFC 3339 date and time. Empty
    /// means never, which is what every server did before this.
    #[serde(default)]
    pub until: String,
    /// True when the date has already gone by, so a panel can say so without
    /// having to work out what the server's clock says.
    #[serde(default)]
    pub run_out: bool,
}

// ---- groups of people, and who sees which folder --------------------------

/// A group of people an administrator made: "Shop", "Field". A person can be
/// in several. A folder can be given to groups, so the people in them see it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: String, // WHAT: the group's id, `grp_…`. WHY: members are added and taken off by it.
    pub name: String, // WHAT: what the office calls it. WHY: a folder's rule names groups by name.
    #[serde(default)]
    pub people: Vec<String>, // WHAT: the ids of the people in it. WHY: the Studio ticks them against the people list.
    #[serde(default)]
    pub made: String, // WHAT: when it was made. WHY: shown, and kept for the record.
    #[serde(default)]
    pub made_by: String, // WHAT: who made it, or the program that named it first. WHY: somebody will ask where a group came from.
}

/// Who sees a folder of a project.
///
/// `office`: administrators and estimators only. `everyone`: everybody who
/// may see the project. A list of groups: the people in those groups, and the
/// office always. On the wire it is the word, or the list of names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum See {
    Office, // WHAT: administrators and estimators. WHY: the office's own papers: contracts, prices.
    Everyone, // WHAT: everybody who may see the project. WHY: what every folder was before folders had rules.
    Groups(Vec<String>), // WHAT: the people in these groups, plus the office. WHY: the drawings the shop or the field needs and nothing else.
}

impl See {
    /// Read from what a program or the Studio sent: `"office"`,
    /// `"everyone"`, or a list of group names. An empty list is the office
    /// only. Names are trimmed, kept once whatever their case, and sorted.
    pub fn from_value(value: &serde_json::Value) -> Result<See, String> {
        match value {
            serde_json::Value::String(word) => match word.trim().to_ascii_lowercase().as_str() {
                "office" => Ok(See::Office), // WHAT: the office only. WHY: the word as documented.
                "everyone" => Ok(See::Everyone), // WHAT: everybody on the project. WHY: likewise.
                _ => Err(format!("Who sees a folder is \"office\", \"everyone\" or a list of group names: \"{word}\" isn't one.")), // WHAT: a word nobody defined. WHY: refused rather than guessed, since a guess could open a folder to everybody.
            },
            serde_json::Value::Array(names) => {
                let mut groups: Vec<String> = Vec::new(); // WHAT: the names, once each. WHY: a list said twice is the same list.
                for name in names {
                    let Some(name) = name.as_str() else {
                        return Err("A group's name is text.".into()); // WHAT: a number or an object in the list. WHY: only names name groups.
                    };
                    let name = name.split_whitespace().collect::<Vec<_>>().join(" "); // WHAT: spacing tidied. WHY: " Shop " is the group Shop.
                    if name.is_empty() || name.chars().count() > LONGEST_GROUP_NAME || name.chars().any(char::is_control) {
                        return Err(format!("A group's name is 1 to {LONGEST_GROUP_NAME} characters of ordinary text.")); // WHAT: an empty, huge or odd name. WHY: a name a person can read in the Studio.
                    }
                    if !groups.iter().any(|g| g.eq_ignore_ascii_case(&name)) {
                        groups.push(name); // WHAT: kept, the first spelling. WHY: Shop and shop are one group.
                    }
                }
                groups.sort_by_key(|g| g.to_lowercase()); // WHAT: in order. WHY: the same rule always reads and compares the same.
                if groups.is_empty() { Ok(See::Office) } else { Ok(See::Groups(groups)) } // WHAT: no groups is the office alone. WHY: the office always sees a group's folder, so that is what is left.
            }
            _ => Err("Who sees a folder is \"office\", \"everyone\" or a list of group names.".into()), // WHAT: anything else. WHY: refused, never read as everyone.
        }
    }

    /// The rule as it goes on the wire.
    pub fn to_value(&self) -> serde_json::Value {
        match self {
            See::Office => serde_json::Value::String("office".into()), // WHAT: the word. WHY: as documented.
            See::Everyone => serde_json::Value::String("everyone".into()), // WHAT: the word. WHY: as documented.
            See::Groups(groups) => serde_json::Value::Array(groups.iter().cloned().map(serde_json::Value::String).collect()), // WHAT: the names. WHY: as documented.
        }
    }

    /// What is left of this rule inside a folder whose own rule is `parent`:
    /// a folder is never seen by anybody who can't see the folder it is in.
    pub fn within(&self, parent: &See) -> See {
        match (self, parent) {
            (_, See::Office) | (See::Office, _) => See::Office, // WHAT: the office only, either way. WHY: nothing is narrower.
            (mine, See::Everyone) => mine.clone(), // WHAT: its own rule. WHY: a parent everybody sees takes nothing away.
            (See::Everyone, theirs) => theirs.clone(), // WHAT: the parent's groups. WHY: a child can't be wider than its parent.
            (See::Groups(mine), See::Groups(theirs)) => {
                let both: Vec<String> = mine.iter().filter(|g| theirs.iter().any(|t| t.eq_ignore_ascii_case(g))).cloned().collect(); // WHAT: the groups named on both. WHY: only those people see both folders.
                if both.is_empty() { See::Office } else { See::Groups(both) } // WHAT: none in common is the office alone. WHY: as an empty list is.
            }
        }
    }

    /// Whether this rule lets more people in than `parent` does.
    pub fn wider_than(&self, parent: &See) -> bool {
        self.within(parent) != *self // WHAT: narrowing it to the parent changed it. WHY: then some of it is wider.
    }

    /// The rule in words, for the Studio: "Everyone", "Office only", "Shop,
    /// Field and the office".
    pub fn words(&self) -> String {
        match self {
            See::Office => "Office only".into(), // WHAT: plain words. WHY: what the Studio shows.
            See::Everyone => "Everyone on the job".into(), // WHAT: plain words. WHY: likewise.
            See::Groups(groups) => format!("{} and the office", groups.join(", ")), // WHAT: the groups, then the office. WHY: the office always sees it, and should be told so.
        }
    }
}

impl Serialize for See {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(serializer) // WHAT: the word or the list. WHY: one shape on the wire everywhere.
    }
}

impl<'de> Deserialize<'de> for See {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<See, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?; // WHAT: whatever came. WHY: a word or a list.
        See::from_value(&value).map_err(serde::de::Error::custom) // WHAT: read as a rule. WHY: refused in the same words as everywhere else.
    }
}

/// The longest a group's name may be.
pub const LONGEST_GROUP_NAME: usize = 60; // WHAT: sixty characters. WHY: long enough for any crew's name, short enough to show on a line.

/// One folder of a project, as `GET /projects/{id}/folders` lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FolderInfo {
    pub folder: String, // WHAT: the folder, as folders written with `/`. WHY: the same spelling a set's `folder` uses.
    pub see: See, // WHAT: who sees it now, its parents' rules counted. WHY: what actually decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub own: Option<See>, // WHAT: its own rule, when it has one; none when it takes its parent's. WHY: the Studio shows which folder a rule was set on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_in: Option<String>, // WHAT: `program` or `view`: where its rule was last set. WHY: a rule changed in the Studio stays until the program sets that folder again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_by: Option<String>, // WHAT: who set it. WHY: somebody will ask who opened a folder to the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_at: Option<String>, // WHAT: when. WHY: likewise.
    #[serde(default)]
    pub sets: usize, // WHAT: the current drawing sets in it, not in its subfolders. WHY: the Studio shows an empty folder as empty.
    #[serde(default)]
    pub files: usize, // WHAT: the current other files in it. WHY: likewise.
}

/// A file in a project's folder that is not a drawing set: a Word or Excel
/// file, a picture, a model. Kept by its bytes, as a drawing is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectFile {
    pub id: String, // WHAT: `file_…`. WHY: moved and downloaded by it.
    pub project: String, // WHAT: its project. WHY: every file is in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>, // WHAT: the folder it is in. WHY: shown there; none is the project's own.
    pub name: String, // WHAT: its name with its extension. WHY: the kind of file is told by it.
    pub bytes: u64, // WHAT: its size. WHY: shown, as a file window does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>, // WHAT: when it was saved where it came from, RFC 3339. WHY: shown, as a file window does.
    pub digest: String, // WHAT: SHA-256 of its bytes. WHY: the same bytes are one file, and a copy can be checked.
    pub uploaded: String, // WHAT: when it arrived here. WHY: kept for the record.
    pub uploaded_by: String, // WHAT: who sent it. WHY: likewise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>, // WHAT: the file this one replaced: the same name in the same folder, saved again. WHY: the old one is kept as its history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<String>, // WHAT: the file that replaced this one. WHY: a listing leaves it off unless asked for everything.
    #[serde(default)]
    pub already_here: bool, // WHAT: true when these bytes were already in the project. WHY: nothing new was made, and this is the one there.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_never_seen_by_more_people_than_the_folder_it_is_in() {
        let shop = See::Groups(vec!["Shop".into()]); // WHAT: the shop's folder. WHY: a parent narrower than everyone.
        let both = See::Groups(vec!["Field".into(), "Shop".into()]); // WHAT: the shop and the field. WHY: wider than the shop alone.
        assert_eq!(both.within(&shop), shop); // WHAT: narrowed to the shop. WHY: the field can't see inside a shop folder.
        assert!(both.wider_than(&shop)); // WHAT: said to be wider. WHY: the answer says so.
        assert_eq!(See::Everyone.within(&See::Office), See::Office); // WHAT: everyone inside the office's folder. WHY: the office only.
        assert_eq!(See::Groups(vec!["Field".into()]).within(&shop), See::Office); // WHAT: no group in common. WHY: the office alone is left.
        assert!(!shop.wider_than(&See::Everyone)); // WHAT: narrower than its parent. WHY: allowed, and nothing said.
    }

    #[test]
    fn who_sees_a_folder_travels_as_a_word_or_a_list() {
        let read = |text: &str| See::from_value(&serde_json::from_str(text).unwrap()); // WHAT: a rule read from JSON. WHY: as it arrives.
        assert_eq!(read("\"office\""), Ok(See::Office)); // WHAT: the word. WHY: documented.
        assert_eq!(read("\" Everyone \""), Ok(See::Everyone)); // WHAT: any case and spacing. WHY: a person typed it.
        assert_eq!(read("[\"shop\",\"Field\",\"Shop\"]"), Ok(See::Groups(vec!["Field".into(), "shop".into()]))); // WHAT: once each, in order. WHY: the same list always compares the same.
        assert_eq!(read("[]"), Ok(See::Office)); // WHAT: no groups. WHY: the office alone.
        assert!(read("\"all\"").is_err()); // WHAT: a word nobody defined. WHY: never read as everyone.
        assert!(read("3").is_err()); // WHAT: a number. WHY: likewise.
        assert_eq!(serde_json::to_string(&See::Groups(vec!["Shop".into()])).unwrap(), "[\"Shop\"]"); // WHAT: written back as a list. WHY: the same shape both ways.
    }

    #[test]
    fn a_takeoff_that_left_something_out_says_so() {
        let short = Takeoff {
            set: "s1".into(),
            revision: 4,
            rows: Vec::new(),
            groups: Vec::new(),
            picks: 3,
            pounds: Some(1560.0),
            tons: Some(0.78),
            left_out: 2,
            sheets_without_a_scale: vec!["S-201".into()],
        };
        assert!(short.is_short());
        let text = serde_json::to_string(&short).unwrap();
        assert!(text.contains("\"left_out\":2"));
        assert!(text.contains("S-201"));
    }

    #[test]
    fn an_unscaled_row_carries_no_numbers_at_all() {
        let row = TakeoffRow {
            markup: "m1".into(),
            page: 3,
            sheet: "S-201".into(),
            subject: "W12x26".into(),
            kind: "Length".into(),
            caption: String::new(),
            author: "Sam".into(),
            scaled: false,
            count: 1.0,
            length_feet: None,
            area_square_feet: None,
            volume_cubic_feet: None,
            pounds: None,
        };
        let text = serde_json::to_string(&row).unwrap();
        // The fields are absent, not zero. A zero would be read as a measurement.
        assert!(!text.contains("length_feet"), "{text}");
        assert!(!text.contains("pounds"), "{text}");
        assert!(text.contains("\"scaled\":false"));
    }

    #[test]
    fn a_viewer_cannot_write_and_an_admin_can_do_both() {
        assert!(!Role::Viewer.may_write());
        assert!(Role::Estimator.may_write());
        assert!(!Role::Estimator.may_administer());
        assert!(Role::Admin.may_write() && Role::Admin.may_administer());
    }

    #[test]
    fn roles_travel_as_the_words_people_use() {
        assert_eq!(serde_json::to_string(&Role::Estimator).unwrap(), "\"estimator\"");
        let back: Role = serde_json::from_str("\"admin\"").unwrap();
        assert_eq!(back, Role::Admin);
    }
}
