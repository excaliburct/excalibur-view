//! The Office Server's `cad` part: Excalibur CAD's office drawings, checked
//! out and in, every version kept.
//!
//! Moved in from Excalibur CAD's own server (its store, nearly line for line),
//! so an office runs one server instead of two. What changed in the move:
//! people and sign-ins are the Office Server's own (one account for View and
//! CAD, the same roles), and every route answers 404 unless the office's
//! signed licence switches the `cad` part on (`crate::office`).
//!
//! Kept in the server's own database (`cad_drawings`, `cad_versions`,
//! `cad_log`) and its own file store, so the nightly off-site backup and its
//! restore drill carry CAD's drawings with View's. A drawing's bytes are kept
//! once, named by their SHA-256, written
//! beside and then moved into place, so a crash never leaves half a file and
//! the same drawing saved twice takes the room once. Names and folders are
//! only labels in the database: they never become file paths, so no name can
//! reach outside the data folder. Nothing is deleted: an archived drawing is
//! hidden, with all its versions kept.

// WHAT: whole-number conversions at the database's edge. WHY: SQLite keeps every integer as i64, and versions, sizes and times here are never negative or anywhere near 2^63.
#![allow(clippy::cast_sign_loss, clippy::cast_possible_wrap, clippy::cast_possible_truncation)]

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::Uri;
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::api::{Answer, Denied, Shared};

/// The longest a drawing may be (a real drawing is far under this).
pub const MOST_BYTES: usize = 256 * 1024 * 1024; // WHAT: 256 MB, CAD's own limit. WHY: the same drawings fit as before the move

/// CAD's tables, made in the server's database as it opens (`crate::store`).
pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS cad_drawings (
    id INTEGER PRIMARY KEY, folder TEXT NOT NULL, name TEXT NOT NULL,
    locked_by TEXT, locked_at INTEGER, archived INTEGER NOT NULL DEFAULT 0,
    created_by TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE UNIQUE INDEX IF NOT EXISTS cad_drawing_place ON cad_drawings(folder, name) WHERE archived = 0;
CREATE TABLE IF NOT EXISTS cad_versions (
    drawing INTEGER NOT NULL REFERENCES cad_drawings(id), version INTEGER NOT NULL,
    sha256 TEXT NOT NULL, size INTEGER NOT NULL, saved_by TEXT NOT NULL,
    saved_at INTEGER NOT NULL, note TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (drawing, version));
CREATE TABLE IF NOT EXISTS cad_log (
    at INTEGER NOT NULL, who TEXT NOT NULL, what TEXT NOT NULL, detail TEXT NOT NULL);
"; // WHAT: CAD's own tables, without its people and sign-ins, named cad_. WHY: the Office Server's accounts are used instead, and the names never meet View's

pub struct Store {
    db: Connection,
    blobs: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct Drawing {
    pub id: i64,
    pub folder: String,
    pub name: String,
    pub version: i64,
    pub size: i64,
    pub saved_by: String,
    pub saved_at: i64,
    pub locked_by: Option<String>,
    pub locked_at: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct Version {
    pub version: i64,
    pub size: i64,
    pub sha256: String,
    pub saved_by: String,
    pub saved_at: i64,
    pub note: String,
}

/// Why something was refused, in words a person can act on.
#[derive(Debug, PartialEq)]
pub enum Refused {
    NotFound,
    /// Someone else has it out: who, and since when.
    Locked(String, i64),
    /// Saving needs the drawing checked out to you first.
    NotYours,
    /// It was saved by someone else since you opened it: the version now.
    Stale(i64),
    Exists,
    Bad(String),
}

#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64) // WHAT: seconds since 1970. WHY: CAD's own clock, so moved drawings read the same
}

/// A drawing's or folder part's name: 1 to 120 characters, no control
/// characters or slashes, not just dots, no spaces at the ends.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn good_name(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= 120 && s.trim() == s && !s.chars().any(|c| c.is_control() || c == '/' || c == '\\') && !s.chars().all(|c| c == '.') // WHAT: CAD's rule. WHY: a label that can never be mistaken for a path
}

/// A folder: names joined by '/', at most 8 deep; empty is the top.
pub fn good_folder(s: &str) -> bool {
    s.is_empty() || (s.split('/').count() <= 8 && s.split('/').all(good_name)) // WHAT: CAD's rule. WHY: the same
}

impl Store {
    /// CAD's drawings in the server's database and file store.
    ///
    /// # Errors
    ///
    /// Refused, in words a person can act on, when it can't be done.
    pub fn open(database: &Path, blobs: &Path) -> rusqlite::Result<Store> {
        std::fs::create_dir_all(blobs).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?; // WHAT: the server's file store. WHY: made by the server already; here for a test's own folder
        let db = Connection::open(database)?;
        db.busy_timeout(std::time::Duration::from_secs(10))?; // WHAT: waits for another request's write. WHY: each request opens its own connection
        db.execute_batch("PRAGMA foreign_keys = ON;")?; // WHAT: versions belong to drawings. WHY: a version for no drawing is refused
        db.execute_batch(SCHEMA)?; // WHAT: CAD's tables, if not made yet. WHY: the server makes them as it opens; a test's own database too
        Ok(Store { db, blobs: blobs.to_path_buf() })
    }

    pub fn log(&self, who: &str, what: &str, detail: &str) {
        let _ = self.db.execute("INSERT INTO cad_log (at, who, what, detail) VALUES (?1, ?2, ?3, ?4)", params![now(), who, what, detail]); // WHAT: one line of what was done. WHY: who checked out, saved, forced or archived what
    }

    /// A drawing's bytes, kept once by their hash.
    fn put_blob(&self, bytes: &[u8]) -> std::io::Result<String> {
        let sha = crate::store::hex(&Sha256::digest(bytes)); // WHAT: the bytes' SHA-256. WHY: their name, so the same drawing is kept once
        let path = self.blob_path(&sha);
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let part = path.with_extension(format!("part{:016x}", rand::random::<u64>())); // WHAT: a name of its own. WHY: two saves of the same bytes at once never write into one file
            let written = (|| {
                use std::io::Write;
                let mut f = std::fs::File::create(&part)?;
                f.write_all(bytes)?;
                f.sync_all() // WHAT: on the disk before it is named. WHY: a power cut never leaves a short file under a whole drawing's name
            })();
            if let Err(e) = written.and_then(|()| std::fs::rename(&part, &path)) {
                let _ = std::fs::remove_file(&part); // WHAT: the leftover. WHY: never a stray part file
                if !path.exists() {
                    return Err(e); // WHAT: nothing kept. WHY: said
                }
            }
        }
        Ok(sha)
    }

    fn blob_path(&self, sha: &str) -> PathBuf {
        self.blobs.join(&sha[0..2]).join(&sha[2..4]).join(sha) // WHAT: ab/cd/abcd…, as the server keeps every file. WHY: one store, one layout, one backup
    }

    fn read_blob(&self, sha: &str) -> Option<Vec<u8>> {
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return None; // WHAT: never a path from anything but a hash. WHY: a damaged row can't reach outside the folder
        }
        let bytes = std::fs::read(self.blob_path(sha)).ok()?;
        (crate::store::hex(&Sha256::digest(&bytes)) == sha.to_ascii_lowercase()).then_some(bytes) // WHAT: only bytes that are what their name says. WHY: a damaged file is never handed out as the drawing
    }

    /// The drawings not archived, by folder then name.
    pub fn drawings(&self) -> Vec<Drawing> {
        let Ok(mut st) = self.db.prepare(
            "SELECT d.id, d.folder, d.name, v.version, v.size, v.saved_by, v.saved_at, d.locked_by, d.locked_at
             FROM cad_drawings d JOIN cad_versions v ON v.drawing = d.id
             WHERE d.archived = 0 AND v.version = (SELECT MAX(version) FROM cad_versions WHERE drawing = d.id)
             ORDER BY d.folder, d.name",
        ) else {
            return Vec::new();
        };
        st.query_map([], row_drawing).map(|it| it.flatten().collect()).unwrap_or_default()
    }

    pub fn drawing(&self, id: i64) -> Option<Drawing> {
        self.db
            .query_row(
                "SELECT d.id, d.folder, d.name, v.version, v.size, v.saved_by, v.saved_at, d.locked_by, d.locked_at
                 FROM cad_drawings d JOIN cad_versions v ON v.drawing = d.id
                 WHERE d.id = ?1 AND d.archived = 0 AND v.version = (SELECT MAX(version) FROM cad_versions WHERE drawing = d.id)",
                [id],
                row_drawing,
            )
            .optional()
            .ok()
            .flatten()
    }

    /// A new drawing, its first version from `bytes`.
    ///
    /// # Errors
    ///
    /// Refused, in words a person can act on, when it can't be done.
    pub fn create(&mut self, folder: &str, name: &str, bytes: &[u8], who: &str) -> Result<i64, Refused> {
        if !good_folder(folder) || !good_name(name) {
            return Err(Refused::Bad("A name is 1 to 120 characters with no slashes; folders are names joined by '/'.".into()));
        }
        if bytes.len() > MOST_BYTES {
            return Err(Refused::Bad("That drawing is too big.".into()));
        }
        let taken: bool = self.db.query_row("SELECT EXISTS(SELECT 1 FROM cad_drawings WHERE folder = ?1 AND name = ?2)", params![folder, name], |r| r.get(0)).unwrap_or(false); // WHAT: the name checked before anything is stored. WHY: a refused create left its file behind for good (audit, 2026-11-11); the index still decides below
        if taken {
            return Err(Refused::Exists);
        }
        let sha = self.put_blob(bytes).map_err(|e| Refused::Bad(format!("Couldn't store it: {e}")))?;
        let tx = self.db.transaction().map_err(|e| Refused::Bad(e.to_string()))?;
        if tx.execute("INSERT INTO cad_drawings (folder, name, created_by, created_at) VALUES (?1, ?2, ?3, ?4)", params![folder, name, who, now()]).is_err() {
            return Err(Refused::Exists); // WHAT: that folder already has that name. WHY: the unique index says so
        }
        let id = tx.last_insert_rowid();
        tx.execute("INSERT INTO cad_versions (drawing, version, sha256, size, saved_by, saved_at) VALUES (?1, 1, ?2, ?3, ?4, ?5)", params![id, sha, bytes.len() as i64, who, now()]).map_err(|e| Refused::Bad(e.to_string()))?;
        tx.commit().map_err(|e| Refused::Bad(e.to_string()))?;
        self.log(who, "create", &format!("{id} {folder}/{name}"));
        Ok(id)
    }

    /// A version's bytes (the newest when `version` is None).
    ///
    /// # Errors
    ///
    /// Refused, in words a person can act on, when it can't be done.
    pub fn file(&self, id: i64, version: Option<i64>) -> Result<(i64, Vec<u8>), Refused> {
        let d = self.drawing(id).ok_or(Refused::NotFound)?;
        let v = version.unwrap_or(d.version);
        let sha: String = self
            .db
            .query_row("SELECT sha256 FROM cad_versions WHERE drawing = ?1 AND version = ?2", params![id, v], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
            .ok_or(Refused::NotFound)?;
        Ok((v, self.read_blob(&sha).ok_or(Refused::NotFound)?))
    }

    /// Check a drawing out to `who` (again, if it's already theirs).
    ///
    /// # Errors
    ///
    /// Refused, in words a person can act on, when it can't be done.
    pub fn lock(&self, id: i64, who: &str) -> Result<(), Refused> {
        let d = self.drawing(id).ok_or(Refused::NotFound)?;
        match (&d.locked_by, d.locked_at) {
            (Some(by), Some(at)) if by != who => Err(Refused::Locked(by.clone(), at)),
            (Some(_), _) => Ok(()),
            _ => {
                let n = self.db.execute("UPDATE cad_drawings SET locked_by = ?2, locked_at = ?3 WHERE id = ?1 AND locked_by IS NULL", params![id, who, now()]).unwrap_or(0); // WHAT: only if still free. WHY: two people asking at once can't both get it
                if n == 0 {
                    let d = self.drawing(id).ok_or(Refused::NotFound)?;
                    return Err(Refused::Locked(d.locked_by.unwrap_or_default(), d.locked_at.unwrap_or(0)));
                }
                self.log(who, "check-out", &id.to_string());
                Ok(())
            }
        }
    }

    /// Check it back in: by the person who has it, or by an admin forcing it.
    ///
    /// # Errors
    ///
    /// Refused, in words a person can act on, when it can't be done.
    pub fn unlock(&self, id: i64, who: &str, force: bool) -> Result<(), Refused> {
        let d = self.drawing(id).ok_or(Refused::NotFound)?;
        match &d.locked_by {
            None => Ok(()),
            Some(by) if by == who || force => {
                self.db.execute("UPDATE cad_drawings SET locked_by = NULL, locked_at = NULL WHERE id = ?1", [id]).map_err(|e| Refused::Bad(e.to_string()))?;
                self.log(who, if by == who { "check-in" } else { "force-release" }, &format!("{id} (had: {by})")); // WHAT: a forced release is named as one. WHY: whoever had it can see who took it back
                Ok(())
            }
            Some(by) => Err(Refused::Locked(by.clone(), d.locked_at.unwrap_or(0))),
        }
    }

    /// A new version, saved by the person who has it out, on top of the
    /// version they opened (`base`); refused if someone saved since.
    ///
    /// # Errors
    ///
    /// Refused, in words a person can act on, when it can't be done.
    pub fn save(&mut self, id: i64, base: i64, bytes: &[u8], who: &str, note: &str) -> Result<i64, Refused> {
        if bytes.len() > MOST_BYTES {
            return Err(Refused::Bad("That drawing is too big.".into()));
        }
        let d = self.drawing(id).ok_or(Refused::NotFound)?; // WHAT: the drawing, looked at before anything is stored. WHY: a save to a drawing that isn't there, or isn't checked out to this person, left its file behind for good (audit, 2026-11-11); the same checks run again inside the lock
        if d.locked_by.as_deref() != Some(who) {
            return match d.locked_by {
                Some(by) => Err(Refused::Locked(by, d.locked_at.unwrap_or(0))),
                None => Err(Refused::NotYours),
            };
        }
        let sha = self.put_blob(bytes).map_err(|e| Refused::Bad(format!("Couldn't store it: {e}")))?; // WHAT: the bytes kept first. WHY: the slow part, outside the write lock
        self.db.execute_batch("BEGIN IMMEDIATE").map_err(|e| Refused::Bad(e.to_string()))?; // WHAT: the check and the write as one. WHY: a check-out forced back and taken by someone else in between can't let this save land
        let saved = self.save_checked(id, base, &sha, bytes.len() as i64, who, note);
        if saved.is_err() {
            let _ = self.db.execute_batch("ROLLBACK"); // WHAT: undone. WHY: a refused save leaves nothing behind
            return saved;
        }
        if let Err(e) = self.db.execute_batch("COMMIT") {
            let _ = self.db.execute_batch("ROLLBACK"); // WHAT: undone. WHY: a save that didn't commit is never reported as saved
            return Err(Refused::Bad(format!("The save didn't finish ({e}). Save again.")));
        }
        saved
    }

    /// [`save`]'s checks and write, inside its transaction.
    fn save_checked(&self, id: i64, base: i64, sha: &str, size: i64, who: &str, note: &str) -> Result<i64, Refused> {
        let d = self.drawing(id).ok_or(Refused::NotFound)?;
        if d.locked_by.as_deref() != Some(who) {
            return match d.locked_by {
                Some(by) => Err(Refused::Locked(by, d.locked_at.unwrap_or(0))),
                None => Err(Refused::NotYours),
            };
        }
        if base != d.version {
            return Err(Refused::Stale(d.version)); // WHAT: saved by someone else since it was opened. WHY: never overwrites their work
        }
        let next = d.version + 1;
        let note: String = note.chars().filter(|c| !c.is_control()).take(500).collect(); // WHAT: the note, cleaned and capped. WHY: it is shown in a list
        self.db
            .execute("INSERT INTO cad_versions (drawing, version, sha256, size, saved_by, saved_at, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![id, next, sha, size, who, now(), note])
            .map_err(|_| Refused::Stale(d.version))?; // WHAT: the version number taken by a save at the same moment. WHY: one of the two is refused, never both written
        self.log(who, "save", &format!("{id} v{next}"));
        Ok(next)
    }

    /// # Errors
    ///
    /// Refused, in words a person can act on, when it can't be done.
    pub fn versions(&self, id: i64) -> Result<Vec<Version>, Refused> {
        self.drawing(id).ok_or(Refused::NotFound)?;
        let mut st = self.db.prepare("SELECT version, size, sha256, saved_by, saved_at, note FROM cad_versions WHERE drawing = ?1 ORDER BY version DESC").map_err(|e| Refused::Bad(e.to_string()))?;
        let rows = st
            .query_map([id], |r| Ok(Version { version: r.get(0)?, size: r.get(1)?, sha256: r.get(2)?, saved_by: r.get(3)?, saved_at: r.get(4)?, note: r.get(5)? }))
            .map_err(|e| Refused::Bad(e.to_string()))?;
        Ok(rows.flatten().collect())
    }

    /// An old version brought back as the newest (by whoever has it out).
    ///
    /// # Errors
    ///
    /// Refused, in words a person can act on, when it can't be done.
    pub fn restore(&mut self, id: i64, version: i64, who: &str) -> Result<i64, Refused> {
        let (_, bytes) = self.file(id, Some(version))?;
        let base = self.drawing(id).ok_or(Refused::NotFound)?.version;
        self.save(id, base, &bytes, who, &format!("Restored version {version}"))
    }

    /// Hidden from the list, every version kept (administrators only).
    ///
    /// # Errors
    ///
    /// Refused, in words a person can act on, when it can't be done.
    pub fn archive(&self, id: i64, who: &str) -> Result<(), Refused> {
        let n = self.db.execute("UPDATE cad_drawings SET archived = 1, locked_by = NULL, locked_at = NULL WHERE id = ?1 AND archived = 0", [id]).unwrap_or(0);
        if n == 0 {
            return Err(Refused::NotFound);
        }
        self.log(who, "archive", &id.to_string());
        Ok(())
    }

    /// Counts: drawings, versions, drawings out.
    pub fn counts(&self) -> (i64, i64, i64) {
        let q = |sql: &str| self.db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
        (
            q("SELECT COUNT(*) FROM cad_drawings WHERE archived = 0"),
            q("SELECT COUNT(*) FROM cad_versions"),
            q("SELECT COUNT(*) FROM cad_drawings WHERE archived = 0 AND locked_by IS NOT NULL"),
        )
    }
}

// ---- moving in from CAD's own server ----------------------------------------

/// A database file's `file:` address, read-only and immutable.
fn url_of(path: &Path) -> Option<String> {
    let full = std::path::absolute(path).ok()?; // WHAT: the full path. WHY: a URI needs one
    let text = full.to_str()?.replace('\\', "/"); // WHAT: forward slashes. WHY: what a file: URI uses on Windows too
    let escaped: String = text.chars().map(|c| match c { '%' => "%25".to_string(), '?' => "%3f".to_string(), '#' => "%23".to_string(), c => c.to_string() }).collect(); // WHAT: the characters a URI reads specially. WHY: a folder named with them still opens
    Some(format!("file:{}{escaped}?mode=ro&immutable=1", if escaped.starts_with('/') { "" } else { "/" })) // WHAT: read-only, immutable. WHY: SQLite then writes nothing beside the old database
}

/// One old drawing file: a plain file, never a link, read no further than
/// the size its version says.
fn read_old_file(path: &Path, size: i64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let kind = std::fs::symlink_metadata(path)?; // WHAT: the entry itself, not what it points at. WHY: a link could point anywhere
    if !kind.file_type().is_file() {
        return Err(std::io::Error::other("it isn't a plain file")); // WHAT: a link, a folder or a device. WHY: never followed
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(size.max(0) as u64 + 1).read_to_end(&mut bytes)?; // WHAT: one byte past its size at most. WHY: a longer file is caught by the size check, never read whole
    Ok(bytes)
}

/// An old server's drawing row, as read: its id, folder, name, who has it out, whether it's archived, and who made it when.
type OldDrawing = (i64, String, String, Option<String>, i64, String, i64);
/// An old server's version row, as read: its drawing, version, fingerprint, size, who saved it when, and the note.
type OldVersion = (i64, i64, String, i64, String, i64, String);

/// What a move brought in.
#[derive(Debug, PartialEq)]
pub struct Moved {
    pub drawings: i64, // WHAT: drawings, archived ones too. WHY: nothing is left behind
    pub versions: i64, // WHAT: every version of each. WHY: the history comes with them
    pub files: i64, // WHAT: drawing files copied. WHY: each checked by its SHA-256
    pub released: i64, // WHAT: drawings that were checked out there. WHY: checked in here, since the accounts are the Office Server's now
}

/// Moves Excalibur CAD's server's drawings (its data folder, `from`) into
/// this server's cad part. Only into an empty one, so nothing here is ever
/// overwritten; the old folder is only read (opened immutable, no links
/// followed), so the old server can be started again if anything is wrong.
/// Every file is checked against its SHA-256 and size before any drawing is
/// written; the drawings and versions go in as one transaction. Stop the old
/// server first, and run it as the account the Office Server runs as (on
/// Windows, an administrator), so the files it copies are the server's.
///
/// # Errors
///
/// Refused, in words a person can act on, when it can't be done.
pub fn move_from(from: &Path, database: &Path, blobs: &Path) -> Result<Moved, String> {
    let old_db = from.join("cad.sqlite");
    if !old_db.is_file() {
        return Err(format!("{} isn't Excalibur CAD's server's data folder (no cad.sqlite in it).", from.display())); // WHAT: the wrong folder. WHY: said plainly, nothing touched
    }
    let old_uri = url_of(&old_db).ok_or_else(|| format!("{} can't be opened as a database.", old_db.display()))?; // WHAT: its address with immutable=1. WHY: opened without writing even SQLite's side files, so the old folder stays exactly as it was
    let old = Connection::open_with_flags(&old_uri, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI).map_err(|e| format!("reading {}: {e}", old_db.display()))?; // WHAT: read only. WHY: the old server's drawings are never changed by the move
    old.execute_batch("BEGIN").map_err(|e| format!("reading {}: {e}", old_db.display()))?; // WHAT: one snapshot for both reads. WHY: drawings and versions from the same moment
    let mut new = Store::open(database, blobs).map_err(|e| format!("opening this server's CAD drawings: {e}"))?;
    let here: i64 = new.db.query_row("SELECT COUNT(*) FROM cad_drawings", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if here > 0 {
        return Err("This server already has CAD drawings; a move only goes into an empty one, so nothing here is overwritten.".into()); // WHAT: refused. WHY: two offices' drawings never mix, and nothing is overwritten
    }
    let old_blobs = from.join("drawings");
    let mut moved = Moved { drawings: 0, versions: 0, files: 0, released: 0 };
    let read_err = |e: rusqlite::Error| format!("reading the old drawings: {e}");
    let mut st = old.prepare("SELECT id, folder, name, locked_by, archived, created_by, created_at FROM drawings ORDER BY id").map_err(read_err)?;
    let drawings: Vec<OldDrawing> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).map_err(read_err)?.collect::<Result<_, _>>().map_err(read_err)?;
    let mut st = old.prepare("SELECT drawing, version, sha256, size, saved_by, saved_at, note FROM versions ORDER BY drawing, version").map_err(read_err)?;
    let versions: Vec<OldVersion> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).map_err(read_err)?.collect::<Result<_, _>>().map_err(read_err)?;
    if let Some((_, folder, name, ..)) = drawings.iter().find(|(_, folder, name, ..)| !good_folder(folder) || !good_name(name)) {
        return Err(format!("the old server has a drawing whose name or folder isn't allowed here ({folder:.40}/{name:.40}); nothing was moved")); // WHAT: refused. WHY: CAD turns names into file names on a computer, so only names this server would take come in
    }
    for (_, _, sha, size, _, _, _) in &versions {
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("a version names a file that isn't a SHA-256 ({sha:.20}…); nothing was moved")); // WHAT: a damaged row. WHY: never becomes a path
        }
        let bytes = read_old_file(&old_blobs.join(&sha[..2]).join(sha), *size).map_err(|e| format!("drawing file {sha:.12}… can't be read from the old folder ({e}); no drawing was moved"))?; // WHAT: the old file, no more than its size. WHY: every version must come, and nothing else
        if crate::store::hex(&Sha256::digest(&bytes)) != sha.to_ascii_lowercase() || bytes.len() as i64 != *size {
            return Err(format!("drawing file {sha:.12}… doesn't match its fingerprint; no drawing was moved")); // WHAT: a damaged file. WHY: never brought in as if it were good
        }
        new.put_blob(&bytes).map_err(|e| format!("copying a drawing file: {e}"))?; // WHAT: copied, written beside then moved into place. WHY: the same way a save writes it
        moved.files += 1;
    }
    let tx = rusqlite::Transaction::new_unchecked(&new.db, rusqlite::TransactionBehavior::Immediate).map_err(|e| e.to_string())?; // WHAT: all rows or none, the write lock held from the start. WHY: a move cut short leaves nothing half there, and nobody adds a drawing between the count and the move (audit, 2026-11-11, as Archetype's move)
    let still_empty: i64 = tx.query_row("SELECT COUNT(*) FROM cad_drawings", [], |r| r.get(0)).map_err(|e| e.to_string())?; // WHAT: counted again inside the lock. WHY: the first count was taken before it
    if still_empty > 0 {
        return Err("A drawing was put on this server while the move ran; nothing was moved. Stop the server, then move again.".into()); // WHAT: refused, inside the lock. WHY: never mixed with drawings made meanwhile
    }
    for (id, folder, name, locked_by, archived, created_by, created_at) in &drawings {
        tx.execute("INSERT INTO cad_drawings (id, folder, name, locked_by, locked_at, archived, created_by, created_at) VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?5, ?6)", params![id, folder, name, archived, created_by, created_at]).map_err(|e| format!("moving drawing {id}: {e}"))?; // WHAT: the same id, folder and name, checked in. WHY: CAD's links keep working; the old sign-in names aren't accounts here
        moved.drawings += 1;
        if locked_by.is_some() {
            moved.released += 1; // WHAT: counted. WHY: the person is told which were out
        }
    }
    for (drawing, version, sha, size, saved_by, saved_at, note) in &versions {
        tx.execute("INSERT INTO cad_versions (drawing, version, sha256, size, saved_by, saved_at, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![drawing, version, sha.to_ascii_lowercase(), size, saved_by, saved_at, note]).map_err(|e| format!("moving a version of drawing {drawing}: {e}"))?; // WHAT: every version, who saved it and when. WHY: the history comes along
        moved.versions += 1;
    }
    tx.execute("INSERT INTO cad_log (at, who, what, detail) VALUES (?1, 'move', 'moved-in', ?2)", params![now(), format!("{} drawings, {} versions from {}", moved.drawings, moved.versions, from.display())]).map_err(|e| e.to_string())?; // WHAT: the move itself, in the log. WHY: where these drawings came from, and when
    tx.commit().map_err(|e| format!("finishing the move: {e}"))?;
    if let Ok(mut st) = old.prepare("SELECT at, who, what, detail FROM log ORDER BY rowid") {
        let rows: Vec<(i64, String, String, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map(|it| it.flatten().collect()).unwrap_or_default(); // WHAT: the old server's log. WHY: who did what before the move
        for (at, who, what, detail) in rows {
            let _ = new.db.execute("INSERT INTO cad_log (at, who, what, detail) VALUES (?1, ?2, ?3, ?4)", params![at, who, what, detail]); // WHAT: kept beside this server's. WHY: the history isn't lost
        }
    }
    Ok(moved)
}

fn row_drawing(r: &rusqlite::Row) -> rusqlite::Result<Drawing> {
    Ok(Drawing { id: r.get(0)?, folder: r.get(1)?, name: r.get(2)?, version: r.get(3)?, size: r.get(4)?, saved_by: r.get(5)?, saved_at: r.get(6)?, locked_by: r.get(7)?, locked_at: r.get(8)? })
}

// ---- the routes -------------------------------------------------------------

/// The part is switched on, or every CAD route answers as if it didn't exist.
fn on(server: &crate::api::Server) -> Answer<()> {
    if crate::office::serves(server, hub::license::Module::Cad) {
        Ok(())
    } else {
        Err(Denied::missing("route")) // WHAT: 404. WHY: a part the licence doesn't switch on isn't there at all
    }
}

/// The store, opened for one request.
fn store(server: &crate::api::Server) -> Answer<Store> {
    Store::open(&server.config.database(), &server.config.blobs()).map_err(|e| Denied::broke("open the CAD drawings", e)) // WHAT: one connection per request. WHY: SQLite with WAL; the locks are in the rows, not the connection
}

fn refused(r: Refused) -> Denied {
    match r {
        Refused::NotFound => Denied::missing("drawing"),
        Refused::Locked(by, _) => Denied::taken(format!("{by} has that drawing checked out.")), // WHAT: who has it. WHY: a person can go and ask them
        Refused::NotYours => Denied::taken("Check the drawing out before saving it."),
        Refused::Stale(v) => Denied::taken(format!("Someone saved version {v} since you opened it. Open it again to see their changes.")),
        Refused::Exists => Denied::taken("A drawing with that name is already in that folder."),
        Refused::Bad(why) => Denied::wrong(why),
    }
}

/// The address's query values, read only once the caller is known: a
/// stranger is answered 401 or 404 whatever they put there.
fn values<T: serde::de::DeserializeOwned>(uri: &Uri) -> Answer<T> {
    Query::<T>::try_from_uri(uri).map(|q| q.0).map_err(|e| Denied::wrong(format!("Those values don't read: {e}"))) // WHAT: parsed here, not by an extractor. WHY: an extractor would answer before the licence and sign-in checks
}

/// The drawing's bytes, read only once the caller is known and no further
/// than [`MOST_BYTES`]: nothing is buffered for a stranger, or on a server
/// without the cad part.
async fn drawing_bytes(body: Body) -> Answer<axum::body::Bytes> {
    axum::body::to_bytes(body, MOST_BYTES).await.map_err(|_| Denied::wrong("That drawing is too big, or it didn't arrive whole.")) // WHAT: the body, capped. WHY: read after `on()` and the door, never before
}

/// Runs a store job off the async threads (drawings can be large).
async fn blocking<T: Send + 'static>(server: &Shared, job: impl FnOnce(&mut Store) -> Result<T, Refused> + Send + 'static) -> Answer<T> {
    let server = server.clone();
    tokio::task::spawn_blocking(move || {
        let mut store = store(&server)?;
        job(&mut store).map_err(refused)
    })
    .await
    .map_err(|e| Denied::broke("finish with the CAD drawing", e))? // WHAT: the job's answer. WHY: a panicked job is a 500, never a hang
}

pub(crate) async fn list(State(server): State<Shared>, headers: HeaderMap) -> Answer<Json<Vec<Drawing>>> {
    on(&server)?;
    crate::api::caller(&server, &headers)?; // WHAT: anybody signed in. WHY: everyone in the office may open the office's drawings
    blocking(&server, |s| Ok(s.drawings())).await.map(Json)
}

#[derive(Deserialize)]
pub struct Place {
    #[serde(default)]
    folder: String,
    #[serde(default)]
    name: String, // WHAT: empty when not given. WHY: refused after the caller is known, so a stranger learns nothing from the answer
}

/// A drawing's id from the address: a number, or no such drawing. Read after
/// the caller is known, so a stranger is told 401 whatever they ask.
fn drawing_id(id: &str) -> Answer<i64> {
    id.parse().map_err(|_| Denied::missing("drawing")) // WHAT: 404 for anything not a number. WHY: the same answer as an id that doesn't exist
}

pub(crate) async fn create(State(server): State<Shared>, headers: HeaderMap, uri: Uri, body: Body) -> Answer<Json<Drawing>> {
    on(&server)?;
    let who = crate::api::writer(&server, &headers)?; // WHAT: someone who may change drawings. WHY: a viewer opens, never adds
    let at: Place = values(&uri)?;
    let _turn = crate::api::big_turn().await; // WHAT: one of four big reads at a time. WHY: parallel uploads could hold gigabytes (audit, 2026-11-11)
    let body = drawing_bytes(body).await?;
    blocking(&server, move |s| {
        let id = s.create(&at.folder, &at.name, &body, &who.email)?;
        s.drawing(id).ok_or(Refused::NotFound)
    })
    .await
    .map(Json)
}

pub(crate) async fn one(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Answer<Json<Drawing>> {
    on(&server)?;
    crate::api::caller(&server, &headers)?;
    let id = drawing_id(&id)?; // WHAT: the drawing's number. WHY: read once the caller is known
    blocking(&server, move |s| s.drawing(id).ok_or(Refused::NotFound)).await.map(Json)
}

#[derive(Deserialize)]
pub struct Which {
    version: Option<i64>,
}

pub(crate) async fn file(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>, uri: Uri) -> Answer<Response> {
    on(&server)?;
    crate::api::caller(&server, &headers)?;
    let which: Which = values(&uri)?;
    let id = drawing_id(&id)?; // WHAT: the drawing's number. WHY: read once the caller is known
    let (version, bytes) = blocking(&server, move |s| s.file(id, which.version)).await?;
    Ok(([(header::CONTENT_TYPE, "application/octet-stream".to_string()), (header::HeaderName::from_static("x-version"), version.to_string())], bytes).into_response()) // WHAT: the bytes, and which version they are (X-Version, as CAD's own server sends it). WHY: the next save names it as its base
}

#[derive(Deserialize)]
pub struct Saving {
    base: Option<i64>, // WHAT: the version it was opened from. WHY: required, and refused after the caller is known
    #[serde(default)]
    note: String,
}

pub(crate) async fn save(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>, uri: Uri, body: Body) -> Answer<Json<Drawing>> {
    on(&server)?;
    let who = crate::api::writer(&server, &headers)?;
    let saving: Saving = values(&uri)?;
    let _turn = crate::api::big_turn().await; // WHAT: one of four big reads at a time. WHY: parallel uploads could hold gigabytes (audit, 2026-11-11)
    let body = drawing_bytes(body).await?;
    let id = drawing_id(&id)?; // WHAT: the drawing's number. WHY: read once the caller is known
    blocking(&server, move |s| {
        s.save(id, saving.base.ok_or_else(|| Refused::Bad("Say which version this was opened from (base).".into()))?, &body, &who.email, &saving.note)?;
        s.drawing(id).ok_or(Refused::NotFound)
    })
    .await
    .map(Json)
}

pub(crate) async fn lock(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Answer<Json<Drawing>> {
    on(&server)?;
    let who = crate::api::writer(&server, &headers)?; // WHAT: checking out is the start of a change. WHY: a viewer can't hold a drawing others need
    let id = drawing_id(&id)?; // WHAT: the drawing's number. WHY: read once the caller is known
    blocking(&server, move |s| {
        s.lock(id, &who.email)?;
        s.drawing(id).ok_or(Refused::NotFound)
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
pub struct Release {
    #[serde(default)]
    force: bool,
}

pub(crate) async fn unlock(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>, uri: Uri) -> Answer<Json<Drawing>> {
    on(&server)?;
    let asked = values::<Release>(&uri); // WHAT: whether this is a forced check-in. WHY: it decides the door; a value that doesn't read is refused after the door
    let who = if asked.as_ref().is_ok_and(|r| r.force) {
        crate::api::administrator(&server, &headers)? // WHAT: forcing someone else's drawing back in. WHY: an administrator's call only
    } else {
        crate::api::writer(&server, &headers)?
    };
    let release = asked?;
    let id = drawing_id(&id)?; // WHAT: the drawing's number. WHY: read once the caller is known
    blocking(&server, move |s| {
        s.unlock(id, &who.email, release.force)?;
        s.drawing(id).ok_or(Refused::NotFound)
    })
    .await
    .map(Json)
}

pub(crate) async fn versions(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Answer<Json<Vec<Version>>> {
    on(&server)?;
    crate::api::caller(&server, &headers)?;
    let id = drawing_id(&id)?; // WHAT: the drawing's number. WHY: read once the caller is known
    blocking(&server, move |s| s.versions(id)).await.map(Json)
}

#[derive(Deserialize)]
pub struct Restoring {
    version: Option<i64>, // WHAT: the version to bring back. WHY: required, and refused after the caller is known
}

pub(crate) async fn restore(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>, uri: Uri) -> Answer<Json<Drawing>> {
    on(&server)?;
    let who = crate::api::writer(&server, &headers)?;
    let restoring: Restoring = values(&uri)?;
    let id = drawing_id(&id)?; // WHAT: the drawing's number. WHY: read once the caller is known
    blocking(&server, move |s| {
        s.restore(id, restoring.version.ok_or_else(|| Refused::Bad("Say which version to bring back.".into()))?, &who.email)?;
        s.drawing(id).ok_or(Refused::NotFound)
    })
    .await
    .map(Json)
}

pub(crate) async fn archive(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Answer<Json<serde_json::Value>> {
    on(&server)?;
    let who = crate::api::administrator(&server, &headers)?; // WHAT: hiding a drawing from everyone. WHY: an administrator's call only
    let id = drawing_id(&id)?; // WHAT: the drawing's number. WHY: read once the caller is known
    blocking(&server, move |s| s.archive(id, &who.email)).await?;
    Ok(Json(serde_json::json!({ "archived": id })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the build's target. Never the shared temp folder.
    fn a_folder(what: &str) -> PathBuf {
        let n: u64 = rand::random();
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("{what}-{n:016x}"))
    }

    /// A server's database and file store, in a fresh folder.
    fn a_server_folder() -> (PathBuf, PathBuf) {
        let dir = a_folder("cad");
        std::fs::create_dir_all(&dir).unwrap();
        (dir.join("hyperview.sqlite"), dir.join("files")) // WHAT: as Config::database and Config::blobs name them. WHY: where the cad part keeps its drawings
    }

    fn a_store() -> Store {
        let (db, blobs) = a_server_folder();
        Store::open(&db, &blobs).unwrap()
    }

    #[test]
    fn one_person_has_a_drawing_out_and_a_save_on_an_old_copy_is_refused() {
        let mut s = a_store();
        let id = s.create("Jobs/2640", "Base plate.excad", b"v1", "sam@example.com").unwrap();
        assert_eq!(s.create("Jobs/2640", "Base plate.excad", b"again", "sam@example.com"), Err(Refused::Exists));
        assert_eq!(s.save(id, 1, b"v2", "sam@example.com", ""), Err(Refused::NotYours), "out first");
        s.lock(id, "sam@example.com").unwrap();
        assert!(matches!(s.lock(id, "pat@example.com"), Err(Refused::Locked(by, _)) if by == "sam@example.com"), "one person at a time");
        assert_eq!(s.save(id, 1, b"v2", "sam@example.com", "first change"), Ok(2));
        assert_eq!(s.save(id, 1, b"v2b", "sam@example.com", ""), Err(Refused::Stale(2)), "an old copy never overwrites a newer one");
        assert_eq!(s.file(id, None).unwrap(), (2, b"v2".to_vec()));
        assert_eq!(s.file(id, Some(1)).unwrap(), (1, b"v1".to_vec()), "every version kept");
        assert_eq!(s.unlock(id, "pat@example.com", false).map_err(|e| matches!(e, Refused::Locked(..))), Err(true), "not theirs to check in");
        s.unlock(id, "pat@example.com", true).unwrap(); // WHAT: forced. WHY: the route allows it to administrators only
        assert_eq!(s.save(id, 2, b"v3", "sam@example.com", ""), Err(Refused::NotYours), "a save after the drawing was taken back never lands");
        assert_eq!(s.restore(id, 1, "pat@example.com"), Err(Refused::NotYours), "restoring is a save, so it needs the drawing out");
        s.archive(id, "admin@example.com").unwrap();
        assert!(s.drawing(id).is_none() && s.drawings().is_empty(), "archived is hidden");
        assert_eq!(s.counts(), (0, 2, 0), "and its versions kept");
    }

    #[test]
    fn a_file_whose_bytes_changed_on_disk_is_never_handed_out() {
        let mut s = a_store();
        let id = s.create("", "Plate.excad", b"the real drawing", "sam@example.com").unwrap();
        let sha = s.versions(id).unwrap()[0].sha256.clone();
        std::fs::write(s.blob_path(&sha), b"something else").unwrap(); // WHAT: the file changed under it. WHY: a damaged disk, or someone in the folder
        assert_eq!(s.file(id, None), Err(Refused::NotFound), "refused, not handed out as the drawing");
    }

    /// An old CAD server's data folder, laid out the way CAD's own server
    /// lays it out: cad.sqlite with its own tables, files in drawings/ab/.
    fn an_old_cad_server() -> PathBuf {
        let dir = a_folder("old-cad");
        std::fs::create_dir_all(dir.join("drawings")).unwrap();
        let db = Connection::open(dir.join("cad.sqlite")).unwrap();
        db.execute_batch(
            "CREATE TABLE users (name TEXT PRIMARY KEY, pass TEXT NOT NULL, admin INTEGER NOT NULL DEFAULT 0, disabled INTEGER NOT NULL DEFAULT 0, created INTEGER NOT NULL);
             CREATE TABLE drawings (id INTEGER PRIMARY KEY, folder TEXT NOT NULL, name TEXT NOT NULL, locked_by TEXT, locked_at INTEGER, archived INTEGER NOT NULL DEFAULT 0, created_by TEXT NOT NULL, created_at INTEGER NOT NULL);
             CREATE TABLE versions (drawing INTEGER NOT NULL REFERENCES drawings(id), version INTEGER NOT NULL, sha256 TEXT NOT NULL, size INTEGER NOT NULL, saved_by TEXT NOT NULL, saved_at INTEGER NOT NULL, note TEXT NOT NULL DEFAULT '', PRIMARY KEY (drawing, version));
             CREATE TABLE log (at INTEGER NOT NULL, who TEXT NOT NULL, what TEXT NOT NULL, detail TEXT NOT NULL);",
        )
        .unwrap(); // WHAT: CAD's server's tables. WHY: what a move reads
        let file = |bytes: &[u8]| {
            let sha = crate::store::hex(&Sha256::digest(bytes));
            std::fs::create_dir_all(dir.join("drawings").join(&sha[..2])).unwrap();
            std::fs::write(dir.join("drawings").join(&sha[..2]).join(&sha), bytes).unwrap(); // WHAT: drawings/ab/abcd…. WHY: CAD's server's layout
            (sha, bytes.len() as i64)
        };
        let ((s1, n1), (s2, n2), (s3, n3)) = (file(b"v1"), file(b"v2"), file(b"old"));
        db.execute("INSERT INTO drawings VALUES (1, 'Jobs/2640', 'Base plate.excad', 'sam', 100, 0, 'sam', 1)", []).unwrap(); // WHAT: checked out to sam. WHY: a lock that must be released by the move
        db.execute("INSERT INTO drawings VALUES (2, '', 'Old.excad', NULL, NULL, 1, 'pat', 1)", []).unwrap(); // WHAT: archived. WHY: moved too, still hidden
        for (d, v, sha, n, note) in [(1, 1, &s1, n1, ""), (1, 2, &s2, n2, "holes"), (2, 1, &s3, n3, "")] {
            db.execute("INSERT INTO versions VALUES (?1, ?2, ?3, ?4, 'sam', 1, ?5)", params![d, v, sha, n, note]).unwrap();
        }
        db.execute("INSERT INTO log VALUES (1, 'sam', 'create', '1')", []).unwrap(); // WHAT: a line of history. WHY: kept beside this server's
        dir
    }

    #[test]
    fn a_cad_server_moves_in_whole_and_its_folder_is_left_as_it_was() {
        let from = an_old_cad_server();
        let before: Vec<(PathBuf, Vec<u8>)> = walk(&from); // WHAT: every file there, with its bytes. WHY: none may change
        let (db, blobs) = a_server_folder();
        let moved = move_from(&from, &db, &blobs).unwrap();
        assert_eq!(moved, Moved { drawings: 2, versions: 3, files: 3, released: 1 });
        let here = Store::open(&db, &blobs).unwrap();
        let listed = here.drawings();
        assert_eq!(listed.len(), 1, "the archived one stays hidden");
        assert_eq!((listed[0].folder.as_str(), listed[0].name.as_str(), listed[0].version, listed[0].locked_by.clone()), ("Jobs/2640", "Base plate.excad", 2, None));
        assert_eq!(here.file(1, Some(1)).unwrap().1, b"v1", "every version came");
        assert_eq!(here.versions(1).unwrap()[0].note, "holes");
        assert_eq!(walk(&from), before, "the old folder is exactly as it was, side files and all");
        assert!(move_from(&from, &db, &blobs).unwrap_err().contains("already has CAD drawings"), "never twice, never over drawings here");
    }

    fn walk(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
        out.sort();
        out
    }

    #[test]
    fn a_damaged_file_a_link_or_a_bad_name_stops_the_move_before_any_drawing_is_moved() {
        let from = an_old_cad_server();
        let sha: String = Connection::open(from.join("cad.sqlite")).unwrap().query_row("SELECT sha256 FROM versions WHERE drawing = 1 AND version = 1", [], |r| r.get(0)).unwrap();
        let path = from.join("drawings").join(&sha[..2]).join(&sha);
        std::fs::write(&path, b"tampered").unwrap(); // WHAT: a file changed on disk. WHY: it must not come in as if it were good
        let (db, blobs) = a_server_folder();
        assert!(move_from(&from, &db, &blobs).unwrap_err().contains("doesn't match its fingerprint"));
        assert!(Store::open(&db, &blobs).unwrap().drawings().is_empty(), "no drawing moved");
        #[cfg(unix)]
        {
            std::fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink("/dev/zero", &path).unwrap(); // WHAT: a link to something endless. WHY: never followed
            assert!(move_from(&from, &db, &blobs).unwrap_err().contains("isn't a plain file"));
        }
        let bad = an_old_cad_server();
        Connection::open(bad.join("cad.sqlite")).unwrap().execute("UPDATE drawings SET name = '../../evil' WHERE id = 1", []).unwrap(); // WHAT: a name this server wouldn't take. WHY: CAD makes file names from them
        let (db, blobs) = a_server_folder();
        assert!(move_from(&bad, &db, &blobs).unwrap_err().contains("isn't allowed here"));
        assert!(move_from(&a_folder("nowhere"), &db, &blobs).unwrap_err().contains("isn't Excalibur CAD's server's data folder"));
    }

    #[test]
    fn names_never_become_paths() {
        let mut s = a_store();
        for bad in ["", "..", "a/b", "a\\b", " lead", "trail ", "nul\0", &"x".repeat(121)] {
            assert!(matches!(s.create("", bad, b"x", "sam@example.com"), Err(Refused::Bad(_))), "{bad:?}");
        }
        assert!(matches!(s.create("a/../b/../../c", "x", b"x", "sam@example.com"), Err(Refused::Bad(_))), "no dot folders");
        assert!(matches!(s.create(&["f"; 9].join("/"), "x", b"x", "sam@example.com"), Err(Refused::Bad(_))), "nine deep is too deep");
        assert!(s.read_blob("../../etc/passwd").is_none(), "a damaged row can't read outside");
    }
}
