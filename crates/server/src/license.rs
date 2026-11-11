//! Where this office stands with Excalibur View Office.
//!
//! Four states, worked out from three settings and the clock:
//!
//! - **founding**: a server that was already in use before licensing existed.
//!   The first customers' servers are these. Every user, every
//!   update, for good, with no file to lose. See [`FOUNDING_BEFORE`].
//! - **licensed**: a signed license file has been added and checks out.
//! - **trial**: the first [`TRIAL_DAYS`] days, with everything on.
//! - **unlicensed**: the trial is over and there is no license. Everything
//!   already on the server can still be opened, printed and exported; only
//!   sharing *new* work through it pauses. Nothing is deleted, and nobody is
//!   signed out.
//!
//! And one more, which only a person can bring about: a licensed server whose
//! program was built after the license's updates ended — a newer version
//! installed by hand, since the server never updates itself past them. It is
//! treated like an ended trial, with the two ways out said plainly: renew, or
//! put back a version the license covers. Everything the license covers keeps
//! working for good, which is the promise on the website; a version it does
//! not cover is not one the office bought.
//!
//! A license is checked here, on the office's own machine, against the keys
//! built into the program. Nothing is sent anywhere to do it.

use hub::license::{Edition, License, Standing, BUY_URL};
use hub::update::Trusted;

use crate::api::{now, Server};

/// How long a new office server runs with everything on before it needs a
/// license.
pub const TRIAL_DAYS: i64 = 30;

/// A server whose first account was made before this, and which already had
/// people on it the first time a licensing version started, is a founding
/// install: the first customers, who took Hyperview before it was sold.
/// They are never asked for a license.
///
/// Both halves matter. A fresh server has nobody on it when licensing first
/// starts, so it gets the trial however its clock is set; and an old version
/// set up after this date is not grandfathered by being upgraded.
pub const FOUNDING_BEFORE: &str = "2026-10-01T00:00:00Z";

/// The day this server program was built (see `build.rs`).
pub const BUILT: &str = env!("HYPERVIEW_BUILT");

/// How long before a license's updates end its administrators are told.
pub const RENEWAL_NOTICE_DAYS: i64 = 30;

const BEGAN: &str = "licensing_began";
const FOUNDING: &str = "founding_install";
const LICENSE: &str = "license";

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Founding,
    Licensed(License),
    /// Licensed, but this program was built after the license's updates
    /// ended: a newer server put in by hand.
    Beyond(License),
    Trial { days_left: u32 },
    Unlicensed,
}

impl State {
    pub fn sharing(&self) -> bool {
        !matches!(self, State::Unlicensed | State::Beyond(_))
    }
}

/// Called once as the server opens. Records when licensing began on this
/// server, decides once and for all whether it is a founding install, and
/// picks up a license file left in the data folder.
pub fn begin(server: &Server) {
    begin_at(server, &now(), FOUNDING_BEFORE);
    settle_sealed(server);
    pick_up_from_the_data_folder(server);
}

/// The fingerprints of licence files already taken from the data folder.
const FOLDER_FILES_SEEN: &str = "license_files_seen";

/// Adds a licence file somebody left in the data folder, once.
///
/// The file stays where it was put, so the server meets it again at every
/// start. Installing it every time would quietly undo whatever came after
/// it: the renewal the server fetched for itself, or the newer licence an
/// administrator added from a seat, put back to the old seats and the old
/// end of updates by a restart. So a file is taken the first time it is
/// seen, or later only when it is a renewal of the licence held.
fn pick_up_from_the_data_folder(server: &Server) {
    let Ok(entries) = std::fs::read_dir(&server.config.data) else { return }; // WHAT: the data folder's files. WHY: nothing there, nothing to do
    let mut paths: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(hub::license::EXTENSION))
        .collect(); // WHAT: every .evlicense file there. WHY: the only kind a person drops in for the server
    paths.sort(); // WHAT: by name. WHY: the same order at every start, whatever the disk lists first
    let mut seen: Vec<String> = server
        .store
        .setting(FOLDER_FILES_SEEN)
        .ok()
        .flatten()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default(); // WHAT: the files already taken. WHY: one is taken once, not at every start
    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else { continue }; // WHAT: the file's text. WHY: one that can't be read is left alone
        let fresh = match License::read(&text, &trust(server)) {
            Ok(fresh) => fresh, // WHAT: a licence that checks out. WHY: only those are weighed up
            Err(why) => {
                tracing::warn!("{} was not used: {why}", path.display()); // WHAT: why, in the log. WHY: whoever dropped it can find out
                continue;
            }
        };
        let print = fingerprint(&text); // WHAT: this file's fingerprint. WHY: whether it was taken before
        let held = held(server, &trust(server)); // WHAT: the licence in use now. WHY: a file is never allowed to make it worse
        if folder_file_wanted(held.as_ref(), &fresh, seen.contains(&print)) {
            match install(server, &text) {
                Ok(license) => tracing::info!("license {} for {} added from {}", license.id, license.company, path.display()), // WHAT: said in the log. WHY: as before
                Err(why) => tracing::warn!("{} was not used: {why}", path.display()), // WHAT: said in the log. WHY: as before
            }
        } else {
            tracing::info!("{} is already in use, or older than the license this server holds; left alone", path.display()); // WHAT: said, and nothing changed. WHY: a restart must never take seats or updates away
        }
        if !seen.contains(&print) {
            seen.push(print); // WHAT: remembered. WHY: met again at the next start, it is not taken again
        }
    }
    if let Ok(text) = serde_json::to_string(&seen) {
        let _ = server.store.set_setting(FOLDER_FILES_SEEN, &text); // WHAT: kept. WHY: across restarts
    }
}

/// Whether a licence file found in the data folder should be put in use.
///
/// Nothing held: yes. The same licence as the one held: only when it is a
/// renewal of it (more seats, longer updates), never the same or worse. A
/// different licence: the first time its file is seen, because a person put
/// it there on purpose; never again after, so it cannot undo a later one.
pub fn folder_file_wanted(held: Option<&License>, fresh: &License, seen_before: bool) -> bool {
    match held {
        None => true, // WHAT: no licence in use. WHY: what a file in the data folder is for
        Some(held) if held.id == fresh.id => is_a_renewal(held, fresh), // WHAT: the same licence, only if better. WHY: an old copy must not undo a renewal
        Some(_) => !seen_before, // WHAT: another licence, once. WHY: dropped in on purpose, but never re-applied over what came after
    }
}

/// SHA-256 of a licence file's text, as hex.
fn fingerprint(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.trim().as_bytes()).iter().map(|b| format!("{b:02x}")).collect() // WHAT: the trimmed text's digest. WHY: a trailing newline added by an editor is the same file
}

/// Tells the `sealed` module what this server's license says, so everything
/// that would otherwise reach the outside world knows before it tries.
/// Called as the server opens and again whenever a license is installed.
pub fn settle_sealed(server: &Server) {
    let edition = held(server, &trust(server)).map(|l| l.edition);
    hub::sealed::license_says(edition.map(|e| e.name()));
}

pub fn begin_at(server: &Server, now: &str, founding_before: &str) {
    let store = &server.store;
    if store.setting(BEGAN).ok().flatten().is_some() {
        return;
    }
    let earliest: Option<String> = store
        .with(|db| Ok(db.query_row("SELECT min(created) FROM people", [], |r| r.get(0))?))
        .ok()
        .flatten();
    if let Some(first) = earliest.filter(|first| first.as_str() < founding_before) {
        let _ = store.set_setting(FOUNDING, &first);
        tracing::info!("in use since {first}, before licensing: a founding install, never asked for a license");
    }
    let _ = store.set_setting(BEGAN, now);
}

/// Checks a license file and keeps it. The error is a sentence for whoever
/// chose the file.
pub fn install(server: &Server, text: &str) -> Result<License, String> {
    install_with(server, text, &trust(server))
}

fn install_with(server: &Server, text: &str, trusted: &Trusted) -> Result<License, String> {
    let license = License::read(text, trusted)?;
    let kept = serde_json::to_string(&license).map_err(|e| e.to_string())?;
    server
        .store
        .set_setting(LICENSE, &kept)
        .map_err(|_| "The server could not keep that license. Nothing was changed.".to_string())?;
    // A Sealed license takes effect the moment it is added, not at the next
    // restart: an office that has just sealed itself should not spend the
    // afternoon still reaching out.
    hub::sealed::license_says(Some(license.edition.name()));
    Ok(license)
}

// ---- a license that renews itself -------------------------------------------
//
// A shop that buys three more seats should not wait for somebody to sign a
// file and email it. The license already carries an id; a server that knows
// its own can go and fetch its own renewal.
//
// What makes this safe is that nothing here decides a license is good. The
// file that comes back is checked against the keys compiled into this server,
// exactly as an update is, and then checked again against the one already
// held: a renewal that gives fewer seats or ends sooner is ignored rather
// than installed. So a feed that was taken over, or an old file served by
// mistake, can only ever hand a shop something it was already entitled to.

/// Where this server would look for its own renewal, or `None` when it
/// should not look at all.
///
/// Nothing is sent: the address is derived from the license id this server
/// already holds, so the request says only "the file at this name", and the
/// name means nothing to anybody who does not have the id.
pub fn renewal_url(server: &Server) -> Option<String> {
    let base = crate::LICENSE_FEED.trim().trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    if hub::sealed::is_sealed() {
        return None;
    }
    let held = held(server, &trust(server))?;
    Some(format!("{base}/{}.evlicense", name_for(&held.id)))
}

/// The name a license is published under: the id put through SHA-256, so the
/// address cannot be worked out from a company's name and the list of
/// customers is not something anybody can walk.
pub fn name_for(id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(id.trim().as_bytes());
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether `fresh` is worth installing over `held`.
///
/// The same license, and better than the one in hand, or it is not a renewal.
/// An automatic change that took seats away, or brought the end of updates
/// forward, would cut a shop off in the middle of a bid because a file was
/// stale — so it is refused, and a genuine reduction is installed by hand
/// like everything else that ought to be somebody's decision.
pub fn is_a_renewal(held: &License, fresh: &License) -> bool {
    if fresh.id != held.id || fresh.company != held.company {
        return false;
    }
    let seats_kept = fresh.users == 0 || (held.users != 0 && fresh.users >= held.users);
    let updates_kept = match (held.updates_through.as_str(), fresh.updates_through.as_str()) {
        (_, "forever") => true,
        ("forever", _) => false,
        (was, now) => now >= was,
    };
    let changed = fresh.users != held.users
        || fresh.updates_through != held.updates_through
        || fresh.edition != held.edition;
    seats_kept && updates_kept && changed
}

/// Fetches the renewal, if there is one, and keeps it. Returns what changed,
/// for the log. Any trouble is `None`: a shop whose licence could not be
/// checked today is a shop that carries on with the one it has.
pub fn look_for_renewal(server: &Server) -> Option<String> {
    let url = renewal_url(server)?;
    let held = held(server, &trust(server))?;
    let text = hub::web::fetch_small("Checking for a renewed license", &url).ok()?;
    let fresh = License::read(&text, &trust(server)).ok()?;
    if !is_a_renewal(&held, &fresh) {
        return None;
    }
    install(server, &text).ok()?;
    Some(format!(
        "{} seats, updates through {}",
        if fresh.users == 0 { "every".to_string() } else { fresh.users.to_string() },
        fresh.updates_through
    ))
}

/// The license this server holds, if it still checks out.
fn held(server: &Server, trusted: &Trusted) -> Option<License> {
    let text = server.store.setting(LICENSE).ok().flatten()?;
    // Checked every time rather than trusted once: a license typed into the
    // database by hand is not a license. (Which is also why a key that has
    // signed licenses is never taken out of the program's list.)
    License::read(&text, trusted).ok()
}

/// The keys a licence on this server may be signed with: the compiled-in
/// ones, and a test's own when a test set one (`Config::license_keys_for_tests`).
fn trust(server: &Server) -> Trusted {
    let mut trust = crate::license_trust(); // WHAT: every release and licence key built in. WHY: the real list
    trust.keys.extend(server.config.license_keys_for_tests.iter().cloned()); // WHAT: a test's key, when one is set. WHY: empty in every server started as a program
    trust
}

pub fn state(server: &Server) -> State {
    state_at(server, &now(), &trust(server))
}

pub fn state_at(server: &Server, now: &str, trusted: &Trusted) -> State {
    state_as_built(server, now, trusted, BUILT)
}

/// The state of a server whose program was built on `built`.
pub fn state_as_built(server: &Server, now: &str, trusted: &Trusted, built: &str) -> State {
    if server.store.setting(FOUNDING).ok().flatten().is_some() {
        return State::Founding;
    }
    if let Some(license) = held(server, trusted) {
        return if license.covers(built) { State::Licensed(license) } else { State::Beyond(license) };
    }
    let began = server.store.setting(BEGAN).ok().flatten().unwrap_or_else(|| now.to_string());
    match days_left(&began, now) {
        0 => State::Unlicensed,
        n => State::Trial { days_left: n },
    }
}

/// Whole days of trial left, counting today. Nought when it is over. A clock
/// that has gone backwards gives the full trial, never an early end.
fn days_left(began: &str, now: &str) -> u32 {
    use time::format_description::well_known::Rfc3339;
    let (Ok(began), Ok(now)) = (
        time::OffsetDateTime::parse(began, &Rfc3339),
        time::OffsetDateTime::parse(now, &Rfc3339),
    ) else {
        return TRIAL_DAYS as u32;
    };
    let ends = began + time::Duration::days(TRIAL_DAYS);
    if now >= ends {
        return 0;
    }
    let left = (ends - now).whole_seconds();
    let days = (left + 86_399) / 86_400;
    days.clamp(1, TRIAL_DAYS) as u32
}

fn people(server: &Server) -> u32 {
    server
        .store
        .with(|db| Ok(db.query_row("SELECT count(*) FROM people", [], |r| r.get::<_, i64>(0))?))
        .unwrap_or(0) as u32
}

/// Where the office stands, as the program shows it. Administrators see the
/// license id and the buttons that go with it; everybody else, one sentence.
pub fn standing(server: &Server, administrator: bool) -> Standing {
    standing_of(&state(server), people(server), administrator, founding_company(server), &now())
}

fn founding_company(server: &Server) -> Option<String> {
    let name = crate::api::installation_name(server);
    (!name.trim().is_empty()).then_some(name)
}

/// Whole days from `now` to the end of `day` (YYYY-MM-DD). Negative once it
/// has passed; `None` for something that is not a date.
fn days_until(day: &str, now: &str) -> Option<i64> {
    use time::format_description::well_known::Rfc3339;
    if !hub::license::is_date(day) {
        return None;
    }
    let number = |range: std::ops::Range<usize>| day[range].parse::<i32>().ok();
    let month = time::Month::try_from(number(5..7)? as u8).ok()?;
    let end = time::Date::from_calendar_date(number(0..4)?, month, number(8..10)? as u8).ok()?;
    let now = time::OffsetDateTime::parse(now, &Rfc3339).ok()?.date();
    Some((end - now).whole_days())
}

/// What an administrator is told about a license's updates ending: a month
/// ahead, and after. Nothing stops working either way.
fn renewal_note(license: &License, now: &str) -> Option<String> {
    if license.forever() {
        return None;
    }
    let left = days_until(&license.updates_through, now)?;
    let day = hub::license::long_date(&license.updates_through);
    if left < 0 {
        Some(format!(
            "Updates on your license ended on {day}. Everything keeps working on the version you \
             have; renew to get newer ones."
        ))
    } else if left <= RENEWAL_NOTICE_DAYS {
        Some(format!(
            "Updates on your license run through {day}. Renew before then to keep getting new \
             versions; everything keeps working either way."
        ))
    } else {
        None
    }
}

pub fn standing_of(state: &State, users: u32, administrator: bool, company: Option<String>, now: &str) -> Standing {
    let buy_url = BUY_URL.to_string();
    match state {
        State::Founding => Standing {
            state: "founding".into(),
            edition: Some(Edition::Office.name().into()),
            company,
            users,
            sharing: true,
            message: "Founding install: every user and every update, for good.".into(),
            buy_url,
            ..Default::default()
        },
        State::Beyond(license) => {
            let day = hub::license::long_date(&license.updates_through);
            Standing {
                state: "licensed".into(),
                edition: Some(license.edition.name().into()),
                company: Some(license.company.clone()),
                license_id: administrator.then(|| license.id.clone()),
                users_allowed: (!license.unlimited()).then_some(license.users),
                users,
                updates_through: Some(license.updates_through.clone()),
                sharing: false,
                message: if administrator {
                    format!(
                        "This server is Excalibur View {}, which came out after the updates on your \
                         license ended ({day}). Nothing is deleted: everyone can still open and export \
                         every drawing and markup. Sharing new work is paused until you renew and add \
                         the new license file, or put back a version from on or before {day} (release \
                         history, on the download page).",
                        env!("CARGO_PKG_VERSION")
                    )
                } else {
                    "Sharing is paused. Your administrator has been told.".into()
                },
                buy_url,
                ..Default::default()
            }
        }
        State::Licensed(license) => {
            let users_allowed = (!license.unlimited()).then_some(license.users);
            let mut notes = Vec::new();
            if let Some(n) = users_allowed.filter(|n| users >= *n && administrator) {
                notes.push(format!(
                    "All {n} users on your license are in use. Add users to your license before \
                     adding another person. Nobody already signed in is affected."
                ));
            }
            if administrator {
                notes.extend(renewal_note(license, now));
            }
            let message = notes.join(" ");
            Standing {
                state: "licensed".into(),
                edition: Some(license.edition.name().into()),
                company: Some(license.company.clone()),
                license_id: administrator.then(|| license.id.clone()),
                users_allowed,
                users,
                updates_through: (!license.forever()).then(|| license.updates_through.clone()),
                sharing: true,
                message,
                buy_url,
                ..Default::default()
            }
        }
        State::Trial { days_left } => Standing {
            state: "trial".into(),
            users,
            trial_days_left: Some(*days_left),
            sharing: true,
            message: if administrator {
                format!(
                    "Office trial: {days_left} day{} left. Everything works. Add a license any time \
                     under Studio → Office → License.",
                    if *days_left == 1 { "" } else { "s" }
                )
            } else {
                String::new()
            },
            buy_url,
            ..Default::default()
        },
        State::Unlicensed => Standing {
            state: "unlicensed".into(),
            users,
            trial_days_left: Some(0),
            sharing: false,
            message: if administrator {
                "Your Office trial has ended. Nothing is deleted: everyone can still open and \
                 export every drawing and markup. Sharing new work is paused until you add a \
                 license."
                    .into()
            } else {
                "Sharing is paused. Your administrator has been told.".into()
            },
            buy_url,
            ..Default::default()
        },
    }
}

/// Whether a version of the office server published on `published` comes
/// with this office's updates. The desktop app is free and is always offered.
pub fn covers(server: &Server, published: &str) -> bool {
    covered_by(&state(server), published)
}

fn covered_by(state: &State, published: &str) -> bool {
    match state {
        State::Founding | State::Trial { .. } => true,
        State::Licensed(license) | State::Beyond(license) => license.covers(published),
        State::Unlicensed => false,
    }
}

/// Whether there is room on the license for one more person.
pub fn room_for_another(server: &Server) -> Result<(), String> {
    room_in(&state(server), people(server))
}

fn room_in(state: &State, people: u32) -> Result<(), String> {
    if let State::Licensed(license) | State::Beyond(license) = state {
        if !license.unlimited() && people >= license.users {
            return Err(format!(
                "Your license covers {} users and all {} are in use. Add users to your license, \
                 or remove someone who has left. Nobody already signed in is affected.",
                license.users, license.users
            ));
        }
    }
    Ok(())
}

/// Whether new work can be shared through the server right now.
pub fn sharing(server: &Server) -> Result<(), String> {
    match state(server) {
        State::Unlicensed => Err("This office's Excalibur View Office trial has ended, so sharing new work \
             through the server is paused. Everything already here can still be opened and exported. \
             An administrator can add a license under Studio → Office → License."
            .into()),
        State::Beyond(license) => Err(format!(
            "This office server is a newer version than its license's updates cover (they ended \
             {}), so sharing new work through it is paused. Everything already here can still be \
             opened and exported. An administrator can renew and add the new license, or put back \
             an earlier version.",
            hub::license::long_date(&license.updates_through)
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, Store};

    fn a_server() -> (Server, std::path::PathBuf) {
        let n: u64 = rand::random();
        let dir = std::env::temp_dir().join(format!("hv-license-{n:016x}"));
        std::fs::create_dir_all(&dir).unwrap();
        let config = Config { data: dir.clone(), ..Config::default() };
        let store = Store::open(&config.database(), &config.blobs()).unwrap();
        (
            Server {
                store,
                config,
                updates: Default::default(),
                patience: Default::default(),
                tunnel: Default::default(),
                manifests: Default::default(),
                notices: Default::default(),
                key_refusals_logged: Default::default(), // WHAT: none written. WHY: the new field, empty as on every start.
            },
            dir,
        )
    }

    const FOREVER_THROUGH: &str = hub::license::FOREVER;

    fn nobody() -> Trusted {
        Trusted { keys: Vec::new() }
    }

    fn issuer() -> (ed25519_dalek::SigningKey, Trusted) {
        // Made up for the test. The real key is on the publisher's own PC.
        let signing = ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]);
        let trusted = Trusted { keys: vec![("mesafab-2026".into(), signing.verifying_key().to_bytes())] };
        (signing, trusted)
    }

    fn a_license(signing: &ed25519_dalek::SigningKey, users: u32, through: &str) -> String {
        use ed25519_dalek::Signer;
        let mut license = License {
            id: "evl_test".into(),
            company: "Example Builders".into(),
            edition: Edition::Office,
            users,
            updates_through: through.into(),
            issued: "2026-10-02".into(),
            note: String::new(),
            key: "mesafab-2026".into(),
            signature: String::new(),
        };
        license.signature =
            signing.sign(&license.signing_payload()).to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        serde_json::to_string(&license).unwrap()
    }

    fn someone(server: &Server, created: &str) {
        server
            .store
            .with(|db| {
                db.execute(
                    "INSERT INTO people (id, name, email, password, role, created)
                     VALUES (?1, 'A', ?2, 'x', 'admin', ?3)",
                    rusqlite::params![crate::api::fresh_id("usr"), format!("{}@x.com", crate::api::fresh_id("e")), created],
                )?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_server_in_use_before_licensing_is_never_asked_to_pay() {
        let (server, dir) = a_server();
        someone(&server, "2026-06-01T12:00:00Z");
        begin_at(&server, "2026-10-20T00:00:00Z", FOUNDING_BEFORE);
        // Years later, with no license file anywhere: still everything.
        let state = state_at(&server, "2031-01-01T00:00:00Z", &nobody());
        assert_eq!(state, State::Founding);
        assert!(state.sharing());
        assert!(room_for_another(&server).is_ok());
        assert!(covers(&server, "2031-01-01"));
        // Starting again does not re-decide it.
        begin_at(&server, "2031-01-01T00:00:00Z", FOUNDING_BEFORE);
        assert_eq!(state_at(&server, "2031-01-02T00:00:00Z", &nobody()), State::Founding);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_server_gets_the_trial_then_keeps_everything_readable() {
        let (server, dir) = a_server();
        // Nobody on it yet the first time licensing starts, whatever the date.
        begin_at(&server, "2026-09-01T00:00:00Z", FOUNDING_BEFORE);
        someone(&server, "2026-09-01T00:05:00Z");
        assert_eq!(state_at(&server, "2026-09-01T01:00:00Z", &nobody()), State::Trial { days_left: 30 });
        assert_eq!(state_at(&server, "2026-09-30T12:00:00Z", &nobody()), State::Trial { days_left: 1 });
        let over = state_at(&server, "2026-10-01T00:00:01Z", &nobody());
        assert_eq!(over, State::Unlicensed);
        assert!(!over.sharing());
        let seen = standing_of(&over, 3, false, None, "2026-10-01T00:00:01Z");
        assert_eq!(seen.message, "Sharing is paused. Your administrator has been told.");
        let admin = standing_of(&over, 3, true, None, "2026-10-01T00:00:01Z");
        assert!(admin.message.contains("Nothing is deleted"), "{}", admin.message);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_clock_turned_back_never_ends_a_trial_early() {
        assert_eq!(days_left("2026-09-10T00:00:00Z", "2026-09-01T00:00:00Z"), 30);
        assert_eq!(days_left("not a date", "2026-09-01T00:00:00Z"), 30);
    }

    #[test]
    fn a_founding_install_needs_people_from_before_the_date() {
        let (server, dir) = a_server();
        // An old version set up after the date, then upgraded: an ordinary trial.
        someone(&server, "2026-11-01T00:00:00Z");
        begin_at(&server, "2026-11-02T00:00:00Z", FOUNDING_BEFORE);
        assert!(matches!(state_at(&server, "2026-11-02T01:00:00Z", &nobody()), State::Trial { .. }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_license_nobody_signed_is_not_a_license() {
        let (server, dir) = a_server();
        begin_at(&server, "2026-11-02T00:00:00Z", FOUNDING_BEFORE);
        let forged = r#"{"id":"evl_1","company":"Me","edition":"office","users":0,
            "updates_through":"forever","issued":"2026-11-02","key":"mesafab-2026","signature":"00"}"#;
        assert!(install(&server, forged).is_err());
        // Typed straight into the database: still not a license.
        server.store.set_setting(LICENSE, forged).unwrap();
        assert!(matches!(state_at(&server, "2026-11-03T00:00:00Z", &nobody()), State::Trial { .. }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_newer_server_put_in_by_hand_is_held_to_the_license_it_has() {
        // Updates through 1 Nov 2027. The server never updates itself past
        // that; somebody installing a later build by hand gets the same
        // treatment as an ended trial, and the way out is said plainly.
        let (server, dir) = a_server();
        let (signing, trusted) = issuer();
        begin_at(&server, "2026-11-01T00:00:00Z", FOUNDING_BEFORE);
        someone(&server, "2026-11-01T00:00:00Z");
        install_with(&server, &a_license(&signing, 5, "2027-11-01"), &trusted).unwrap();

        let covered = state_as_built(&server, "2028-01-01T00:00:00Z", &trusted, "2027-10-20");
        assert!(matches!(covered, State::Licensed(_)) && covered.sharing(), "a covered build keeps working for good");
        let same_day = state_as_built(&server, "2028-01-01T00:00:00Z", &trusted, "2027-11-01");
        assert!(same_day.sharing(), "the last day of updates is still covered");

        let beyond = state_as_built(&server, "2028-01-01T00:00:00Z", &trusted, "2027-11-02");
        assert!(matches!(beyond, State::Beyond(_)));
        assert!(!beyond.sharing(), "sharing pauses");
        assert!(room_in(&beyond, 1).is_ok(), "still the same license for counting people");
        let admin = standing_of(&beyond, 1, true, None, "2028-01-01T00:00:00Z");
        assert_eq!(admin.state, "licensed");
        assert!(!admin.sharing);
        assert!(admin.message.contains("Nov 1, 2027") && admin.message.contains("renew") && admin.message.contains("put back"), "{}", admin.message);
        let seen = standing_of(&beyond, 1, false, None, "2028-01-01T00:00:00Z");
        assert_eq!(seen.message, "Sharing is paused. Your administrator has been told.");
        assert!(seen.license_id.is_none());

        // A founding install is never held to anything.
        let founding = State::Founding;
        assert!(founding.sharing());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn administrators_hear_about_updates_ending_a_month_ahead_and_after() {
        let (signing, trusted) = issuer();
        let license = License::read(&a_license(&signing, 5, "2027-11-01"), &trusted).unwrap();
        let state = State::Licensed(license.clone());
        let at = |now: &str, admin: bool| standing_of(&state, 2, admin, None, now).message;

        assert_eq!(at("2027-09-01T00:00:00Z", true), "", "two months out: nothing yet");
        assert!(at("2027-10-05T00:00:00Z", true).contains("run through Nov 1, 2027"), "{}", at("2027-10-05T00:00:00Z", true));
        assert!(at("2027-11-01T23:00:00Z", true).contains("run through"), "the last day still counts");
        assert!(at("2027-11-02T00:00:00Z", true).contains("ended on Nov 1, 2027"));
        assert!(at("2027-11-02T00:00:00Z", true).contains("keeps working"));
        assert_eq!(at("2027-10-05T00:00:00Z", false), "", "only administrators, who can do something about it");

        let forever = License::read(&a_license(&signing, 0, FOREVER_THROUGH), &trusted).unwrap();
        assert_eq!(standing_of(&State::Licensed(forever), 2, true, None, "2090-01-01T00:00:00Z").message, "");
    }

    #[test]
    fn a_license_turns_an_ended_trial_back_on_and_counts_users() {
        let (server, dir) = a_server();
        let (signing, trusted) = issuer();
        begin_at(&server, "2026-11-01T00:00:00Z", FOUNDING_BEFORE);
        for n in 0..3 {
            someone(&server, &format!("2026-11-0{}T00:00:00Z", n + 1));
        }
        assert_eq!(state_at(&server, "2027-01-01T00:00:00Z", &trusted), State::Unlicensed);

        install_with(&server, &a_license(&signing, 3, "2027-11-01"), &trusted).expect("a good license");
        let state = state_at(&server, "2027-01-01T00:00:00Z", &trusted);
        assert!(matches!(state, State::Licensed(_)));
        assert!(state.sharing(), "sharing is back on");
        // Three of three in use: the fourth waits. Nobody is taken off.
        assert!(room_in(&state, 3).unwrap_err().contains("all 3 are in use"));
        assert!(room_in(&state, 2).is_ok());
        // Server versions published after the updates end are not taken.
        assert!(covered_by(&state, "2027-11-01T09:00:00Z"));
        assert!(!covered_by(&state, "2027-11-02T00:00:00Z"));

        let seen = standing_of(&state, 3, true, None, "2027-01-01T00:00:00Z");
        assert_eq!(seen.state, "licensed");
        assert_eq!(seen.license_id.as_deref(), Some("evl_test"));
        assert_eq!(seen.users_allowed, Some(3));
        assert_eq!(seen.updates_through.as_deref(), Some("2027-11-01"));
        assert!(standing_of(&state, 3, false, None, "2027-01-01T00:00:00Z").license_id.is_none(), "only administrators see the id");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_founding_license_file_is_every_user_and_every_update() {
        let (server, dir) = a_server();
        let (signing, trusted) = issuer();
        begin_at(&server, "2026-11-01T00:00:00Z", FOUNDING_BEFORE);
        install_with(&server, &a_license(&signing, 0, "forever"), &trusted).unwrap();
        let state = state_at(&server, "2040-01-01T00:00:00Z", &trusted);
        assert!(room_in(&state, 500).is_ok());
        assert!(covered_by(&state, "2040-01-01"));
        let seen = standing_of(&state, 500, true, None, "2040-01-01T00:00:00Z");
        assert_eq!(seen.users_allowed, None);
        assert_eq!(seen.updates_through, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The name the folder tests' key goes by. Not a real key's name: the
    /// server looks a key up by name, and the built-in one would answer first.
    const UNIT_KEY: &str = "license-unit-test";

    /// A licence signed with the test key under [`UNIT_KEY`], with its own id.
    fn a_license_called(signing: &ed25519_dalek::SigningKey, id: &str, users: u32, through: &str) -> String {
        let mut license: License = serde_json::from_str(&a_license(signing, users, through)).unwrap(); // WHAT: the usual test licence. WHY: only the id and key name differ
        license.id = id.into(); // WHAT: its id. WHY: a different licence, or a newer copy of one
        license.key = UNIT_KEY.into(); // WHAT: the test key's own name. WHY: the built-in mesafab-2026 would be found first and refuse it
        use ed25519_dalek::Signer;
        license.signature = signing.sign(&license.signing_payload()).to_bytes().iter().map(|b| format!("{b:02x}")).collect(); // WHAT: signed again. WHY: the id is in the signed text
        serde_json::to_string_pretty(&license).unwrap()
    }

    fn users_held(server: &Server, trusted: &Trusted) -> Option<(String, u32, String)> {
        held(server, trusted).map(|l| (l.id, l.users, l.updates_through)) // WHAT: id, seats and end of updates. WHY: what a restart must not take away
    }

    #[test]
    fn a_license_file_in_the_data_folder_is_taken_once_and_never_undoes_a_later_one() {
        let (mut server, dir) = a_server();
        let (signing, _) = issuer();
        let trusted = Trusted { keys: vec![(UNIT_KEY.into(), signing.verifying_key().to_bytes())] }; // WHAT: the test key under its own name. WHY: see UNIT_KEY
        server.config.license_keys_for_tests = trusted.keys.clone(); // WHAT: the test key, as a real server has its built-in ones. WHY: begin() checks with the server's own keys
        std::fs::write(dir.join("Example Builders.evlicense"), a_license_called(&signing, "evl_test", 3, "2027-11-01")).unwrap(); // WHAT: the file the customer was sent, dropped in. WHY: one of the ways a licence arrives
        begin(&server);
        assert_eq!(users_held(&server, &trusted), Some(("evl_test".into(), 3, "2027-11-01".into())), "picked up at start");

        // WHAT: three more seats and another year arrive by themselves (the renewal). WHY: the file in the folder is now the old one
        install_with(&server, &a_license_called(&signing, "evl_test", 6, "2028-11-01"), &trusted).unwrap();
        begin(&server); // WHAT: the server restarts with the old file still there. WHY: the case this guards
        assert_eq!(users_held(&server, &trusted), Some(("evl_test".into(), 6, "2028-11-01".into())), "a restart never puts the old seats back");

        // WHAT: a renewal dropped in the folder by hand. WHY: it is better than what is held, so it is taken
        std::fs::write(dir.join("Example Builders renewed.evlicense"), a_license_called(&signing, "evl_test", 9, "2028-11-01")).unwrap();
        begin(&server);
        assert_eq!(users_held(&server, &trusted).map(|h| h.1), Some(9), "a better copy of the same licence is taken");

        // WHAT: a different licence dropped in. WHY: a person put it there on purpose, so it is used, once
        std::fs::write(dir.join("New licence.evlicense"), a_license_called(&signing, "evl_other", 4, "2029-01-01")).unwrap();
        begin(&server);
        assert_eq!(users_held(&server, &trusted).map(|h| h.0), Some("evl_other".into()));
        install_with(&server, &a_license_called(&signing, "evl_test", 12, "2030-01-01"), &trusted).unwrap(); // WHAT: an administrator adds another from a seat. WHY: the latest decision
        begin(&server); // WHAT: restart, both old files still in the folder. WHY: neither may undo it
        assert_eq!(users_held(&server, &trusted), Some(("evl_test".into(), 12, "2030-01-01".into())), "the licence added last stays");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_folder_file_is_wanted_when_nothing_is_held_or_it_is_better_and_never_when_worse() {
        let (signing, _) = issuer();
        let trusted = Trusted { keys: vec![(UNIT_KEY.into(), signing.verifying_key().to_bytes())] }; // WHAT: the test key under its own name. WHY: see UNIT_KEY
        let read = |text: String| License::read(&text, &trusted).unwrap(); // WHAT: a signed licence. WHY: the rule is about real ones
        let held = read(a_license_called(&signing, "evl_test", 6, "2027-11-01"));
        assert!(folder_file_wanted(None, &held, true), "nothing held: always");
        assert!(!folder_file_wanted(Some(&held), &held, false), "the same licence again: no");
        assert!(!folder_file_wanted(Some(&held), &read(a_license_called(&signing, "evl_test", 3, "2027-11-01")), false), "fewer seats: no");
        assert!(!folder_file_wanted(Some(&held), &read(a_license_called(&signing, "evl_test", 6, "2027-01-01")), false), "earlier end of updates: no");
        assert!(folder_file_wanted(Some(&held), &read(a_license_called(&signing, "evl_test", 9, "2027-11-01")), true), "more seats: yes, even seen before");
        let other = read(a_license_called(&signing, "evl_other", 2, "2027-01-01"));
        assert!(folder_file_wanted(Some(&held), &other, false), "another licence, new in the folder: yes");
        assert!(!folder_file_wanted(Some(&held), &other, true), "another licence seen before: no");
    }

    #[test]
    fn a_license_left_in_the_data_folder_is_picked_up_at_start() {
        let (server, dir) = a_server();
        std::fs::write(dir.join("Example Steel.evlicense"), "not a license").unwrap();
        // A bad file is logged and left alone; the server still opens.
        begin(&server);
        assert!(server.store.setting(LICENSE).unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod renewal_tests {
    use super::*;

    fn a_license(users: u32, through: &str) -> License {
        License {
            id: "evl_abc123".into(),
            company: "Acme Steel, Inc.".into(),
            edition: hub::license::Edition::Office,
            users,
            updates_through: through.into(),
            issued: "2026-09-01".into(),
            note: String::new(),
            key: "mesafab-2026".into(),
            signature: "00".repeat(64),
        }
    }

    #[test]
    fn three_more_seats_is_a_renewal() {
        assert!(is_a_renewal(&a_license(6, "2027-09-01"), &a_license(9, "2027-09-01")));
    }

    #[test]
    fn another_year_of_updates_is_a_renewal() {
        assert!(is_a_renewal(&a_license(6, "2027-09-01"), &a_license(6, "2028-09-01")));
    }

    /// The case this exists to refuse. A stale file, or a feed somebody took
    /// over, must never be able to take seats off a shop in the middle of a
    /// bid. A genuine reduction is installed by hand, like anything else that
    /// ought to be somebody's decision.
    #[test]
    fn fewer_seats_is_never_installed_by_itself() {
        assert!(!is_a_renewal(&a_license(9, "2027-09-01"), &a_license(6, "2027-09-01")));
    }

    #[test]
    fn an_earlier_end_to_updates_is_never_installed_by_itself() {
        assert!(!is_a_renewal(&a_license(6, "2028-09-01"), &a_license(6, "2027-09-01")));
    }

    #[test]
    fn forever_is_better_than_any_date_and_never_worse() {
        assert!(is_a_renewal(&a_license(6, "2027-09-01"), &a_license(6, "forever")));
        assert!(!is_a_renewal(&a_license(6, "forever"), &a_license(6, "2030-09-01")));
    }

    /// Nought means no limit, so it is the most seats there are.
    #[test]
    fn unlimited_seats_are_more_than_any_number_and_a_number_is_fewer() {
        assert!(is_a_renewal(&a_license(6, "2027-09-01"), &a_license(0, "2027-09-01")));
        assert!(!is_a_renewal(&a_license(0, "2027-09-01"), &a_license(25, "2027-09-01")));
    }

    #[test]
    fn somebody_elses_license_is_not_a_renewal_of_this_one() {
        let mine = a_license(6, "2027-09-01");
        let mut theirs = a_license(25, "2030-01-01");
        theirs.id = "evl_someoneelse".into();
        assert!(!is_a_renewal(&mine, &theirs));

        let mut renamed = a_license(25, "2030-01-01");
        renamed.company = "Acme Steel LLC".into();
        assert!(!is_a_renewal(&mine, &renamed), "a company rename is not automatic");
    }

    #[test]
    fn the_same_license_again_is_not_worth_installing() {
        assert!(!is_a_renewal(&a_license(6, "2027-09-01"), &a_license(6, "2027-09-01")));
    }

    /// The published name gives nothing away about the customer, and the same
    /// id always lands on the same name.
    #[test]
    fn the_name_a_license_is_published_under_says_nothing_about_who_it_is_for() {
        let name = name_for("evl_abc123");
        assert_eq!(name.len(), 64);
        assert_eq!(name, name_for(" evl_abc123 "), "trimmed, so a stray space is not a lost license");
        assert!(!name.contains("evl"));
        assert_ne!(name, name_for("evl_abc124"));
    }

    /// The publishing script works this name out too. If the two ever drift
    /// apart, every server quietly stops finding its own renewal and nobody
    /// gets an error to read — so the answer is pinned here rather than
    /// described in a comment.
    #[test]
    fn the_publishing_script_and_the_server_agree_on_that_name() {
        assert_eq!(
            name_for("evl_abc123"),
            "19431c5846060c7cb52d58f6af8352ed35243c827a01b2703ca4c850160aaebd"
        );
    }

    /// A licence the always-on licensing service signed, byte for byte as it
    /// serves it, with a key made for licences alone. If this stops checking
    /// out, a customer who paid is handed a file their server refuses.
    const CLOUD_SIGNED: &str = r#"{
  "id": "evl_cloud0000001",
  "company": "Ünïcode Fab & Co, LLC",
  "edition": "sealed",
  "users": 7,
  "updates_through": "2027-09-24",
  "issued": "2026-09-24",
  "note": "Added users",
  "key": "excalibur-licences-test",
  "signature": "b07d4a2472bb98d87fe6b098936d58a627ebeed5959e0c66d6d836eae0e94bd3ebacc4c09052fa2127855fd3a9fc74717045508bac6cb38f48a956d74e5c1506"
}
"#;

    #[test]
    fn a_licence_the_licensing_service_signed_checks_out() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let trusted = Trusted { keys: vec![("excalibur-licences-test".into(), key.verifying_key().to_bytes())] };
        let license = License::read(CLOUD_SIGNED, &trusted).unwrap();
        assert_eq!(license.company, "Ünïcode Fab & Co, LLC");
        assert_eq!(license.users, 7);
        assert_eq!(license.edition, Edition::Sealed);
        let tampered = CLOUD_SIGNED.replace("\"users\": 7", "\"users\": 70");
        assert_ne!(tampered, CLOUD_SIGNED);
        assert!(License::read(&tampered, &trusted).is_err());
    }

    #[test]
    fn a_licence_key_signs_licences_and_never_releases() {
        let licensing = crate::license_trust();
        let releases = crate::trusted();
        for (name, _) in crate::LICENSE_KEYS {
            assert!(licensing.key(name).is_some(), "{name} signs licences");
            assert!(releases.key(name).is_none(), "{name} must never be able to sign a release");
        }
        for (name, _) in crate::KEYS {
            assert!(licensing.key(name).is_some(), "licences signed on the PC with {name} still check out");
        }
        assert_eq!(
            licensing.keys.len(),
            crate::KEYS.len() + crate::LICENSE_KEYS.len(),
            "every key in both lists is well formed"
        );
    }
}
