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
    /// `assistant` or `integration`.
    pub purpose: String,
    /// Whose it is: it can do what they can do, and no more.
    pub person: String,
    pub made: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<String>,
    /// The key itself, only in the answer that made it, and never again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Asking for a key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewKey {
    pub purpose: String,
    pub name: String,
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

#[cfg(test)]
mod tests {
    use super::*;

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
