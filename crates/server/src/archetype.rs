//! The Office Server's `archetype` part: Archetype's shared jobs, checked out
//! and in, every check-in kept.
//!
//! Moved in from Archetype's own server, with its rules unchanged (one person
//! edits a job at a time, on one computer; a check-in only lands on top of the
//! revision it started from; an administrator can take a check-out back; the
//! history says who did what). What changed in the move: people and sign-ins
//! are the Office Server's own (a writer details, a viewer opens read-only, an
//! administrator takes back), every route answers 404 unless the office's
//! signed licence switches the `archetype` part on, and the tables live in
//! the server's own database (`arch_…`), so the nightly off-site backup and
//! its drill carry Archetype's jobs with everything else.
//!
//! A job's files are kept in the server's file store by their SHA-256. That
//! store holds View's drawings too, so Archetype only ever hands out a file
//! that was sent to it for a job (`arch_files`): a file's fingerprint alone
//! never opens a drawing somebody can't see in View.

use std::collections::{BTreeMap, BTreeSet};

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::{header, HeaderMap, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::api::{Answer, Denied, Shared};
use crate::auth::Who;

/// The most files a job folder holds (Archetype's own limit).
pub const MOST_FILES: usize = 20_000; // WHAT: Archetype's folder limit. WHY: a list longer than any job is refused before it is read

/// The largest single file a move reads: 4 GB.
const MOST_FILE: u64 = 4 << 30; // WHAT: 4 GB. WHY: larger than any drawing or model a job holds, and a cap on what one read takes

/// Archetype's tables, made in the server's database as it opens.
pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS arch_jobs (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, number TEXT NOT NULL DEFAULT '',
    created TEXT NOT NULL, created_by TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 0, updated TEXT NOT NULL DEFAULT '', updated_by TEXT NOT NULL DEFAULT '',
    holder TEXT, holder_name TEXT, held_since TEXT, held_on TEXT);
CREATE TABLE IF NOT EXISTS arch_checkins (
    job TEXT NOT NULL REFERENCES arch_jobs(id), revision INTEGER NOT NULL,
    at TEXT NOT NULL, person TEXT NOT NULL, who TEXT NOT NULL, comment TEXT NOT NULL DEFAULT '',
    files TEXT NOT NULL, count INTEGER NOT NULL, bytes INTEGER NOT NULL, changes TEXT NOT NULL DEFAULT '{}',
    PRIMARY KEY (job, revision));
CREATE TABLE IF NOT EXISTS arch_job_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT, job TEXT NOT NULL REFERENCES arch_jobs(id),
    revision INTEGER NOT NULL, at TEXT NOT NULL, who TEXT NOT NULL, what TEXT NOT NULL, comment TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS arch_files (
    digest TEXT PRIMARY KEY, bytes INTEGER NOT NULL, sent TEXT NOT NULL);
"; // WHAT: Archetype's jobs, check-ins, events, and the files sent for them. WHY: in the server's own database, named arch_ so they never meet View's

// ---- the wire: the same shapes Archetype's own server speaks -------------------------

/// One file of a job folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String, // WHAT: relative to the job folder, forward slashes. WHY: checked by `is_safe_path`
    pub digest: String, // WHAT: SHA-256, lower-case hex. WHY: the file's name in the store
    pub bytes: u64,
}

/// Who has a job checked out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Holder {
    pub person: String, // WHAT: their account's id. WHY: only they may check in
    pub name: String,
    pub since: String,
    #[serde(default)]
    pub machine: String, // WHAT: the computer it's out on. WHY: one computer at a time
}

/// A job, as the list shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub number: String,
    pub created: String,
    pub created_by: String,
    pub revision: u64,
    #[serde(default)]
    pub updated: String,
    #[serde(default)]
    pub updated_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder: Option<Holder>,
    #[serde(default)]
    pub files: usize,
    #[serde(default)]
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct NewJob {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub number: String,
    #[serde(default)]
    pub machine: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct JobFiles {
    pub job: JobInfo,
    pub revision: u64,
    pub files: Vec<FileEntry>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CheckOut {
    #[serde(default)]
    pub machine: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CheckIn {
    #[serde(default)]
    pub base_revision: u64,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub files: Vec<FileEntry>,
    #[serde(default)]
    pub keep: bool,
    #[serde(default)]
    pub machine: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CheckedIn {
    pub job: JobInfo,
    pub revision: u64,
    pub changes: Changes,
}

/// What changed between two check-ins, by path.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changes {
    #[serde(default)]
    pub added: Vec<String>,
    #[serde(default)]
    pub changed: Vec<String>,
    #[serde(default)]
    pub removed: Vec<String>,
}

impl Changes {
    pub fn between(before: &[FileEntry], after: &[FileEntry]) -> Changes {
        let old: BTreeMap<&str, &str> = before.iter().map(|f| (f.path.as_str(), f.digest.as_str())).collect(); // WHAT: the old paths and contents. WHY: compared by path
        let new: BTreeMap<&str, &str> = after.iter().map(|f| (f.path.as_str(), f.digest.as_str())).collect();
        let mut changes = Changes::default();
        for (path, digest) in &new {
            match old.get(path) {
                None => changes.added.push(path.to_string()),
                Some(was) if was != digest => changes.changed.push(path.to_string()),
                Some(_) => {}
            }
        }
        for path in old.keys() {
            if !new.contains_key(path) {
                changes.removed.push(path.to_string());
            }
        }
        changes
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Missing {
    #[serde(default)]
    pub digests: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HistoryEntry {
    pub revision: u64,
    pub at: String,
    pub who: String,
    pub what: String,
    pub comment: String,
    pub files: usize,
    pub bytes: u64,
    pub changes: Changes,
}

// ---- the rules both ends keep ----------------------------------------------------

/// A path inside a job folder that is safe to write on any machine
/// (Archetype's own rule, checked at both ends).
pub fn is_safe_path(path: &str) -> bool {
    if path.is_empty() || path.len() > 400 || path.contains('\\') || path.starts_with('/') {
        return false; // WHAT: empty, too long, backslashes or absolute. WHY: never outside the job folder
    }
    if path.chars().any(|c| c.is_control() || "<>:\"|?*".contains(c)) {
        return false; // WHAT: characters Windows refuses. WHY: every computer must be able to hold it
    }
    const RESERVED: [&str; 22] = ["con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9"]; // WHAT: names Windows keeps for devices. WHY: a file can't be called that there
    path.split('/').all(|part| {
        let stem = part.split('.').next().unwrap_or("").to_ascii_lowercase();
        !part.is_empty() && part != "." && part != ".." && !part.ends_with('.') && !part.ends_with(' ') && !RESERVED.contains(&stem.as_str()) // WHAT: each part a real name. WHY: no climbing out, no two paths that are one file on Windows
    })
}

/// Sixty-four lower-case hex characters, and nothing else.
pub fn is_digest(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) // WHAT: a SHA-256 as the store names files. WHY: what stops a digest walking out of the store
}

/// `2026-10-11T09:14:05Z` → `Oct 11, 9:14`.
fn when(stamp: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (Some(month), Some(day), Some(hour), Some(minute)) = (
        stamp.get(5..7).and_then(|m| m.parse::<usize>().ok()),
        stamp.get(8..10).and_then(|d| d.parse::<u32>().ok()),
        stamp.get(11..13).and_then(|h| h.parse::<u32>().ok()),
        stamp.get(14..16),
    ) else {
        return stamp.to_string();
    };
    if !(1..=12).contains(&month) {
        return stamp.to_string();
    }
    format!("{} {day}, {hour}:{minute}", MONTHS[month - 1]) // WHAT: as Archetype shows it. WHY: the same words on both ends
}

#[derive(Debug, PartialEq)]
pub enum Taken {
    Now,
    AlreadyYours,
}

fn elsewhere(h: &Holder, machine: &str) -> bool {
    !h.machine.trim().is_empty() && !machine.trim().is_empty() && !h.machine.trim().eq_ignore_ascii_case(machine.trim()) // WHAT: out on another computer. WHY: a computer that didn't say is taken at its word
}

fn held_line(h: &Holder) -> String {
    let on = if h.machine.trim().is_empty() { String::new() } else { format!(" on {}", h.machine) };
    format!("{} has it checked out{on} since {}", h.name, when(&h.since))
}

pub fn may_check_out(holder: Option<&Holder>, who: &Who, machine: &str) -> Result<Taken, String> {
    if !who.role.may_write() {
        return Err("Your account opens jobs read-only. An administrator can change that.".into());
    }
    match holder {
        None => Ok(Taken::Now),
        Some(h) if h.person == who.id && elsewhere(h, machine) => Err(format!("You have it checked out on {} since {}. Check it in (or give it back) there first.", h.machine, when(&h.since))),
        Some(h) if h.person == who.id => Ok(Taken::AlreadyYours),
        Some(h) => Err(format!("{}. Open it read-only, or ask them to check it in.", held_line(h))),
    }
}

pub fn may_check_in(holder: Option<&Holder>, who: &Who, base: u64, current: u64, machine: &str) -> Result<(), String> {
    if !who.role.may_write() {
        return Err("Your account opens jobs read-only. An administrator can change that.".into());
    }
    if let Some(h) = holder.filter(|h| h.person == who.id && elsewhere(h, machine)) {
        return Err(format!("It's checked out to you on {}, not this computer, so it's checked in from there. Your work here is safe on this computer: save a copy with File > Save as.", h.machine));
    }
    match holder {
        None => Err("This job isn't checked out to you any more (an administrator may have taken it back), so it can't be checked in. Your work is safe on this computer: save a copy with File > Save as.".into()),
        Some(h) if h.person != who.id => Err(format!("{}, so this can't be checked in over their work. Your work is safe on this computer: save a copy with File > Save as, and talk to them.", held_line(h))),
        Some(_) if base != current => Err(format!("This work started from revision {base}, and the server has revision {current} now. It can't be checked in over that. Save a copy with File > Save as.")),
        Some(_) => Ok(()),
    }
}

pub fn may_release(holder: Option<&Holder>, who: &Who) -> Result<bool, String> {
    match holder {
        None => Ok(false),
        Some(h) if h.person == who.id => Ok(true),
        Some(h) => Err(format!("{}. Only they can give it back (or an administrator can take it back).", held_line(h))),
    }
}

pub fn may_take_back(holder: Option<&Holder>, who: &Who) -> Result<bool, String> {
    if !who.role.may_administer() {
        return Err("Only an administrator can take a check-out back from somebody.".into());
    }
    Ok(holder.is_some())
}

/// The files of a check-in, checked: every path safe and said once, a model
/// in it, every content already sent to this part.
pub fn check_files(files: &[FileEntry], have: impl Fn(&str) -> bool) -> Result<(), String> {
    if files.len() > MOST_FILES {
        return Err(format!("That's more than {MOST_FILES} files: it isn't a job folder."));
    }
    let mut seen = BTreeSet::new();
    for file in files {
        if !is_safe_path(&file.path) {
            return Err(format!("{:?} isn't a name every computer can hold. Rename it and check in again.", file.path));
        }
        if !is_digest(&file.digest) {
            return Err(format!("{} came without a proper fingerprint.", file.path));
        }
        if !seen.insert(file.path.to_lowercase()) {
            return Err(format!("{} is in the job twice.", file.path)); // WHAT: two names one file on Windows. WHY: the next computer would lose one
        }
    }
    if !files.iter().any(|f| f.path == "job.frame") {
        return Err("A job folder holds its model, job.frame, and this one doesn't. Save the job, then check in.".into());
    }
    let missing = files.iter().filter(|f| !have(&f.digest)).count();
    if missing > 0 {
        return Err(format!("{missing} of the job's files haven't reached the server. Check in again; only those are sent."));
    }
    Ok(())
}

// ---- the store --------------------------------------------------------------------

const COLUMNS: &str = "id, name, number, created, created_by, revision, updated, updated_by, holder, holder_name, held_since, held_on";

fn job_from(r: &rusqlite::Row) -> rusqlite::Result<JobInfo> {
    let holder: Option<String> = r.get(8)?;
    Ok(JobInfo {
        id: r.get(0)?,
        name: r.get(1)?,
        number: r.get(2)?,
        created: r.get(3)?,
        created_by: r.get(4)?,
        revision: r.get::<_, i64>(5)? as u64,
        updated: r.get(6)?,
        updated_by: r.get(7)?,
        holder: match holder {
            Some(person) => Some(Holder { person, name: r.get::<_, Option<String>>(9)?.unwrap_or_default(), since: r.get::<_, Option<String>>(10)?.unwrap_or_default(), machine: r.get::<_, Option<String>>(11)?.unwrap_or_default() }),
            None => None,
        },
        files: 0,
        bytes: 0,
    })
}

fn sized(db: &Connection, mut job: JobInfo) -> rusqlite::Result<JobInfo> {
    if job.revision > 0 {
        if let Some((count, bytes)) = db.query_row("SELECT count, bytes FROM arch_checkins WHERE job = ?1 AND revision = ?2", params![job.id, job.revision as i64], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).optional()? {
            job.files = count as usize; // WHAT: the latest check-in's file count. WHY: shown in the list
            job.bytes = bytes as u64;
        }
    }
    Ok(job)
}

pub fn list(db: &Connection) -> rusqlite::Result<Vec<JobInfo>> {
    let mut statement = db.prepare(&format!("SELECT {COLUMNS} FROM arch_jobs ORDER BY lower(name)"))?;
    let jobs = statement.query_map([], job_from)?.collect::<rusqlite::Result<Vec<_>>>()?;
    jobs.into_iter().map(|j| sized(db, j)).collect()
}

pub fn one(db: &Connection, id: &str) -> rusqlite::Result<Option<JobInfo>> {
    match db.query_row(&format!("SELECT {COLUMNS} FROM arch_jobs WHERE id = ?1"), params![id], job_from).optional()? {
        Some(job) => Ok(Some(sized(db, job)?)),
        None => Ok(None),
    }
}

fn files_of(db: &Connection, id: &str, revision: u64) -> anyhow::Result<Option<Vec<FileEntry>>> {
    if revision == 0 {
        return Ok(None);
    }
    let text: Option<String> = db.query_row("SELECT files FROM arch_checkins WHERE job = ?1 AND revision = ?2", params![id, revision as i64], |r| r.get(0)).optional()?;
    Ok(match text {
        Some(text) => Some(serde_json::from_str(&text)?),
        None => None,
    })
}

pub fn history(db: &Connection, id: &str) -> anyhow::Result<Vec<HistoryEntry>> {
    let mut lines: Vec<(u64, i64, HistoryEntry)> = Vec::new();
    let mut statement = db.prepare("SELECT revision, at, who, comment, count, bytes, changes FROM arch_checkins WHERE job = ?1")?;
    for row in statement.query_map(params![id], |r| {
        Ok(HistoryEntry { revision: r.get::<_, i64>(0)? as u64, at: r.get(1)?, who: r.get(2)?, what: "checked in".into(), comment: r.get(3)?, files: r.get::<_, i64>(4)? as usize, bytes: r.get::<_, i64>(5)? as u64, changes: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default() })
    })? {
        let entry = row?;
        lines.push((2 * entry.revision, 0, entry)); // WHAT: a check-in to r sits between r-1's events and r's. WHY: the job's own order, not the clock's
    }
    let mut statement = db.prepare("SELECT revision, at, who, what, comment, id FROM arch_job_events WHERE job = ?1")?;
    for row in statement.query_map(params![id], |r| {
        Ok((r.get::<_, i64>(5)?, HistoryEntry { revision: r.get::<_, i64>(0)? as u64, at: r.get(1)?, who: r.get(2)?, what: r.get(3)?, comment: r.get(4)?, files: 0, bytes: 0, changes: Changes::default() }))
    })? {
        let (seq, entry) = row?;
        lines.push((2 * entry.revision + 1, seq, entry));
    }
    lines.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1))); // WHAT: newest first. WHY: as Archetype shows it
    Ok(lines.into_iter().map(|(_, _, e)| e).collect())
}

fn event(db: &Connection, job: &str, revision: u64, at: &str, who: &str, what: &str, comment: &str) -> rusqlite::Result<()> {
    db.execute("INSERT INTO arch_job_events (job, revision, at, who, what, comment) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![job, revision as i64, at, who, what, comment])?;
    Ok(())
}

fn hold(db: &Connection, id: &str, who: &Who, machine: &str, now: &str) -> rusqlite::Result<()> {
    db.execute("UPDATE arch_jobs SET holder = ?2, holder_name = ?3, held_since = ?4, held_on = ?5 WHERE id = ?1", params![id, who.id, who.name, now, machine])?;
    let revision: i64 = db.query_row("SELECT revision FROM arch_jobs WHERE id = ?1", params![id], |r| r.get(0))?;
    event(db, id, revision as u64, now, &who.name, "checked out", machine)
}

fn let_go(db: &Connection, id: &str, who: &Who, what: &str, now: &str, comment: &str) -> rusqlite::Result<()> {
    db.execute("UPDATE arch_jobs SET holder = NULL, holder_name = NULL, held_since = NULL, held_on = NULL WHERE id = ?1", params![id])?;
    let revision: i64 = db.query_row("SELECT revision FROM arch_jobs WHERE id = ?1", params![id], |r| r.get(0))?;
    event(db, id, revision as u64, now, &who.name, what, comment)
}

fn record_check_in(db: &Connection, job: &JobInfo, who: &Who, comment: &str, files: &[FileEntry], keep: bool, now: &str) -> anyhow::Result<(u64, Changes)> {
    let before = files_of(db, &job.id, job.revision)?.unwrap_or_default();
    let changes = Changes::between(&before, files);
    let revision = job.revision + 1;
    let bytes: u64 = files.iter().map(|f| f.bytes).sum();
    db.execute(
        "INSERT INTO arch_checkins (job, revision, at, person, who, comment, files, count, bytes, changes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![job.id, revision as i64, now, who.id, who.name, comment.trim(), serde_json::to_string(files)?, files.len() as i64, bytes as i64, serde_json::to_string(&changes)?],
    )?; // WHAT: the next revision, its files and what changed. WHY: every check-in kept
    db.execute("UPDATE arch_jobs SET revision = ?2, updated = ?3, updated_by = ?4 WHERE id = ?1", params![job.id, revision as i64, now, who.name])?;
    if !keep {
        db.execute("UPDATE arch_jobs SET holder = NULL, holder_name = NULL, held_since = NULL, held_on = NULL WHERE id = ?1", params![job.id])?; // WHAT: given back. WHY: unless the detailer is carrying on
    }
    Ok((revision, changes))
}

/// The size a file arrived at, when it was sent to Archetype.
fn sent_size(db: &Connection, digest: &str) -> Option<u64> {
    db.query_row("SELECT bytes FROM arch_files WHERE digest = ?1", params![digest], |r| r.get::<_, i64>(0)).optional().ok().flatten().map(|n| n as u64) // WHAT: its recorded size. WHY: compared with what a check-in says
}

/// Whether a file was sent to Archetype (and so may be handed out by it).
fn sent(db: &Connection, digest: &str) -> bool {
    db.query_row("SELECT 1 FROM arch_files WHERE digest = ?1", params![digest], |_| Ok(())).optional().ok().flatten().is_some() // WHAT: in Archetype's own list. WHY: View's drawings share the store and are never reached through here
}

// ---- the routes -------------------------------------------------------------------

/// The part is switched on, or every Archetype route answers as if it didn't exist.
fn on(server: &crate::api::Server) -> Answer<()> {
    if crate::office::serves(server, hub::license::Module::Archetype) {
        Ok(())
    } else {
        Err(Denied::missing("route")) // WHAT: 404. WHY: a part the licence doesn't switch on isn't there at all
    }
}

fn now() -> String {
    crate::api::now() // WHAT: RFC 3339, UTC. WHY: the server's clock, the same for everyone reading the history
}

/// A JSON body, read once the caller is known, no bigger than `most` bytes.
async fn json_body<T: serde::de::DeserializeOwned>(body: Body, most: usize) -> Answer<T> {
    let bytes = axum::body::to_bytes(body, most).await.map_err(|_| Denied::wrong("That's more than this server takes in one go."))?; // WHAT: the body, capped. WHY: read after the checks, never before
    serde_json::from_slice(&bytes).map_err(|e| Denied::wrong(format!("That didn't read: {e}"))) // WHAT: parsed. WHY: a sentence, not a stack
}

/// The most a check-in's list may weigh: twenty thousand files' paths.
const MOST_LIST: usize = 10 * 1024 * 1024; // WHAT: 10 MB of JSON. WHY: twenty thousand files at the longest path fit; no more is read

pub async fn list_jobs(State(server): State<Shared>, headers: HeaderMap) -> Answer<Json<Vec<JobInfo>>> {
    on(&server)?;
    crate::api::caller(&server, &headers)?; // WHAT: anybody signed in. WHY: everyone opens any job read-only
    Ok(Json(server.store.with(|db| Ok(list(db)?)).map_err(|e| Denied::broke("list the jobs", e))?))
}

fn the_job(server: &crate::api::Server, id: &str) -> Answer<JobInfo> {
    server.store.with(|db| Ok(one(db, id)?)).map_err(|e| Denied::broke("find that job", e))?.ok_or_else(|| Denied::missing("job"))
}

pub async fn one_job(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Answer<Json<JobInfo>> {
    on(&server)?;
    crate::api::caller(&server, &headers)?;
    Ok(Json(the_job(&server, &id)?))
}

pub async fn create_job(State(server): State<Shared>, headers: HeaderMap, body: Body) -> Answer<Json<JobInfo>> {
    on(&server)?;
    let who = crate::api::writer(&server, &headers)?; // WHAT: a writer, with sharing on. WHY: a viewer opens, never adds
    let asked: NewJob = json_body(body, 64 * 1024).await?;
    let name = asked.name.trim().to_string();
    if name.is_empty() || name.chars().any(|c| c.is_control()) || name.len() > 120 {
        return Err(Denied::wrong("Give the job a name: what's on its title block."));
    }
    let id = crate::api::fresh_id("job");
    let at = now();
    let made = server
        .store
        .with(|db| {
            let tx = db.unchecked_transaction()?; // WHAT: the name check and the insert as one. WHY: two people putting the same job up at once can't both
            let taken: i64 = tx.query_row("SELECT count(*) FROM arch_jobs WHERE lower(name) = lower(?1) AND number = ?2", params![name, asked.number.trim()], |r| r.get(0))?;
            if taken > 0 {
                return Ok(None);
            }
            tx.execute("INSERT INTO arch_jobs (id, name, number, created, created_by, holder, holder_name, held_since, held_on) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?4, ?8)", params![id, name, asked.number.trim(), at, who.name, who.id, who.name, asked.machine.trim()])?; // WHAT: the job, checked out to whoever put it there. WHY: its first check-in fills it
            event(&tx, &id, 0, &at, &who.name, "put on the server", "")?;
            tx.commit()?;
            Ok(one(db, &id)?)
        })
        .map_err(|e| Denied::broke("put that job on the server", e))?;
    let job = made.ok_or_else(|| Denied::taken(format!("There's already a job called {name} on the server. Open that one, or give this one another name.")))?;
    tracing::info!("{} put Archetype job {} on the server", who.email, job.name);
    Ok(Json(job))
}

#[derive(Deserialize)]
struct Which {
    #[serde(default)]
    revision: Option<u64>,
}

pub async fn job_files(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>, uri: Uri) -> Answer<Json<JobFiles>> {
    on(&server)?;
    crate::api::caller(&server, &headers)?;
    let which: Which = axum::extract::Query::try_from_uri(&uri).map(|q| q.0).map_err(|e| Denied::wrong(format!("Those values don't read: {e}")))?; // WHAT: the revision asked for, read after the checks. WHY: a stranger is answered 401/404 whatever they put there
    let job = the_job(&server, &id)?;
    let revision = which.revision.unwrap_or(job.revision);
    let files = if revision == 0 { Some(Vec::new()) } else { server.store.with(|db| files_of(db, &id, revision)).map_err(|e| Denied::broke("read that job's files", e))? };
    let files = files.ok_or_else(|| Denied::missing("revision of that job"))?;
    Ok(Json(JobFiles { job, revision, files }))
}

pub async fn job_history(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Answer<Json<Vec<HistoryEntry>>> {
    on(&server)?;
    crate::api::caller(&server, &headers)?;
    the_job(&server, &id)?;
    Ok(Json(server.store.with(|db| history(db, &id)).map_err(|e| Denied::broke("read that job's history", e))?))
}

pub async fn check_out(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>, body: Body) -> Answer<Json<JobInfo>> {
    on(&server)?;
    let who = crate::api::writer(&server, &headers)?; // WHAT: someone who may change jobs, with sharing on. WHY: a viewer is refused before anything is read; may_check_out says it again in Archetype's words
    let asked: CheckOut = json_body(body, 64 * 1024).await?;
    let at = now();
    let outcome = server
        .store
        .with(|db| {
            let tx = db.unchecked_transaction()?; // WHAT: looked at and taken as one. WHY: two people pressing at once can't both have it
            let Some(job) = one(&tx, &id)? else { return Ok(None) };
            let result = match may_check_out(job.holder.as_ref(), &who, asked.machine.trim()) {
                Ok(Taken::Now) => {
                    hold(&tx, &id, &who, asked.machine.trim(), &at)?;
                    Ok(())
                }
                Ok(Taken::AlreadyYours) => Ok(()),
                Err(why) => Err(why),
            };
            tx.commit()?;
            Ok(Some(result))
        })
        .map_err(|e| Denied::broke("check that job out", e))?;
    match outcome {
        None => Err(Denied::missing("job")),
        Some(Err(why)) => Err(Denied::taken(why)),
        Some(Ok(())) => Ok(Json(the_job(&server, &id)?)),
    }
}

pub async fn check_in(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>, body: Body) -> Answer<Json<CheckedIn>> {
    on(&server)?;
    let who = crate::api::writer(&server, &headers)?; // WHAT: someone who may change jobs, with sharing on. WHY: a viewer is refused before anything is read
    let asked: CheckIn = json_body(body, MOST_LIST).await?;
    check_files(&asked.files, |_| true).map_err(Denied::wrong)?; // WHAT: the list's length, paths and fingerprints first. WHY: nothing is looked up for a list no job could hold
    let sent_here = server.store.with(|db| Ok(asked.files.iter().filter_map(|f| sent_size(db, &f.digest).map(|n| (f.digest.clone(), n))).collect::<BTreeMap<_, _>>())).map_err(|e| Denied::broke("check the job's files", e))?; // WHAT: the files Archetype has, with the size each arrived at. WHY: a check-in may only name files sent for it
    if let Some(f) = asked.files.iter().find(|f| sent_here.get(&f.digest).is_some_and(|n| *n != f.bytes)) {
        return Err(Denied::wrong(format!("{} is listed at a size it didn't arrive at. Check in again.", f.path))); // WHAT: refused. WHY: the job's sizes and history say what really came
    }
    check_files(&asked.files, |d| sent_here.contains_key(d)).map_err(Denied::wrong)?; // WHAT: every file already sent. WHY: only those can be fetched later
    let at = now();
    let outcome = server
        .store
        .with(|db| {
            let tx = db.unchecked_transaction()?; // WHAT: the check and the write as one. WHY: never over someone else's revision
            let Some(job) = one(&tx, &id)? else { return Ok(None) };
            if let Err(why) = may_check_in(job.holder.as_ref(), &who, asked.base_revision, job.revision, asked.machine.trim()) {
                return Ok(Some(Err(why)));
            }
            let done = record_check_in(&tx, &job, &who, &asked.comment, &asked.files, asked.keep, &at)?;
            tx.commit()?;
            Ok(Some(Ok(done)))
        })
        .map_err(|e| Denied::broke("check that work in", e))?;
    match outcome {
        None => Err(Denied::missing("job")),
        Some(Err(why)) => Err(Denied::taken(why)),
        Some(Ok((revision, changes))) => {
            let job = the_job(&server, &id)?;
            tracing::info!("{} checked in Archetype job {} revision {revision}", who.email, job.name);
            Ok(Json(CheckedIn { job, revision, changes }))
        }
    }
}

pub async fn release(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Answer<Json<JobInfo>> {
    on(&server)?;
    let who = crate::api::caller(&server, &headers)?;
    let at = now();
    let outcome = server
        .store
        .with(|db| {
            let tx = db.unchecked_transaction()?;
            let Some(job) = one(&tx, &id)? else { return Ok(None) };
            let result = match may_release(job.holder.as_ref(), &who) {
                Ok(true) => {
                    let_go(&tx, &id, &who, "gave it back", &at, "")?;
                    Ok(())
                }
                Ok(false) => Ok(()),
                Err(why) => Err(why),
            };
            tx.commit()?;
            Ok(Some(result))
        })
        .map_err(|e| Denied::broke("give that job back", e))?;
    match outcome {
        None => Err(Denied::missing("job")),
        Some(Err(why)) => Err(Denied::taken(why)),
        Some(Ok(())) => Ok(Json(the_job(&server, &id)?)),
    }
}

pub async fn take_back(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Answer<Json<JobInfo>> {
    on(&server)?;
    let who = crate::api::administrator(&server, &headers)?; // WHAT: an administrator signed in, never a key. WHY: taking someone's check-out is theirs to decide
    let at = now();
    let outcome = server
        .store
        .with(|db| {
            let tx = db.unchecked_transaction()?;
            let Some(job) = one(&tx, &id)? else { return Ok(None) };
            let result = match may_take_back(job.holder.as_ref(), &who) {
                Ok(true) => {
                    let from = job.holder.as_ref().map(|h| format!("from {}", h.name)).unwrap_or_default();
                    let_go(&tx, &id, &who, "took it back", &at, &from)?; // WHAT: given back, in the history by name. WHY: the detailer sees who took it
                    Ok(())
                }
                Ok(false) => Ok(()),
                Err(why) => Err(why),
            };
            tx.commit()?;
            Ok(Some(result))
        })
        .map_err(|e| Denied::broke("take that job back", e))?;
    match outcome {
        None => Err(Denied::missing("job")),
        Some(Err(why)) => Err(Denied::taken(why)),
        Some(Ok(())) => Ok(Json(the_job(&server, &id)?)),
    }
}

pub async fn missing_files(State(server): State<Shared>, headers: HeaderMap, body: Body) -> Answer<Json<Missing>> {
    on(&server)?;
    crate::api::writer(&server, &headers)?; // WHAT: someone who may check in. WHY: only a check-in asks which files to send
    let asked: Missing = json_body(body, MOST_LIST).await?;
    if asked.digests.len() > MOST_FILES {
        return Err(Denied::wrong("That's more files than any job holds."));
    }
    if let Some(bad) = asked.digests.iter().find(|d| !is_digest(d)) {
        return Err(Denied::wrong(format!("{bad:.70} is not a file's fingerprint.")));
    }
    let mut digests: Vec<String> = server.store.with(|db| Ok(asked.digests.into_iter().filter(|d| !sent(db, d)).collect())).map_err(|e| Denied::broke("look for those files", e))?; // WHAT: the ones Archetype hasn't been sent. WHY: View's files are never reported, so nothing is learned about them
    digests.sort();
    digests.dedup();
    Ok(Json(Missing { digests }))
}

pub async fn put_file(State(server): State<Shared>, headers: HeaderMap, UrlPath(digest): UrlPath<String>, body: Body) -> Answer<Json<serde_json::Value>> {
    on(&server)?;
    crate::api::writer(&server, &headers)?; // WHAT: someone who may check in. WHY: the store isn't filled by viewers or strangers
    if !is_digest(&digest) {
        return Err(Denied::wrong("That is not a file's fingerprint."));
    }
    let bytes = axum::body::to_bytes(body, server.config.largest_upload as usize).await.map_err(|_| Denied::wrong("That file is larger than this server takes."))?; // WHAT: the file, read after the checks, no bigger than the office's upload limit. WHY: nothing is buffered for a stranger
    if crate::store::hex(&Sha256::digest(&bytes)) != digest {
        return Err(Denied::wrong("That file arrived damaged. Check in again.")); // WHAT: refused. WHY: the store names files by their bytes, and a wrong name would hand somebody the wrong file
    }
    let size = bytes.len() as i64;
    let keeping = server.clone();
    tokio::task::spawn_blocking(move || keeping.store.put_blob(&bytes)).await.map_err(|e| Denied::broke("keep that file", e))?.map_err(|e| Denied::broke("keep that file", e))?;
    server.store.with(|db| Ok(db.execute("INSERT OR IGNORE INTO arch_files (digest, bytes, sent) VALUES (?1, ?2, ?3)", params![digest, size, now()])?)).map_err(|e| Denied::broke("note that file", e))?; // WHAT: Archetype's own list. WHY: only files sent here are handed out here
    Ok(Json(serde_json::json!({ "kept": digest })))
}

pub async fn get_file(State(server): State<Shared>, headers: HeaderMap, UrlPath(digest): UrlPath<String>) -> Answer<Response> {
    on(&server)?;
    crate::api::caller(&server, &headers)?;
    if !is_digest(&digest) || !server.store.with(|db| Ok(sent(db, &digest))).unwrap_or(false) {
        return Err(Denied::missing("file")); // WHAT: not one sent to Archetype. WHY: a View drawing's fingerprint opens nothing here
    }
    let reading = server.clone();
    let bytes = tokio::task::spawn_blocking(move || reading.store.get_blob(&digest)).await.map_err(|e| Denied::broke("read that file", e))?.map_err(|e| Denied::broke("read that file", e))?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream")], Body::from(bytes)).into_response())
}

// ---- moving in from Archetype's own server -----------------------------------------

/// What a move brought in.
#[derive(Debug, PartialEq)]
pub struct Moved {
    pub jobs: i64, // WHAT: jobs. WHY: every one comes
    pub checkins: i64, // WHAT: every check-in of each. WHY: the history comes with them
    pub files: i64, // WHAT: files copied, each checked by its SHA-256. WHY: an old revision must still open
    pub released: i64, // WHAT: jobs that were checked out there. WHY: given back here, since the accounts are the Office Server's now
}

/// Moves Archetype's server's jobs (its data folder, `from`: archetype.sqlite
/// and files/) into this server's archetype part. Only into an empty one; the
/// old folder is only read (the database opened immutable, no links followed);
/// every file every check-in names is checked against its SHA-256 and size
/// before any job is written, and the jobs, check-ins and history go in as one
/// transaction. Check-outs there are given back here: the people sign in with
/// the office's accounts, and whoever had a job checks it out again.
pub fn move_from(from: &std::path::Path, database: &std::path::Path, blobs: &std::path::Path) -> Result<Moved, String> {
    let old_db = from.join("archetype.sqlite");
    if !old_db.is_file() {
        return Err(format!("{} isn't Archetype's server's data folder (no archetype.sqlite in it).", from.display())); // WHAT: the wrong folder. WHY: said plainly, nothing touched
    }
    let full = std::path::absolute(&old_db).map_err(|e| e.to_string())?;
    let text = full.to_str().ok_or("that folder's name can't be read")?.replace('\\', "/");
    let escaped: String = text.chars().map(|c| match c { '%' => "%25".to_string(), '?' => "%3f".to_string(), '#' => "%23".to_string(), c => c.to_string() }).collect(); // WHAT: characters a URI reads specially. WHY: a folder named with them still opens
    let uri = format!("file:{}{escaped}?mode=ro&immutable=1", if escaped.starts_with('/') { "" } else { "/" }); // WHAT: read-only, immutable. WHY: SQLite writes nothing beside the old database
    let old = Connection::open_with_flags(&uri, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI).map_err(|e| format!("reading {}: {e}", old_db.display()))?;
    old.execute_batch("BEGIN").map_err(|e| e.to_string())?; // WHAT: one snapshot. WHY: jobs and check-ins from the same moment
    let new = Connection::open(database).map_err(|e| format!("opening this server's database: {e}"))?;
    new.busy_timeout(std::time::Duration::from_secs(10)).map_err(|e| e.to_string())?;
    new.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    let here: i64 = new.query_row("SELECT COUNT(*) FROM arch_jobs", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if here > 0 {
        return Err("This server already has Archetype jobs; a move only goes into an empty one, so nothing here is overwritten.".into());
    }
    let read = |e: rusqlite::Error| format!("reading the old jobs: {e}");
    type JobRow = (String, String, String, String, String, i64, String, String, Option<String>);
    let mut st = old.prepare("SELECT id, name, number, created, created_by, revision, updated, updated_by, holder FROM jobs ORDER BY id").map_err(read)?;
    let jobs: Vec<JobRow> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))).map_err(read)?.collect::<Result<_, _>>().map_err(read)?;
    type CheckinRow = (String, i64, String, String, String, String, String, i64, i64, String);
    let mut st = old.prepare("SELECT job, revision, at, person, who, comment, files, count, bytes, changes FROM checkins ORDER BY job, revision").map_err(read)?;
    let checkins: Vec<CheckinRow> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?))).map_err(read)?.collect::<Result<_, _>>().map_err(read)?;
    type EventRow = (String, i64, String, String, String, String);
    let mut st = old.prepare("SELECT job, revision, at, who, what, comment FROM job_events ORDER BY id").map_err(read)?;
    let events: Vec<EventRow> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).map_err(read)?.collect::<Result<_, _>>().map_err(read)?;
    for (_, name, ..) in &jobs {
        if name.trim().is_empty() || name.chars().any(|c| c.is_control()) || name.len() > 120 {
            return Err(format!("the old server has a job whose name isn't allowed here ({name:.40}); no job was moved")); // WHAT: refused. WHY: only names this server would take come in
        }
    }
    let mut wanted: BTreeMap<String, u64> = BTreeMap::new(); // WHAT: every file any check-in names, with its size. WHY: an old revision must still open after the move
    for (job, revision, _, _, _, _, files, ..) in &checkins {
        let list: Vec<FileEntry> = serde_json::from_str(files).map_err(|_| format!("check-in {revision} of job {job} doesn't read; no job was moved"))?;
        check_files(&list, |_| true).map_err(|why| format!("check-in {revision} of job {job}: {why} No job was moved."))?; // WHAT: the same rules a check-in passes here. WHY: paths that could leave a folder never come in
        for f in list {
            wanted.insert(f.digest, f.bytes);
        }
    }
    let store = crate::store::Store::open(database, blobs).map_err(|e| format!("opening this server's files: {e}"))?;
    let mut moved = Moved { jobs: 0, checkins: 0, files: 0, released: 0 };
    for (digest, bytes) in &wanted {
        let path = from.join("files").join(&digest[0..2]).join(&digest[2..4]).join(digest); // WHAT: the old server's layout. WHY: files/ab/cd/<digest>
        let kind = std::fs::symlink_metadata(&path).map_err(|e| format!("file {digest:.12}… is missing from the old folder ({e}); no job was moved"))?;
        if !kind.file_type().is_file() {
            return Err(format!("file {digest:.12}… isn't a plain file; no job was moved")); // WHAT: a link, folder or device. WHY: never followed
        }
        let mut content = Vec::new();
        use std::io::Read;
        if *bytes > MOST_FILE {
            return Err(format!("file {digest:.12}… is listed at {bytes} bytes, more than any job file; no job was moved")); // WHAT: refused. WHY: never read a file that size into memory
        }
        std::fs::File::open(&path).and_then(|f| f.take(bytes.saturating_add(1)).read_to_end(&mut content)).map_err(|e| format!("reading file {digest:.12}…: {e}"))?; // WHAT: one byte past its size at most. WHY: a longer file is caught below, never read whole
        if content.len() as u64 != *bytes || crate::store::hex(&Sha256::digest(&content)) != *digest {
            return Err(format!("file {digest:.12}… doesn't match its fingerprint; no job was moved")); // WHAT: damaged. WHY: never brought in as if good
        }
        store.put_blob(&content).map_err(|e| format!("copying a file: {e}"))?;
        moved.files += 1;
    }
    let tx = rusqlite::Transaction::new_unchecked(&new, rusqlite::TransactionBehavior::Immediate).map_err(|e| e.to_string())?; // WHAT: all rows or none, the write lock held from the start. WHY: a move cut short leaves nothing half there, and nobody adds a job in between
    let still_empty: i64 = tx.query_row("SELECT COUNT(*) FROM arch_jobs", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if still_empty > 0 {
        return Err("A job was put on this server while the move ran; nothing was moved. Stop the server, then move again.".into()); // WHAT: refused, inside the lock. WHY: never mixed with jobs made meanwhile
    }
    let at = crate::api::now();
    for (id, name, number, created, created_by, revision, updated, updated_by, holder) in &jobs {
        tx.execute("INSERT INTO arch_jobs (id, name, number, created, created_by, revision, updated, updated_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)", params![id, name, number, created, created_by, revision, updated, updated_by]).map_err(|e| format!("moving job {name}: {e}"))?; // WHAT: the same id and name, given back. WHY: Archetype's links keep working; the old accounts aren't accounts here
        moved.jobs += 1;
        if holder.is_some() {
            moved.released += 1;
            tx.execute("INSERT INTO arch_job_events (job, revision, at, who, what, comment) VALUES (?1, ?2, ?3, 'move', 'given back by the move', '')", params![id, revision, at]).map_err(|e| e.to_string())?; // WHAT: noted in its history. WHY: whoever had it sees why it's free
        }
    }
    for (job, revision, at, person, who, comment, files, count, bytes, changes) in &checkins {
        tx.execute("INSERT INTO arch_checkins (job, revision, at, person, who, comment, files, count, bytes, changes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)", params![job, revision, at, person, who, comment, files, count, bytes, changes]).map_err(|e| format!("moving a check-in of job {job}: {e}"))?;
        moved.checkins += 1;
    }
    for (job, revision, at, who, what, comment) in &events {
        tx.execute("INSERT INTO arch_job_events (job, revision, at, who, what, comment) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![job, revision, at, who, what, comment]).map_err(|e| format!("moving job {job}'s history: {e}"))?;
    }
    for (digest, bytes) in &wanted {
        tx.execute("INSERT OR IGNORE INTO arch_files (digest, bytes, sent) VALUES (?1, ?2, ?3)", params![digest, *bytes as i64, at]).map_err(|e| e.to_string())?; // WHAT: every file now Archetype's here. WHY: only those are handed out by it
    }
    tx.commit().map_err(|e| format!("finishing the move: {e}"))?;
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hub::Role;

    fn person(id: &str, name: &str, role: Role) -> Who {
        Who { id: id.into(), name: name.into(), email: format!("{id}@example.com"), role, key: None }
    }

    fn holder(id: &str, name: &str) -> Holder {
        Holder { person: id.into(), name: name.into(), since: "2026-10-11T09:14:00Z".into(), machine: "DESK-3".into() }
    }

    #[test]
    fn the_rules_are_archetype_s_own_with_the_office_s_roles() {
        let sam = person("usr_s", "Sam", Role::Estimator); // WHAT: an estimator details. WHY: the office's writer
        assert_eq!(may_check_out(None, &sam, "SAM-PC"), Ok(Taken::Now));
        assert_eq!(may_check_out(Some(&holder("usr_s", "Sam")), &sam, "DESK-3"), Ok(Taken::AlreadyYours));
        assert!(may_check_out(Some(&holder("usr_s", "Sam")), &sam, "LAPTOP").unwrap_err().starts_with("You have it checked out on DESK-3"), "one computer at a time");
        assert_eq!(may_check_out(Some(&holder("usr_j", "Jane Smith")), &sam, "SAM-PC").unwrap_err(), "Jane Smith has it checked out on DESK-3 since Oct 11, 9:14. Open it read-only, or ask them to check it in.");
        let viewer = person("usr_v", "Vic", Role::Viewer);
        assert!(may_check_out(None, &viewer, "").unwrap_err().contains("read-only"));
        assert!(may_check_in(Some(&holder("usr_s", "Sam")), &sam, 4, 5, "DESK-3").unwrap_err().contains("revision 4"), "never over a newer revision");
        assert!(may_check_in(None, &sam, 4, 4, "DESK-3").unwrap_err().contains("safe on this computer"));
        let boss = person("usr_b", "Boss", Role::Admin);
        assert!(may_take_back(Some(&holder("usr_j", "Jane")), &sam).is_err());
        assert_eq!(may_take_back(Some(&holder("usr_j", "Jane")), &boss), Ok(true));
        assert!(may_release(Some(&holder("usr_j", "Jane")), &sam).is_err());
    }

    fn a_folder(what: &str) -> std::path::PathBuf {
        let n: u64 = rand::random();
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("{what}-{n:016x}"))
    }

    /// An old Archetype server's data folder, laid out as Archetype's server lays it out.
    fn an_old_archetype_server() -> std::path::PathBuf {
        let dir = a_folder("old-arch");
        std::fs::create_dir_all(&dir).unwrap();
        let db = Connection::open(dir.join("archetype.sqlite")).unwrap();
        db.execute_batch(
            "CREATE TABLE people (id TEXT PRIMARY KEY, name TEXT, email TEXT, password TEXT, role TEXT, created TEXT);
             CREATE TABLE jobs (id TEXT PRIMARY KEY, name TEXT NOT NULL, number TEXT NOT NULL DEFAULT '', created TEXT NOT NULL, created_by TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0, updated TEXT NOT NULL DEFAULT '', updated_by TEXT NOT NULL DEFAULT '', holder TEXT, holder_name TEXT, held_since TEXT, held_on TEXT);
             CREATE TABLE checkins (job TEXT NOT NULL, revision INTEGER NOT NULL, at TEXT NOT NULL, person TEXT NOT NULL, who TEXT NOT NULL, comment TEXT NOT NULL DEFAULT '', files TEXT NOT NULL, count INTEGER NOT NULL, bytes INTEGER NOT NULL, changes TEXT NOT NULL DEFAULT '{}', PRIMARY KEY (job, revision));
             CREATE TABLE job_events (id INTEGER PRIMARY KEY AUTOINCREMENT, job TEXT NOT NULL, revision INTEGER NOT NULL, at TEXT NOT NULL, who TEXT NOT NULL, what TEXT NOT NULL, comment TEXT NOT NULL DEFAULT '');",
        )
        .unwrap(); // WHAT: Archetype's server's tables. WHY: what a move reads
        let put = |bytes: &[u8]| {
            let d = crate::store::hex(&Sha256::digest(bytes));
            let at = dir.join("files").join(&d[0..2]).join(&d[2..4]);
            std::fs::create_dir_all(&at).unwrap();
            std::fs::write(at.join(&d), bytes).unwrap();
            d
        };
        let (m1, m2, u) = (put(b"model one"), put(b"model two"), put(b"underlay"));
        let list = |m: &str| format!(r#"[{{"path":"job.frame","digest":"{m}","bytes":9}},{{"path":"underlays/S-201.pdf","digest":"{u}","bytes":8}}]"#);
        db.execute("INSERT INTO jobs VALUES ('job_a', 'Akron Awning', '2640', '2026-10-01T00:00:00Z', 'Sam', 2, '2026-10-02T00:00:00Z', 'Sam', 'usr_s', 'Sam', '2026-10-03T00:00:00Z', 'DESK-3')", []).unwrap(); // WHAT: checked out to Sam. WHY: given back by the move
        for (rev, m) in [(1, &m1), (2, &m2)] {
            db.execute("INSERT INTO checkins VALUES ('job_a', ?1, '2026-10-02T00:00:00Z', 'usr_s', 'Sam', 'work', ?2, 2, 17, '{}')", params![rev, list(m)]).unwrap();
        }
        db.execute("INSERT INTO job_events (job, revision, at, who, what) VALUES ('job_a', 0, '2026-10-01T00:00:00Z', 'Sam', 'put on the server')", []).unwrap();
        dir
    }

    #[test]
    fn an_archetype_server_moves_in_whole_and_its_folder_is_left_as_it_was() {
        let from = an_old_archetype_server();
        let before = std::fs::read(from.join("archetype.sqlite")).unwrap();
        let to = a_folder("arch-to");
        std::fs::create_dir_all(&to).unwrap();
        let (db, blobs) = (to.join("hyperview.sqlite"), to.join("files"));
        assert_eq!(move_from(&from, &db, &blobs).unwrap(), Moved { jobs: 1, checkins: 2, files: 3, released: 1 });
        let here = Connection::open(&db).unwrap();
        let job = one(&here, "job_a").unwrap().unwrap();
        assert_eq!((job.revision, job.holder.is_none(), job.files), (2, true, 2), "both check-ins, given back");
        assert_eq!(files_of(&here, "job_a", 1).unwrap().unwrap()[0].path, "job.frame", "an old revision still lists its files");
        assert!(history(&here, "job_a").unwrap().iter().any(|h| h.what == "given back by the move"));
        assert_eq!(std::fs::read(from.join("archetype.sqlite")).unwrap(), before, "the old database is only read");
        assert!(move_from(&from, &db, &blobs).unwrap_err().contains("already has Archetype jobs"));
    }

    #[test]
    fn a_damaged_file_or_a_path_that_could_leave_a_folder_stops_the_move() {
        let from = an_old_archetype_server();
        let d = crate::store::hex(&Sha256::digest(b"model one"));
        std::fs::write(from.join("files").join(&d[0..2]).join(&d[2..4]).join(&d), b"tampered!").unwrap(); // WHAT: same size, other bytes. WHY: the fingerprint must catch it
        let to = a_folder("arch-to");
        std::fs::create_dir_all(&to).unwrap();
        assert!(move_from(&from, &to.join("hyperview.sqlite"), &to.join("files")).unwrap_err().contains("doesn't match its fingerprint"));
        let bad = an_old_archetype_server();
        Connection::open(bad.join("archetype.sqlite")).unwrap().execute("UPDATE checkins SET files = replace(files, 'underlays/S-201.pdf', '../../evil.pdf')", []).unwrap(); // WHAT: a path out of the folder. WHY: never brought in
        assert!(move_from(&bad, &to.join("hyperview.sqlite"), &to.join("files")).unwrap_err().contains("isn't a name every computer can hold"));
        assert_eq!(Connection::open(to.join("hyperview.sqlite")).unwrap().query_row("SELECT COUNT(*) FROM arch_jobs", [], |r| r.get::<_, i64>(0)).unwrap(), 0, "no job moved");
    }

    fn entry(path: &str, fill: char) -> FileEntry {
        FileEntry { path: path.into(), digest: fill.to_string().repeat(64), bytes: 3 }
    }

    #[test]
    fn a_check_in_holds_a_model_every_path_safe_once_and_every_file_already_sent() {
        let good = [entry("job.frame", 'a'), entry("underlays/S-201.pdf", 'b')];
        assert_eq!(check_files(&good, |_| true), Ok(()));
        assert!(check_files(&[entry("underlays/S-201.pdf", 'b')], |_| true).unwrap_err().contains("job.frame"));
        for bad in ["../x", "a/../b", "/abs", "C:x", "con.txt", "a\\b", "dot.", "space "] {
            assert!(check_files(&[entry("job.frame", 'a'), entry(bad, 'b')], |_| true).is_err(), "{bad}");
        }
        assert!(check_files(&[entry("job.frame", 'a'), entry("A.pdf", 'b'), entry("a.pdf", 'c')], |_| true).unwrap_err().contains("twice"));
        let mut bad = entry("job.frame", 'a');
        bad.digest = "../../etc/passwd".into();
        assert!(check_files(&[bad], |_| true).is_err());
        assert!(check_files(&good, |d| d.starts_with('a')).unwrap_err().starts_with("1 of the job's files"));
    }
}
