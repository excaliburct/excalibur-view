//! Keeping a company's Hyperview up to date without anybody there doing it.
//!
//! Every half hour, and whenever a seat asks and the last look is ten minutes
//! old, the server asks the publisher's feed what the newest release is. A
//! new viewer is downloaded once, checked against the keys compiled into this
//! server, and offered to every seat in the office, each of which checks it
//! again for itself before it installs. A new server replaces this one and the
//! service manager starts it again a few seconds later.
//!
//! Two things an administrator decides, and nothing else does:
//!
//! - **Early or not.** An office on the early channel gets releases the
//!   publisher is still trying — which is how a new version reaches a shop
//!   before it reaches anybody else.
//! - **Held.** A pin holds every seat on one version while a bid goes out.
//!   The feed is still read; nothing is offered past the pin.
//!
//! What this never does is decide a build is trustworthy on the feed's say-so.
//! The signature is checked here, and again on every seat.

use std::sync::Arc;
use std::time::Duration;

use hub::feed::{Feed, Found, SERVER_PLATFORM};
use hub::update::{newer, Channel, Release};
use rusqlite::params;

use crate::api::{now, Server};
use crate::VERSION;

/// How often the feed is asked with nobody asking. A fix published at lunch
/// is in the office before the afternoon is out, and two looks an hour is
/// nothing to GitHub, which allows sixty from any one address.
pub const EVERY: Duration = Duration::from_secs(30 * 60);

/// A seat asking a server whose last look is older than this sets off another
/// one, so the office hears about a release soon after somebody opens
/// Hyperview rather than at the server's next turn.
pub const STALE: Duration = Duration::from_secs(10 * 60);

/// Two looks closer together than this are one look. Six people pressing
/// "Check for Updates" at once are answered by the first.
pub const FRESH: Duration = Duration::from_secs(60);

/// How long somebody who pressed "Check for Updates" is kept waiting while
/// the server fetches a new version, before they are told it is still coming.
pub const PATIENCE: Duration = Duration::from_secs(75);

/// When the feed was last looked at, and the one look allowed at a time.
#[derive(Default)]
pub struct Watch {
    one_at_a_time: tokio::sync::Mutex<()>,
    last: std::sync::Mutex<Option<std::time::Instant>>,
    under_way: std::sync::atomic::AtomicBool,
}

impl Watch {
    /// How long ago the last look finished. `None` before the first.
    pub fn since_last(&self) -> Option<Duration> {
        self.last.lock().ok().and_then(|last| last.map(|t| t.elapsed()))
    }

    /// Whether a look is going on right now.
    pub fn is_looking(&self) -> bool {
        self.under_way.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn looked(&self) {
        if let Ok(mut last) = self.last.lock() {
            *last = Some(std::time::Instant::now());
        }
    }
}

/// The feed this server reads, or `None` when it has been told not to look.
pub fn feed_url(server: &Server) -> Option<String> {
    // A sealed office is updated by hand, from a file its administrator
    // installs. The server never reaches out, and there is no setting that
    // makes it.
    if hub::sealed::is_sealed() {
        return None;
    }
    if !server.config.check_for_updates {
        return None;
    }
    let feed = server
        .config
        .release_feed
        .clone()
        .unwrap_or_else(|| crate::HOME_FEED.to_string());
    (!feed.trim().is_empty()).then_some(feed)
}

/// What a server that has put a newer copy of itself in place exits with, so
/// the service manager's "restart on failure" brings the new one up.
pub const RESTART_TO_UPDATE: i32 = 3;

pub mod settings {
    /// `stable` or `preview`. Set by an administrator.
    pub const CHANNEL: &str = "update_channel";
    /// When the feed was last asked.
    pub const CHECKED: &str = "update_checked";
    /// What happened when it was, in words.
    pub const NOTE: &str = "update_note";
}

/// Which releases this office takes.
pub fn office_channel(server: &Server) -> Channel {
    let chosen = server
        .store
        .setting(settings::CHANNEL)
        .ok()
        .flatten()
        .unwrap_or_else(|| server.config.channel.clone());
    match chosen.trim() {
        "preview" | "early" => Channel::Preview,
        _ => Channel::Stable,
    }
}

/// What one look at the feed came to.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// Nothing newer than what the office already has.
    UpToDate,
    /// A new viewer is now on offer to the seats.
    Offered(String),
    /// A new server is in place and takes over when this one restarts.
    ServerReplaced(String),
    /// A newer server came out after this office's updates ended. The office
    /// keeps the version it has, which keeps working.
    NotCovered(String),
    /// Something went wrong, said in words. Nothing was changed.
    Trouble(String),
}

/// Asks the feed every [`EVERY`], for as long as the server runs.
pub async fn keep_up_to_date(server: Arc<Server>) {
    // An office that has told this server not to look for new versions may
    // still have bought three more seats this morning. Two reasons to make
    // the trip, and either one is enough.
    if feed_url(&server).is_none() && crate::license::renewal_url(&server).is_none() {
        return;
    }
    // Not the moment it starts: a server that has just been installed is busy
    // being found by the office, and a few seconds make no difference.
    tokio::time::sleep(Duration::from_secs(20)).await;
    loop {
        if feed_url(&server).is_some() {
            look(Arc::clone(&server)).await;
        }
        // On the same trip out, because a shop that has just bought three more
        // seats should have them before somebody notices they are missing --
        // and because two schedules for the same half hour is one more thing
        // to keep in step than there needs to be.
        if let Some(what) = crate::license::look_for_renewal(&server) {
            tracing::info!("the license renewed itself: {what}");
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// What a seat asking for an update sets off. `fresh` is somebody pressing
/// "Check for Updates"; the look is waited for, up to [`PATIENCE`]. Otherwise
/// a stale look is started and not waited for. True when a look is still
/// going on as the seat is answered.
pub async fn on_asking(server: &Arc<Server>, fresh: bool) -> bool {
    if feed_url(server).is_none() {
        return false;
    }
    let stale = server.updates.since_last().is_none_or(|ago| ago > STALE);
    if fresh {
        // Its own task, so a seat that gives up waiting does not cut a
        // download off half way.
        let looking = tokio::spawn(look(Arc::clone(server)));
        let _ = tokio::time::timeout(PATIENCE, looking).await;
    } else if stale && !server.updates.is_looking() {
        tokio::spawn(look(Arc::clone(server)));
    }
    server.updates.is_looking()
}

/// One look at the feed, unless one finished within [`FRESH`] — then that one
/// stands. A look already going on is waited for rather than doubled.
pub async fn look(server: Arc<Server>) -> Option<Outcome> {
    let url = feed_url(&server)?;
    let _one = server.updates.one_at_a_time.lock().await;
    if server.updates.since_last().is_some_and(|ago| ago < FRESH) {
        return None;
    }
    server
        .updates
        .under_way
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let asking = Arc::clone(&server);
    let outcome = tokio::task::spawn_blocking(move || check_once(&asking, &Feed::new(&url)))
        .await
        .unwrap_or_else(|e| Outcome::Trouble(format!("the update check stopped: {e}")));
    server.updates.looked();
    server
        .updates
        .under_way
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let note = describe(&outcome);
    tracing::info!("update check: {note}");
    let _ = server.store.set_setting(settings::CHECKED, &now());
    let _ = server.store.set_setting(settings::NOTE, &note);
    if let Outcome::ServerReplaced(_) = outcome {
        // The service manager restarts it in a few seconds, as the new
        // version. A moment first, so a seat that is waiting on this look is
        // answered rather than cut off; seats simply ask the new one again.
        tracing::info!("restarting as the new version");
        tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            std::process::exit(RESTART_TO_UPDATE);
        });
    }
    Some(outcome)
}

fn describe(outcome: &Outcome) -> String {
    match outcome {
        Outcome::UpToDate => "up to date".into(),
        Outcome::Offered(v) => format!("version {v} is on offer to the seats"),
        Outcome::ServerReplaced(v) => format!("the server is updating itself to {v}"),
        Outcome::NotCovered(v) => format!(
            "server version {v} came out after this office's updates ended; it stays on {VERSION}, which keeps working"
        ),
        Outcome::Trouble(why) => why.clone(),
    }
}

/// One look at the feed, trusting the keys compiled into this server.
/// Blocking; run it off the async threads.
pub fn check_once(server: &Server, feed: &Feed) -> Outcome {
    check_with(server, feed, &crate::trusted())
}

/// One look at the feed, trusting exactly `trusted`.
pub fn check_with(server: &Server, feed: &Feed, trusted: &hub::update::Trusted) -> Outcome {
    let found = match feed.newest(office_channel(server)) {
        Ok(Some(found)) => found,
        Ok(None) => return Outcome::UpToDate,
        Err(e) => return Outcome::Trouble(e),
    };
    if trusted.is_empty() {
        return Outcome::Trouble(
            "this server trusts no signing key, so it will not take any release".into(),
        );
    }

    // The server first: if it is replacing itself, the new one will offer the
    // viewer when it starts, and a restart in the middle of storing a viewer
    // would only mean doing it twice.
    let mut not_covered = None;
    if server.config.replace_self {
        if let Some(release) = found.published.server_for(SERVER_PLATFORM) {
            if newer(&release.version, VERSION) {
                // The server is what Office buys. The desktop app below is free
                // and is offered whatever the license says.
                if crate::license::covers(server, &release.published) {
                    return match replace_self(release, &found, feed, trusted) {
                        Ok(()) => Outcome::ServerReplaced(release.version.clone()),
                        Err(why) => Outcome::Trouble(why),
                    };
                }
                not_covered = Some(release.version.clone());
            }
        }
    }

    // Every platform in the release, not this server's own. A Windows server
    // holds the Mac build for the Mac seats in the same office; a seat that
    // turns up next month finds its version already here rather than waiting
    // on the next look at the feed.
    let mut offered: Option<String> = None;
    let mut trouble: Option<String> = None;
    for app in found.published.viewers() {
        if already_have(server, app) || !newer(&app.version, &newest_held(server, &app.platform)) {
            continue;
        }
        match take(server, app, &found, feed, trusted) {
            Ok(()) => {
                tracing::info!("holding {} for {}", app.version, app.platform);
                offered = Some(app.version.clone());
            }
            // One platform failing does not stop another. A Mac build that
            // will not download is a log line, not a Windows office stuck on
            // an old version.
            Err(why) => {
                tracing::warn!("{} for {}: {why}", app.version, app.platform);
                trouble.get_or_insert(why);
            }
        }
    }
    match (offered, trouble, not_covered) {
        (Some(version), _, _) => Outcome::Offered(version),
        (None, Some(why), _) => Outcome::Trouble(why),
        (None, None, Some(version)) => Outcome::NotCovered(version),
        (None, None, None) => Outcome::UpToDate,
    }
}

fn download(
    release: &Release,
    found: &Found,
    feed: &Feed,
    trusted: &hub::update::Trusted,
) -> Result<Vec<u8>, String> {
    let url = found
        .url(&release.download)
        .ok_or_else(|| format!("the release has no file called {}", release.download))?;
    let bytes = feed.fetch(url)?;
    trusted
        .check(release, &bytes)
        .map_err(|refusal| format!("version {}: {refusal}", release.version))?;
    Ok(bytes)
}

/// Stores a checked viewer and offers it to the seats.
fn take(
    server: &Server,
    release: &Release,
    found: &Found,
    feed: &Feed,
    trusted: &hub::update::Trusted,
) -> Result<(), String> {
    let bytes = download(release, found, feed, trusted)?;
    server
        .store
        .put_blob(&bytes)
        .map_err(|e| format!("could not store version {}: {e}", release.version))?;
    server
        .store
        .with(|db| {
            db.execute(
                "INSERT INTO releases (version, channel, platform, published, notes, bytes,
                                       digest, signature, signing_key, minimum_api, ready)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1)
                 ON CONFLICT(version, platform) DO UPDATE SET
                     channel = excluded.channel, published = excluded.published,
                     notes = excluded.notes, bytes = excluded.bytes,
                     digest = excluded.digest, signature = excluded.signature,
                     signing_key = excluded.signing_key, minimum_api = excluded.minimum_api,
                     ready = 1",
                params![
                    release.version,
                    crate::api::channel_name(release.channel),
                    release.platform,
                    release.published,
                    release.notes,
                    crate::store::int::signed(release.bytes), // WHAT: written as SQLite keeps it, a signed 64-bit number. WHY: read back the same by int::unsigned or int::index
                    release.digest.to_lowercase(),
                    release.signature,
                    release.key,
                    i64::from(release.minimum_api_version) // WHAT: widened, never cut. WHY: every one of these fits a 64-bit number
                ],
            )?;
            Ok(())
        })
        .map_err(|e| format!("could not record version {}: {e}", release.version))
}

fn replace_self(
    release: &Release,
    found: &Found,
    feed: &Feed,
    trusted: &hub::update::Trusted,
) -> Result<(), String> {
    let bytes = download(release, found, feed, trusted)?;
    let program = std::env::current_exe().map_err(|e| e.to_string())?;
    hub::update::swap_in(&program, &bytes)
        .map_err(|e| format!("could not put version {} in place: {e}", release.version))
}

fn already_have(server: &Server, release: &Release) -> bool {
    server
        .store
        .with(|db| {
            Ok(db.query_row(
                "SELECT count(*) FROM releases WHERE version = ?1 AND platform = ?2",
                params![release.version, release.platform],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .is_ok_and(|n| n > 0) // WHAT: whether any are held, false when it can't be read. WHY: as before
}

/// The newest viewer this server already holds for one platform, or "0" when
/// it holds none. Per platform, because a Mac starting from nothing must not
/// be told it is up to date by the Windows build sitting beside it.
fn newest_held(server: &Server, platform: &str) -> String {
    let versions: Vec<String> = server
        .store
        .with(|db| {
            let mut statement =
                db.prepare("SELECT version FROM releases WHERE platform = ?1")?;
            let all = statement
                .query_map(params![platform], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(all)
        })
        .unwrap_or_default();
    versions
        .into_iter()
        .max_by(|a, b| hub::update::compare(a, b))
        .unwrap_or_else(|| "0".into())
}
