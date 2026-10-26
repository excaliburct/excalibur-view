//! The channel the Excalibur Fleet talks to a shop's server on.
//!
//! Deliberately not part of `/api/v1`. That is the product's API — documented,
//! versioned, and the thing a customer's ERP integrates against. This is
//! something else: the small maintenance surface one shop's server offers to
//! whoever looks after it, shaped so the Fleet can watch and feed a Hyperview
//! installation exactly as it watches and feeds a Citadel one, with no
//! per-product code in the Fleet at all.
//!
//! Four things, on the paths the Fleet already asks for:
//!
//! - `GET  /api/version` — public. What this server is running.
//! - `GET  /api/admin/ops` — key-gated. The 6am questions: when were the
//!   drawings last copied off this box, how much disk is left, is the update
//!   channel still refusing unsigned builds.
//! - `GET  /api/admin/logtail` — key-gated. The server's own log, clamped.
//! - `GET  /api/admin/presence` — key-gated. Who is connected: the seats this
//!   process has served lately, bounded and read-only (`crate::presence`), in
//!   the same wire shape Citadel answers with, so the Fleet's Devices view
//!   works here without a line of per-product code.
//! - `POST /api/admin/update` — key-gated. A signed release, delivered
//!   verbatim. Up to [`LARGEST_PUSH`], read only once the key is right.
//! - `POST /api/admin/update/rollback` — key-gated. Put the office back.
//!
//! **The Fleet cannot mint a build.** What arrives at `/api/admin/update` is
//! the exact signed manifest the release tool produced, and this server checks
//! that signature against the same pinned public keys a seat checks it
//! against — the Fleet's key opens the door, it does not vouch for what comes
//! through it. So a Fleet with a compromised shelf can offer only builds that
//! were already signed by the release key, and every seat in the shop
//! checks the same signature again for itself before running anything. Two
//! independent checks of one signature, neither of them the hub's.
//!
//! The maintenance key is set by whoever runs the server, and until they set
//! one **every route here answers 404**: a shop that has not asked to be looked
//! after does not quietly grow a remote maintenance channel.

use std::sync::Arc;

use axum::body::{Body, Bytes, HttpBody}; // WHAT: the raw request body, its chunks, and the trait that reads them. WHY: a push is read by hand, after the key is checked and no further than its limit.
use axum::extract::{Query, State};
use axum::http::header::{CONTENT_LENGTH, EXPECT}; // WHAT: the two headers that say how big a body is and whether the caller waits before sending it. WHY: an oversized push is refused from those before any of it is read.
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use hub::update::Release;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::api::{Server, Shared};
use crate::VERSION;

/// The header the Fleet signs its requests with. The name is the one the
/// Fleet already sends, and inventing a second name for the same header
/// would mean per-product code in the Fleet.
pub const KEY_HEADER: &str = "x-av-update-key";

/// Where the digest of a Fleet key an administrator turned on is kept.
pub const FLEET_KEY: &str = "fleet_key_digest";

pub fn key_digest(key: &str) -> String {
    use sha2::{Digest, Sha256};
    crate::store::hex(&Sha256::digest(key.trim().as_bytes()))
}

/// Lets the Fleet in with a new key, or shuts the door. Returns the key, once:
/// it is not kept anywhere it can be read back.
pub fn set_fleet_access(server: &Server, on: bool) -> anyhow::Result<Option<String>> {
    if !on {
        server.store.set_setting(FLEET_KEY, "")?;
        return Ok(None);
    }
    let mut bytes = [0u8; 24];
    rand::Rng::fill(&mut rand::thread_rng(), &mut bytes);
    let key = crate::store::hex(&bytes);
    server.store.set_setting(FLEET_KEY, &key_digest(&key))?;
    Ok(Some(key))
}

pub fn fleet_is_on(server: &Server) -> bool {
    server.config.maintenance_key.is_some()
        || server
            .store
            .setting(FLEET_KEY)
            .ok()
            .flatten()
            .is_some_and(|d| !d.is_empty())
}

// ---- what it says about itself --------------------------------------------

#[derive(Serialize)]
struct Version {
    /// What is *running*, always. Not what is staged, not what is on offer to
    /// the seats — the Fleet asserts a rollout worked by reading this back, so
    /// a hopeful answer here would turn a failed update into a green board.
    version: String,
    /// Which release channel this server follows.
    wave: String,
    /// The build. Same string the About box shows.
    hash: String,
    product: String,
}

#[derive(Serialize, Default)]
struct Ops {
    /// How many days since the drawings were last copied somewhere that is not
    /// this machine. **Null when it has never happened** — and null is not
    /// zero. A shop holding its own drawings with no copy anywhere else is one
    /// dead disk away from losing every takeoff it has ever done, and the Fleet
    /// turns this amber on purpose.
    #[serde(rename = "backupDays")]
    backup_days: Option<f64>,
    disk: Disk,
    #[serde(rename = "updateChannel")]
    update_channel: UpdateChannel,
    #[serde(rename = "serviceLogBytes")]
    service_log_bytes: u64,
    /// Nothing failed to load. Present and empty rather than absent, so the
    /// difference between "asked and nothing is wrong" and "did not ask" is
    /// visible.
    #[serde(rename = "routeFailures")]
    route_failures: Vec<String>,
    /// What this installation is actually holding, which is the thing somebody
    /// is deciding about when they look at the board.
    drawings: Drawings,
}

#[derive(Serialize, Default)]
struct Disk {
    #[serde(rename = "freeGb")]
    free_gb: Option<f64>,
}

#[derive(Serialize, Default)]
struct UpdateChannel {
    /// Always true, and not a setting. There is no way to run this server in a
    /// state where it would accept an unsigned release, which is why the answer
    /// is a constant rather than a flag somebody could have turned off.
    #[serde(rename = "signingRequired")]
    signing_required: bool,
    /// How many public keys this build will accept a release from. Zero means
    /// it will accept none — which is the state a build ships in until a key is
    /// compiled into it, and is worth being able to see from the board.
    #[serde(rename = "trustedKeys")]
    trusted_keys: usize,
}

#[derive(Serialize, Default)]
struct Drawings {
    people: i64,
    projects: i64,
    sets: i64,
    markups: i64,
    #[serde(rename = "bytesOnDisk")]
    bytes_on_disk: u64,
}

// ---- saying no ------------------------------------------------------------

struct No(StatusCode, &'static str);

impl IntoResponse for No {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

/// Is this the person who looks after this box?
///
/// A server with no maintenance key set is not "unlocked" — it has no such
/// channel at all, and says 404 rather than 401, because 401 is an invitation
/// to keep trying.
fn maintainer(server: &Server, headers: &HeaderMap) -> Result<(), No> {
    let given = headers
        .get(KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    // A key an administrator turned on from the program is kept only as its
    // digest, so reading the database does not hand anybody the key.
    let stored = server.store.setting(FLEET_KEY).ok().flatten();
    let (expected, given) = match (server.config.maintenance_key.as_deref(), stored) {
        (Some(key), _) => (key.to_string(), given.to_string()),
        (None, Some(digest)) if !digest.is_empty() => (digest, key_digest(given)),
        _ => return Err(No(StatusCode::NOT_FOUND, "no such route")),
    };
    let (expected, given) = (expected.as_str(), given.as_str());
    // Compared to the end whichever way it goes, so how long this takes says
    // nothing about how much of the key was right.
    if given.len() != expected.len() || expected.is_empty() {
        return Err(No(StatusCode::UNAUTHORIZED, "wrong maintenance key"));
    }
    let mut same = 0u8;
    for (a, b) in given.bytes().zip(expected.bytes()) {
        same |= a ^ b;
    }
    if same != 0 {
        return Err(No(StatusCode::UNAUTHORIZED, "wrong maintenance key"));
    }
    Ok(())
}

// ---- the routes -----------------------------------------------------------

pub fn router(server: Shared) -> Router {
    Router::new()
        .route("/api/version", get(version))
        .route("/api/admin/ops", get(ops))
        .route("/api/admin/logtail", get(logtail))
        .route("/api/admin/presence", get(presence))
        .route("/api/admin/update", post(update))
        .route("/api/admin/update/rollback", post(rollback))
        .with_state(server)
}

async fn version(State(server): State<Shared>) -> Json<Version> {
    Json(Version {
        version: VERSION.to_string(),
        wave: match crate::updates::office_channel(&server) {
            hub::update::Channel::Preview => "early".into(),
            hub::update::Channel::Stable => "stable".into(),
        },
        hash: option_env!("HYPERVIEW_BUILD").unwrap_or(VERSION).to_string(),
        product: "hyperview".to_string(),
    })
}

async fn ops(State(server): State<Shared>, headers: HeaderMap) -> Result<Json<Ops>, No> {
    maintainer(&server, &headers)?;

    let counts = server
        .store
        .with(|db| {
            let one = |db: &rusqlite::Connection, sql: &str| -> rusqlite::Result<i64> {
                db.query_row(sql, [], |r| r.get(0))
            };
            Ok(Drawings {
                people: one(db, "SELECT count(*) FROM people")?,
                projects: one(db, "SELECT count(*) FROM projects")?,
                sets: one(db, "SELECT count(*) FROM sets")?,
                markups: one(db, "SELECT count(*) FROM markups WHERE removed = 0")?,
                bytes_on_disk: 0,
            })
        })
        .unwrap_or_default();

    let mut drawings = counts;
    drawings.bytes_on_disk = folder_size(&server.config.data);

    Ok(Json(Ops {
        backup_days: days_since_backup(&server),
        disk: Disk {
            free_gb: free_gigabytes(&server.config.data),
        },
        update_channel: UpdateChannel {
            signing_required: true,
            trusted_keys: crate::trusted().keys.len(),
        },
        service_log_bytes: log_file(&server.config.data)
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(0),
        route_failures: Vec::new(),
        drawings,
    }))
}

#[derive(Deserialize)]
pub struct HowMany {
    #[serde(default)]
    pub n: Option<usize>,
}

#[derive(Serialize)]
struct Tail {
    ok: bool,
    bytes: u64,
    lines: Vec<String>,
}

async fn logtail(
    State(server): State<Shared>,
    headers: HeaderMap,
    Query(how_many): Query<HowMany>,
) -> Result<Json<Tail>, No> {
    maintainer(&server, &headers)?;
    let n = how_many.n.unwrap_or(200).clamp(1, 500);
    let Some(path) = log_file(&server.config.data) else {
        return Ok(Json(Tail {
            ok: true,
            bytes: 0,
            lines: vec!["This server is not writing to a log file.".into()],
        }));
    };
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    // Only the tail is read. A crash loop can make this file enormous, and
    // reading all of it to show the last two hundred lines is how asking a
    // sick machine a question makes it sicker.
    let lines = last_lines(&path, n);
    Ok(Json(Tail { ok: true, bytes, lines }))
}

/// Who is connected. The ledger is the same class of answer as the log tail:
/// a look at this box for whoever maintains it, gated by the same key, and
/// incapable of changing anything.
async fn presence(
    State(server): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<crate::presence::Roster>, No> {
    maintainer(&server, &headers)?;
    Ok(Json(crate::presence::roster(&server.config.name)))
}

// ---- taking a release -----------------------------------------------------

/// The largest push `/api/admin/update` takes, as it arrives: the JSON, with
/// the installer in it as base64 (a third bigger than the installer).
///
/// Sized from what is really sent. One push carries one platform's
/// program, and the biggest today is the Linux one at about 60 MB, which
/// arrives as about 80 MB; the Windows push is about 62 MB. 192 MiB is more
/// than twice that, so a program can grow to about 140 MB before this needs
/// changing, and it stays well short of what an office server should hold in
/// memory for one request. Only this route has it: everywhere else on this
/// channel keeps the 2 MB that every small request gets.
pub const LARGEST_PUSH: usize = 192 * 1024 * 1024; // WHAT: 192 MiB. WHY: twice and more the biggest real push (about 80 MB), where axum's default 2 MB refused every one of them.

/// How much of a push that is too big is read and thrown away, so the caller
/// is still listening when it is told so.
///
/// A server that answers and hangs up while the caller is still sending makes
/// the caller's next write fail, and all it then reports is "broken pipe",
/// never the answer. Reading the rest (into nothing, so it costs no memory)
/// lets the refusal arrive as a plain 413. Only a caller that has already
/// given the right key gets this, and only up to here.
pub const READ_PAST_AT_MOST: u64 = 1024 * 1024 * 1024; // WHAT: 1 GiB thrown away at most. WHY: covers any honest mistake (several programs in one push) with a readable answer, and no more.

/// What a push too big for [`LARGEST_PUSH`] is told.
const TOO_BIG: &str = "that release is larger than this server takes in one push (192 MiB)"; // WHAT: the refusal, with the limit in it. WHY: whoever sent it can see at once what is wrong and by how much it has to shrink.

#[derive(Deserialize)]
pub struct Push {
    /// The signed manifest, exactly as the release tool produced it.
    pub release: Release,
    /// The installer itself, base64. Carried in the bundle rather than fetched,
    /// so what was signed and what arrives are the same journey.
    pub installer: String,
    /// True also makes it the version the office is offered. False stages it
    /// for one machine to try first, which is what a careful shop does.
    #[serde(default)]
    pub ready: bool,
}

#[derive(Serialize)]
struct Landed {
    /// What the Fleet counts. One entry per thing written.
    written: Vec<String>,
    /// Anything refused, with why. The Fleet treats a non-empty list as a
    /// partial failure and does not call it a rollout.
    skipped: Vec<String>,
    restarting: bool,
    message: String,
}

/// Takes a pushed release.
///
/// The key is checked before a byte of the body is read. The body is read
/// here rather than by axum's `Json`, because an extractor reads the body
/// before the handler runs, and with a limit big enough for a real push that
/// would let anybody who can reach the port make the server take in 192 MiB
/// before being told no.
async fn update(
    State(server): State<Shared>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<Landed>, No> {
    maintainer(&server, &headers)?; // WHAT: the maintenance key, first. WHY: nothing of the body has been read yet, so a caller without the key costs the server nothing.
    let bytes = read_push(&headers, body).await?; // WHAT: the push, read up to its limit. WHY: a real push is tens of megabytes, and one too big is refused with a plain 413.
    let Ok(push) = serde_json::from_slice::<Push>(&bytes) else {
        return Err(No(StatusCode::BAD_REQUEST, "that push is not a release")); // WHAT: a body that is not a push, refused. WHY: the same refusal Json gave before, said in the channel's own words.
    };
    drop(bytes); // WHAT: lets go of the raw body. WHY: the installer is now in `push`, and holding both would double what the push costs in memory.

    use base64::Engine;
    let Ok(installer) = base64::engine::general_purpose::STANDARD.decode(push.installer.as_bytes())
    else {
        return Ok(Json(refused("the installer was not readable")));
    };

    // The signature, checked here against the keys this build pins — not
    // against anything the caller supplied, and not taken on trust because the
    // maintenance key was right. The key said who may knock. This says what may
    // come in.
    if let Err(refusal) = crate::trusted().check(&push.release, &installer) {
        tracing::warn!("refused a pushed release: {refusal}");
        return Ok(Json(refused_owned(format!(
            "the signature was not accepted: {refusal}"
        ))));
    }

    let digest = push.release.digest.to_lowercase();
    if let Err(e) = server.store.put_blob(&installer) {
        return Ok(Json(refused_owned(format!("could not store it: {e}"))));
    }
    let stored = server.store.with(|db| {
        db.execute(
            "INSERT INTO releases (version, channel, platform, published, notes, bytes,
                                   digest, signature, signing_key, minimum_api, ready)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(version, platform) DO UPDATE SET
                 channel = excluded.channel, published = excluded.published,
                 notes = excluded.notes, bytes = excluded.bytes,
                 digest = excluded.digest, signature = excluded.signature,
                 signing_key = excluded.signing_key, minimum_api = excluded.minimum_api,
                 ready = excluded.ready",
            params![
                push.release.version,
                crate::api::channel_name(push.release.channel),
                push.release.platform,
                push.release.published,
                push.release.notes,
                push.release.bytes as i64,
                digest,
                push.release.signature,
                push.release.key,
                push.release.minimum_api_version as i64,
                if push.ready { 1 } else { 0 }
            ],
        )?;
        Ok(())
    });
    if let Err(e) = stored {
        return Ok(Json(refused_owned(format!("could not record it: {e}"))));
    }

    let version = push.release.version.clone();
    tracing::info!("took release {version} for {}", push.release.platform);
    Ok(Json(Landed {
        written: vec![format!("{version} ({})", push.release.platform)],
        skipped: Vec::new(),
        // This server does not restart itself. It is holding a release for the
        // seats, and a seat installs it the next time somebody starts
        // Hyperview — never in the middle of a takeoff. Saying `true` here
        // would make the Fleet wait ninety seconds for a restart that is not
        // coming and then call a good rollout a failure.
        restarting: false,
        message: if push.ready {
            format!("{version} is on the server and offered to every seat.")
        } else {
            format!("{version} is on the server, held back until somebody marks it ready.")
        },
    }))
}

/// Reads a push's body, no further than [`LARGEST_PUSH`].
///
/// A push too big is refused with 413, from its Content-Length when it says
/// one and as soon as it passes the limit when it does not. Before that
/// answer, the rest of it is read into nothing (see [`READ_PAST_AT_MOST`]), so
/// the caller hears the 413 instead of a broken connection. A caller that
/// asked to wait before sending ("Expect: 100-continue") is answered at once
/// and never sends it.
async fn read_push(headers: &HeaderMap, mut body: Body) -> Result<Vec<u8>, No> {
    let declared = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok()); // WHAT: the size the caller says it is sending. WHY: one too big can be refused before any of it is read.
    let waits = headers
        .get(EXPECT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("100-continue")); // WHAT: whether the caller waits to be told to go on. WHY: then a refusal now means it never sends the body at all.
    if let Some(size) = declared.filter(|&n| n > LARGEST_PUSH as u64) {
        let budget = if waits { 0 } else { size.min(READ_PAST_AT_MOST) }; // WHAT: nothing for a caller that waits, else what it said it is sending, but never more than the cap. WHY: a waiting caller has not sent it and must not be asked to; anyone else is read past only so far.
        read_past(&mut body, budget).await; // WHAT: reads that much into nothing. WHY: so a caller still sending is listening when the 413 comes.
        return Err(No(StatusCode::PAYLOAD_TOO_LARGE, TOO_BIG)); // WHAT: refused for its size. WHY: a clear answer that says what is wrong.
    }
    let mut kept = Vec::with_capacity(declared.unwrap_or(0) as usize); // WHAT: room for what it says it is sending. WHY: under the limit, by the check above, so one allocation instead of many.
    while let Some(chunk) = next_chunk(&mut body).await {
        let Ok(chunk) = chunk else {
            return Err(No(StatusCode::BAD_REQUEST, "the push did not arrive whole")); // WHAT: a body that broke off. WHY: half an installer is never taken.
        };
        if kept.len() + chunk.len() > LARGEST_PUSH {
            let so_far = (kept.len() + chunk.len()) as u64; // WHAT: how much has come so far. WHY: counts against what is read past.
            drop(kept); // WHAT: lets go of what was kept. WHY: it is not going to be used, and need not be held while the rest is read past.
            read_past(&mut body, READ_PAST_AT_MOST.saturating_sub(so_far)).await; // WHAT: the rest, into nothing. WHY: so the 413 is heard.
            return Err(No(StatusCode::PAYLOAD_TOO_LARGE, TOO_BIG)); // WHAT: refused for its size. WHY: it said no size, and turned out bigger than the limit.
        }
        kept.extend_from_slice(&chunk); // WHAT: keeps the chunk. WHY: under the limit, so it is part of the push.
    }
    Ok(kept) // WHAT: the whole push. WHY: it came in under the limit.
}

/// The next chunk of a body's data, or `None` at its end. Trailers are not
/// data and are passed over.
async fn next_chunk(body: &mut Body) -> Option<Result<Bytes, axum::Error>> {
    loop {
        let frame = std::future::poll_fn(|cx| std::pin::Pin::new(&mut *body).poll_frame(cx)).await?; // WHAT: the next frame, or None at the end. WHY: read a frame at a time, so the limit is checked as it comes.
        match frame {
            Err(e) => return Some(Err(e)), // WHAT: the body broke off. WHY: said to the caller, who refuses the push.
            Ok(frame) => {
                if let Ok(data) = frame.into_data() {
                    return Some(Ok(data)); // WHAT: a chunk of data. WHY: that is what a push is made of.
                } // WHAT: anything else (trailers) is passed over. WHY: a push has nothing in them.
            }
        }
    }
}

/// Reads up to `at_most` more of a body and throws it away.
async fn read_past(body: &mut Body, at_most: u64) {
    let mut read = 0u64; // WHAT: how much has been thrown away. WHY: it stops at `at_most`.
    while read < at_most {
        match next_chunk(body).await {
            Some(Ok(chunk)) => read += chunk.len() as u64, // WHAT: counted and dropped. WHY: costs no memory beyond one chunk.
            _ => break, // WHAT: the end of the body, or it broke off. WHY: nothing more to read either way.
        }
    }
}

/// What a rollback may be narrowed to. The Fleet sends no body at all, which
/// means "whatever is newest, on every platform" — so this is read leniently
/// and an unreadable body is treated as an absent one rather than an error.
#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct Withdraw {
    /// Just the Mac build, say, when the Windows one is fine.
    platform: String,
}

async fn rollback(
    State(server): State<Shared>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<Landed>, No> {
    maintainer(&server, &headers)?;
    let only: Option<String> = serde_json::from_str::<Withdraw>(&body)
        .ok()
        .map(|w| w.platform.trim().to_string())
        .filter(|p| !p.is_empty());

    // Withdrawing the newest, not deleting it: the office goes back to what it
    // was on, and the release is still there to look at.
    let undone = server.store.with(|db| {
        // Newest by version, the way the seats read it — not by the day it was
        // published. Those two disagree the moment a fix for an old version
        // goes out after a new one, and then the wrong release is withdrawn
        // while the one actually on offer stays on offer.
        let mut statement = db.prepare("SELECT DISTINCT version FROM releases WHERE ready = 1")?;
        let newest = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .max_by(|a, b| hub::update::compare(a, b));

        let mut platforms = Vec::new();
        if let Some(version) = newest.as_deref() {
            // Every platform of that version unless one was named, because a
            // release is one thing that happened to be built twice, and an
            // office left offering half of it is an office where the Macs and
            // the Windows machines are on different versions of the program.
            match only.as_deref() {
                Some(platform) => {
                    db.execute(
                        "UPDATE releases SET ready = 0 WHERE version = ?1 AND platform = ?2",
                        params![version, platform],
                    )?;
                    platforms.push(platform.to_string());
                }
                None => {
                    let mut listing = db
                        .prepare("SELECT platform FROM releases WHERE version = ?1 AND ready = 1")?;
                    platforms = listing
                        .query_map(params![version], |r| r.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    db.execute(
                        "UPDATE releases SET ready = 0 WHERE version = ?1",
                        params![version],
                    )?;
                }
            }
        }
        Ok(newest.filter(|_| !platforms.is_empty()).map(|v| (v, platforms)))
    });

    match undone {
        Ok(Some((version, platforms))) => Ok(Json(Landed {
            written: platforms.iter().map(|p| format!("withdrew {version} ({p})")).collect(),
            skipped: Vec::new(),
            restarting: false,
            message: format!(
                "{version} is no longer offered for {}. Seats already on it stay on it — nothing \
                 uninstalls itself — and every other seat is offered what it was offered before.",
                platforms.join(", ")
            ),
        })),
        Ok(None) => Ok(Json(Landed {
            written: Vec::new(),
            skipped: Vec::new(),
            restarting: false,
            message: "Nothing was on offer, so there was nothing to withdraw.".into(),
        })),
        Err(e) => Ok(Json(refused_owned(format!("could not withdraw it: {e}")))),
    }
}

fn refused(why: &'static str) -> Landed {
    refused_owned(why.to_string())
}

fn refused_owned(why: String) -> Landed {
    Landed {
        written: Vec::new(),
        skipped: vec![why.clone()],
        restarting: false,
        message: why,
    }
}

// ---- the 6am questions ----------------------------------------------------

/// When the drawings were last copied off this machine, in days.
///
/// `None` means never, and never is not the same as today. Whoever set the
/// server up records a backup by touching a file; a shop with a real backup
/// system points that at wherever it already runs.
fn days_since_backup(server: &Server) -> Option<f64> {
    let marker = server.config.data.join("last-backup");
    let when = std::fs::metadata(&marker).ok()?.modified().ok()?;
    let ago = std::time::SystemTime::now().duration_since(when).ok()?;
    Some(ago.as_secs_f64() / 86_400.0)
}

fn log_file(data: &std::path::Path) -> Option<std::path::PathBuf> {
    let path = data.join("hyperview-server.log");
    path.exists().then_some(path)
}

fn folder_size(path: &std::path::Path) -> u64 {
    fn walk(path: &std::path::Path, depth: usize) -> u64 {
        if depth > 6 {
            return 0;
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };
        entries
            .flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path(), depth + 1),
                Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
                Err(_) => 0,
            })
            .sum()
    }
    walk(path, 0)
}

/// Free space on the drive the drawings are on. Asked of the operating system
/// rather than worked out, and `None` rather than a guess when it cannot be.
fn free_gigabytes(data: &std::path::Path) -> Option<f64> {
    #[cfg(unix)]
    {
        // `df` rather than a binding to statvfs, because the struct's layout
        // differs between platforms and getting it wrong reads garbage rather
        // than failing. This is a number on a dashboard, once a minute; asking
        // the tool that already knows is the cheaper kind of correct.
        let out = std::process::Command::new("df")
            .arg("-Pk")
            .arg(data)
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().nth(1)?;
        let available_kb: f64 = line.split_whitespace().nth(3)?.parse().ok()?;
        Some((available_kb / 1_048_576.0 * 10.0).round() / 10.0)
    }
    #[cfg(windows)]
    {
        // GetDiskFreeSpaceExW, called without a crate for it: one function,
        // and a dependency for one function is a dependency to keep updated.
        #[link(name = "kernel32")]
        extern "system" {
            fn GetDiskFreeSpaceExW(
                directory: *const u16,
                free_to_caller: *mut u64,
                total: *mut u64,
                free: *mut u64,
            ) -> i32;
        }
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = data
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let (mut free_to_caller, mut total, mut free) = (0u64, 0u64, 0u64);
        let ok = unsafe {
            GetDiskFreeSpaceExW(wide.as_ptr(), &mut free_to_caller, &mut total, &mut free)
        };
        if ok == 0 {
            return None;
        }
        Some((free_to_caller as f64 / 1_073_741_824.0 * 10.0).round() / 10.0)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = data;
        None
    }
}

/// The last `n` lines, read from the end.
fn last_lines(path: &std::path::Path, n: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    // Generous for a log line, and bounded whatever the file does.
    let want = (n as u64).saturating_mul(400).min(2 * 1024 * 1024);
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let from = size.saturating_sub(want);
    if file.seek(SeekFrom::Start(from)).is_err() {
        return Vec::new();
    }
    let mut text = String::new();
    if file.read_to_string(&mut text).is_err() {
        // A log with something that is not text in it should still answer.
        return vec!["(the log could not be read as text)".into()];
    }
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    // The first line is probably half a line, unless we started at the top.
    if from > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    if lines.len() > n {
        lines.drain(0..lines.len() - n);
    }
    lines
}

pub type Shelf = Arc<Server>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_with_no_maintenance_key_has_no_maintenance_channel() {
        // Not "unlocked" and not "401" — 404, because a shop that never asked
        // to be looked after should not have a door to rattle.
        let config = crate::Config {
            maintenance_key: None,
            ..crate::Config::default()
        };
        let store = crate::Store::temporary(std::path::Path::new("/tmp/hyperview-fleet-test"))
            .expect("a store");
        let server = Server::new(store, config);
        let refused = maintainer(&server, &HeaderMap::new()).err().expect("no way in");
        assert_eq!(refused.0, StatusCode::NOT_FOUND);
    }

    #[test]
    fn the_right_key_gets_in_and_a_wrong_one_does_not() {
        let config = crate::Config {
            maintenance_key: Some("brace-gusset-purlin-shim-42".into()),
            ..crate::Config::default()
        };
        let store = crate::Store::temporary(std::path::Path::new("/tmp/hyperview-fleet-test2"))
            .expect("a store");
        let server = Server::new(store, config);

        let mut right = HeaderMap::new();
        right.insert(KEY_HEADER, "brace-gusset-purlin-shim-42".parse().unwrap());
        assert!(maintainer(&server, &right).is_ok());

        let mut wrong = HeaderMap::new();
        wrong.insert(KEY_HEADER, "brace-gusset-purlin-shim-43".parse().unwrap());
        assert_eq!(
            maintainer(&server, &wrong).err().map(|n| n.0),
            Some(StatusCode::UNAUTHORIZED)
        );

        // And a key of the wrong length is refused rather than compared.
        let mut short = HeaderMap::new();
        short.insert(KEY_HEADER, "brace".parse().unwrap());
        assert!(maintainer(&server, &short).is_err());
    }

    #[test]
    fn a_key_turned_on_from_the_program_opens_the_door_and_turning_it_off_shuts_it() {
        let config = crate::Config::default();
        let store = crate::Store::temporary(std::path::Path::new("/tmp/hyperview-fleet-test4"))
            .expect("a store");
        let server = Server::new(store, config);
        assert!(!fleet_is_on(&server));

        let key = set_fleet_access(&server, true).unwrap().expect("a key");
        assert!(fleet_is_on(&server));
        let mut right = HeaderMap::new();
        right.insert(KEY_HEADER, key.parse().unwrap());
        assert!(maintainer(&server, &right).is_ok());
        // Only the digest is kept.
        let kept = server.store.setting(FLEET_KEY).unwrap().unwrap();
        assert_ne!(kept, key);

        let mut wrong = HeaderMap::new();
        wrong.insert(KEY_HEADER, "0".repeat(key.len()).parse().unwrap());
        assert_eq!(
            maintainer(&server, &wrong).err().map(|n| n.0),
            Some(StatusCode::UNAUTHORIZED)
        );

        set_fleet_access(&server, false).unwrap();
        assert!(!fleet_is_on(&server));
        assert_eq!(
            maintainer(&server, &right).err().map(|n| n.0),
            Some(StatusCode::NOT_FOUND)
        );
    }

    #[test]
    fn never_backed_up_is_not_the_same_as_backed_up_today() {
        let config = crate::Config {
            data: std::path::PathBuf::from("/tmp/hyperview-no-such-folder-at-all"),
            ..crate::Config::default()
        };
        let store = crate::Store::temporary(std::path::Path::new("/tmp/hyperview-fleet-test3"))
            .expect("a store");
        let server = Server::new(store, config);
        // None, which the Fleet turns amber. Not 0.0, which would read as
        // "backed up this morning".
        assert!(days_since_backup(&server).is_none());
    }

    #[test]
    fn the_tail_of_a_log_is_the_end_of_it() {
        let path = std::env::temp_dir().join("hyperview-tail-test.log");
        let mut text = String::new();
        for i in 0..1000 {
            text.push_str(&format!("line {i}\n"));
        }
        std::fs::write(&path, text).expect("a log");
        let lines = last_lines(&path, 10);
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[9], "line 999");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_log_shorter_than_the_tail_asked_for_comes_back_whole() {
        let path = std::env::temp_dir().join("hyperview-short-log.log");
        std::fs::write(&path, "one\ntwo\nthree\n").expect("a log");
        assert_eq!(last_lines(&path, 200), vec!["one", "two", "three"]);
        let _ = std::fs::remove_file(&path);
    }


    // ---- withdrawing a release ----------------------------------------------

    fn a_maintained_server(name: &str) -> (Arc<Server>, HeaderMap) {
        let config = crate::Config {
            maintenance_key: Some("brace-gusset-purlin-shim-42".into()),
            ..crate::Config::default()
        };
        let store = crate::Store::temporary(&std::env::temp_dir().join(name)).expect("a store");
        let server = Arc::new(Server::new(store, config));
        let mut headers = HeaderMap::new();
        headers.insert(KEY_HEADER, "brace-gusset-purlin-shim-42".parse().unwrap());
        (server, headers)
    }

    /// Puts a release in the table the way a successful push leaves one.
    fn on_offer(server: &Server, version: &str, platform: &str, published: &str) {
        server
            .store
            .with(|db| {
                db.execute(
                    "INSERT INTO releases (version, channel, platform, published, notes, bytes,
                                           digest, signature, signing_key, minimum_api, ready)
                     VALUES (?1, 'stable', ?2, ?3, '', 1, '00', '11', 'test', 1, 1)",
                    params![version, platform, published],
                )?;
                Ok(())
            })
            .expect("a release");
    }

    fn offered(server: &Server) -> Vec<(String, String)> {
        server
            .store
            .with(|db| {
                let mut statement = db.prepare(
                    "SELECT version, platform FROM releases WHERE ready = 1 ORDER BY version, platform",
                )?;
                let all = statement
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(all)
            })
            .unwrap_or_default()
    }

    fn withdraw(server: &Arc<Server>, headers: &HeaderMap, body: &str) -> Landed {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(rollback(State(Arc::clone(server)), headers.clone(), body.to_string()))
            .map(|Json(landed)| landed)
            .unwrap_or_else(|_| panic!("the channel should have answered"))
    }

    /// The seats pick what to install by version number. If a withdrawal
    /// picked by publication date instead, a fix for an old version published
    /// after a new one would be the one withdrawn — while the release actually
    /// on offer stayed on offer, and the Fleet's board went green.
    #[test]
    fn the_newest_by_version_is_withdrawn_not_the_one_published_last() {
        let (server, headers) = a_maintained_server("hyperview-fleet-rollback1");
        on_offer(&server, "9.10.0", "windows-x64", "2026-01-01T00:00:00Z");
        on_offer(&server, "9.9.0", "windows-x64", "2026-06-01T00:00:00Z");

        let landed = withdraw(&server, &headers, "");

        assert!(landed.message.contains("9.10.0"), "{}", landed.message);
        assert_eq!(offered(&server), vec![("9.9.0".into(), "windows-x64".into())]);
    }

    /// A release is one thing that happened to be built twice. Withdrawing it
    /// takes back both builds, or the Macs in an office end up on a different
    /// version from the Windows machines beside them.
    #[test]
    fn withdrawing_a_release_takes_back_every_platform_of_it() {
        let (server, headers) = a_maintained_server("hyperview-fleet-rollback2");
        on_offer(&server, "9.4.0", "windows-x64", "2026-09-01T00:00:00Z");
        on_offer(&server, "9.4.0", "macos-universal", "2026-09-01T00:00:00Z");
        on_offer(&server, "9.3.0", "windows-x64", "2026-08-01T00:00:00Z");

        let landed = withdraw(&server, &headers, "");

        assert_eq!(landed.written.len(), 2, "{:?}", landed.written);
        assert_eq!(offered(&server), vec![("9.3.0".into(), "windows-x64".into())]);
    }

    /// Unless the Fleet says which one, for the case the whole thing exists
    /// for: the Mac build is bad and the Windows one is fine.
    #[test]
    fn one_platform_can_be_withdrawn_on_its_own() {
        let (server, headers) = a_maintained_server("hyperview-fleet-rollback3");
        on_offer(&server, "9.4.0", "windows-x64", "2026-09-01T00:00:00Z");
        on_offer(&server, "9.4.0", "macos-universal", "2026-09-01T00:00:00Z");

        let landed = withdraw(&server, &headers, r#"{"platform":"macos-universal"}"#);

        assert_eq!(landed.written, vec!["withdrew 9.4.0 (macos-universal)"]);
        assert_eq!(offered(&server), vec![("9.4.0".into(), "windows-x64".into())]);
    }

    /// The Fleet sends no body at all today, and a body it cannot read must
    /// mean the same as no body rather than an error — the one thing a
    /// withdrawal must never do is quietly not happen.
    #[test]
    fn a_body_that_makes_no_sense_withdraws_everything_rather_than_nothing() {
        let (server, headers) = a_maintained_server("hyperview-fleet-rollback4");
        on_offer(&server, "9.4.0", "windows-x64", "2026-09-01T00:00:00Z");
        on_offer(&server, "9.4.0", "macos-universal", "2026-09-01T00:00:00Z");

        let landed = withdraw(&server, &headers, "not json at all");

        assert_eq!(landed.written.len(), 2, "{:?}", landed.written);
        assert!(offered(&server).is_empty());
    }

    #[test]
    fn withdrawing_when_nothing_is_on_offer_says_so_and_is_not_a_failure() {
        let (server, headers) = a_maintained_server("hyperview-fleet-rollback5");
        let landed = withdraw(&server, &headers, "");
        assert!(landed.written.is_empty());
        assert!(landed.skipped.is_empty(), "not a failure: {:?}", landed.skipped);
        assert!(landed.message.contains("nothing to withdraw"), "{}", landed.message);
    }
}
