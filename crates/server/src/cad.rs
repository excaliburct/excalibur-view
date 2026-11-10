//! The Office Server's `cad` part: Excalibur CAD's office drawings, checked
//! out and in, every version kept.
//!
//! Moved in from Excalibur CAD's own server (its store, nearly line for line),
//! so an office runs one server instead of two. What changed in the move:
//! people and sign-ins are the Office Server's own (one account for View and
//! CAD, the same roles), and every route answers 404 unless the office's
//! signed licence switches the `cad` part on (`crate::office`).
//!
//! One SQLite file and a folder of drawing files, under `cad/` in the data
//! folder. A drawing's bytes are kept once, named by their SHA-256, written
//! beside and then moved into place, so a crash never leaves half a file and
//! the same drawing saved twice takes the room once. Names and folders are
//! only labels in the database: they never become file paths, so no name can
//! reach outside the data folder. Nothing is deleted: an archived drawing is
//! hidden, with all its versions kept.

use std::path::{Path, PathBuf};

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::api::{Answer, Denied, Shared};

/// The longest a drawing may be (a real drawing is far under this).
pub const MOST_BYTES: usize = 256 * 1024 * 1024; // WHAT: 256 MB, CAD's own limit. WHY: the same drawings fit as before the move

/// The CAD part's folder, inside the server's data folder.
pub fn folder(data: &Path) -> PathBuf {
    data.join("cad") // WHAT: data/cad. WHY: apart from View's own files, backed up with them
}

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

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) // WHAT: seconds since 1970. WHY: CAD's own clock, so moved drawings read the same
}

/// A drawing's or folder part's name: 1 to 120 characters, no control
/// characters or slashes, not just dots, no spaces at the ends.
pub fn good_name(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= 120 && s.trim() == s && !s.chars().any(|c| c.is_control() || c == '/' || c == '\\') && !s.chars().all(|c| c == '.') // WHAT: CAD's rule. WHY: a label that can never be mistaken for a path
}

/// A folder: names joined by '/', at most 8 deep; empty is the top.
pub fn good_folder(s: &str) -> bool {
    s.is_empty() || (s.split('/').count() <= 8 && s.split('/').all(good_name)) // WHAT: CAD's rule. WHY: the same
}

impl Store {
    /// The store in `root` (made if new).
    pub fn open(root: &Path) -> rusqlite::Result<Store> {
        let new = !root.exists(); // WHAT: whether this start makes the folder. WHY: only a folder it makes is locked here
        std::fs::create_dir_all(root.join("drawings")).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        #[cfg(unix)]
        if new {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?; // WHAT: the server's alone. WHY: on Windows the service folder's own permissions do this
        }
        let db = Connection::open(root.join("cad.sqlite"))?;
        db.busy_timeout(std::time::Duration::from_secs(10))?; // WHAT: waits for another request's write. WHY: each request opens its own connection
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS drawings (
                 id INTEGER PRIMARY KEY, folder TEXT NOT NULL, name TEXT NOT NULL,
                 locked_by TEXT, locked_at INTEGER, archived INTEGER NOT NULL DEFAULT 0,
                 created_by TEXT NOT NULL, created_at INTEGER NOT NULL);
             CREATE UNIQUE INDEX IF NOT EXISTS drawing_place ON drawings(folder, name) WHERE archived = 0;
             CREATE TABLE IF NOT EXISTS versions (
                 drawing INTEGER NOT NULL REFERENCES drawings(id), version INTEGER NOT NULL,
                 sha256 TEXT NOT NULL, size INTEGER NOT NULL, saved_by TEXT NOT NULL,
                 saved_at INTEGER NOT NULL, note TEXT NOT NULL DEFAULT '',
                 PRIMARY KEY (drawing, version));
             CREATE TABLE IF NOT EXISTS log (
                 at INTEGER NOT NULL, who TEXT NOT NULL, what TEXT NOT NULL, detail TEXT NOT NULL);",
        )?; // WHAT: CAD's own tables, without its people and sign-ins. WHY: the Office Server's accounts are used instead
        Ok(Store { db, blobs: root.join("drawings") })
    }

    pub fn log(&self, who: &str, what: &str, detail: &str) {
        let _ = self.db.execute("INSERT INTO log (at, who, what, detail) VALUES (?1, ?2, ?3, ?4)", params![now(), who, what, detail]); // WHAT: one line of what was done. WHY: who checked out, saved, forced or archived what
    }

    /// A drawing's bytes, kept once by their hash.
    fn put_blob(&self, bytes: &[u8]) -> std::io::Result<String> {
        let sha = crate::store::hex(&Sha256::digest(bytes)); // WHAT: the bytes' SHA-256. WHY: their name, so the same drawing is kept once
        let path = self.blob_path(&sha);
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let part = path.with_extension("part");
            std::fs::write(&part, bytes)?; // WHAT: written beside. WHY: a crash never leaves half a drawing under its real name
            std::fs::rename(&part, &path)?; // WHAT: moved into place. WHY: all at once
        }
        Ok(sha)
    }

    fn blob_path(&self, sha: &str) -> PathBuf {
        self.blobs.join(&sha[..2]).join(sha) // WHAT: drawings/ab/abcd…. WHY: no folder holds too many files
    }

    fn read_blob(&self, sha: &str) -> Option<Vec<u8>> {
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return None; // WHAT: never a path from anything but a hash. WHY: a damaged row can't reach outside the folder
        }
        std::fs::read(self.blob_path(sha)).ok()
    }

    /// The drawings not archived, by folder then name.
    pub fn drawings(&self) -> Vec<Drawing> {
        let Ok(mut st) = self.db.prepare(
            "SELECT d.id, d.folder, d.name, v.version, v.size, v.saved_by, v.saved_at, d.locked_by, d.locked_at
             FROM drawings d JOIN versions v ON v.drawing = d.id
             WHERE d.archived = 0 AND v.version = (SELECT MAX(version) FROM versions WHERE drawing = d.id)
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
                 FROM drawings d JOIN versions v ON v.drawing = d.id
                 WHERE d.id = ?1 AND d.archived = 0 AND v.version = (SELECT MAX(version) FROM versions WHERE drawing = d.id)",
                [id],
                row_drawing,
            )
            .optional()
            .ok()
            .flatten()
    }

    /// A new drawing, its first version from `bytes`.
    pub fn create(&mut self, folder: &str, name: &str, bytes: &[u8], who: &str) -> Result<i64, Refused> {
        if !good_folder(folder) || !good_name(name) {
            return Err(Refused::Bad("A name is 1 to 120 characters with no slashes; folders are names joined by '/'.".into()));
        }
        if bytes.len() > MOST_BYTES {
            return Err(Refused::Bad("That drawing is too big.".into()));
        }
        let sha = self.put_blob(bytes).map_err(|e| Refused::Bad(format!("Couldn't store it: {e}")))?;
        let tx = self.db.transaction().map_err(|e| Refused::Bad(e.to_string()))?;
        if tx.execute("INSERT INTO drawings (folder, name, created_by, created_at) VALUES (?1, ?2, ?3, ?4)", params![folder, name, who, now()]).is_err() {
            return Err(Refused::Exists); // WHAT: that folder already has that name. WHY: the unique index says so
        }
        let id = tx.last_insert_rowid();
        tx.execute("INSERT INTO versions (drawing, version, sha256, size, saved_by, saved_at) VALUES (?1, 1, ?2, ?3, ?4, ?5)", params![id, sha, bytes.len() as i64, who, now()]).map_err(|e| Refused::Bad(e.to_string()))?;
        tx.commit().map_err(|e| Refused::Bad(e.to_string()))?;
        self.log(who, "create", &format!("{id} {folder}/{name}"));
        Ok(id)
    }

    /// A version's bytes (the newest when `version` is None).
    pub fn file(&self, id: i64, version: Option<i64>) -> Result<(i64, Vec<u8>), Refused> {
        let d = self.drawing(id).ok_or(Refused::NotFound)?;
        let v = version.unwrap_or(d.version);
        let sha: String = self
            .db
            .query_row("SELECT sha256 FROM versions WHERE drawing = ?1 AND version = ?2", params![id, v], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
            .ok_or(Refused::NotFound)?;
        Ok((v, self.read_blob(&sha).ok_or(Refused::NotFound)?))
    }

    /// Check a drawing out to `who` (again, if it's already theirs).
    pub fn lock(&self, id: i64, who: &str) -> Result<(), Refused> {
        let d = self.drawing(id).ok_or(Refused::NotFound)?;
        match (&d.locked_by, d.locked_at) {
            (Some(by), Some(at)) if by != who => Err(Refused::Locked(by.clone(), at)),
            (Some(_), _) => Ok(()),
            _ => {
                let n = self.db.execute("UPDATE drawings SET locked_by = ?2, locked_at = ?3 WHERE id = ?1 AND locked_by IS NULL", params![id, who, now()]).unwrap_or(0); // WHAT: only if still free. WHY: two people asking at once can't both get it
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
    pub fn unlock(&self, id: i64, who: &str, force: bool) -> Result<(), Refused> {
        let d = self.drawing(id).ok_or(Refused::NotFound)?;
        match &d.locked_by {
            None => Ok(()),
            Some(by) if by == who || force => {
                self.db.execute("UPDATE drawings SET locked_by = NULL, locked_at = NULL WHERE id = ?1", [id]).map_err(|e| Refused::Bad(e.to_string()))?;
                self.log(who, if by == who { "check-in" } else { "force-release" }, &format!("{id} (had: {by})")); // WHAT: a forced release is named as one. WHY: whoever had it can see who took it back
                Ok(())
            }
            Some(by) => Err(Refused::Locked(by.clone(), d.locked_at.unwrap_or(0))),
        }
    }

    /// A new version, saved by the person who has it out, on top of the
    /// version they opened (`base`); refused if someone saved since.
    pub fn save(&mut self, id: i64, base: i64, bytes: &[u8], who: &str, note: &str) -> Result<i64, Refused> {
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
        if bytes.len() > MOST_BYTES {
            return Err(Refused::Bad("That drawing is too big.".into()));
        }
        let sha = self.put_blob(bytes).map_err(|e| Refused::Bad(format!("Couldn't store it: {e}")))?;
        let next = d.version + 1;
        let note: String = note.chars().filter(|c| !c.is_control()).take(500).collect(); // WHAT: the note, cleaned and capped. WHY: it is shown in a list
        self.db
            .execute("INSERT INTO versions (drawing, version, sha256, size, saved_by, saved_at, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![id, next, sha, bytes.len() as i64, who, now(), note])
            .map_err(|_| Refused::Stale(d.version))?; // WHAT: the version number taken by a save at the same moment. WHY: one of the two is refused, never both written
        self.log(who, "save", &format!("{id} v{next}"));
        Ok(next)
    }

    pub fn versions(&self, id: i64) -> Result<Vec<Version>, Refused> {
        self.drawing(id).ok_or(Refused::NotFound)?;
        let mut st = self.db.prepare("SELECT version, size, sha256, saved_by, saved_at, note FROM versions WHERE drawing = ?1 ORDER BY version DESC").map_err(|e| Refused::Bad(e.to_string()))?;
        let rows = st
            .query_map([id], |r| Ok(Version { version: r.get(0)?, size: r.get(1)?, sha256: r.get(2)?, saved_by: r.get(3)?, saved_at: r.get(4)?, note: r.get(5)? }))
            .map_err(|e| Refused::Bad(e.to_string()))?;
        Ok(rows.flatten().collect())
    }

    /// An old version brought back as the newest (by whoever has it out).
    pub fn restore(&mut self, id: i64, version: i64, who: &str) -> Result<i64, Refused> {
        let (_, bytes) = self.file(id, Some(version))?;
        let base = self.drawing(id).ok_or(Refused::NotFound)?.version;
        self.save(id, base, &bytes, who, &format!("Restored version {version}"))
    }

    /// Hidden from the list, every version kept (administrators only).
    pub fn archive(&self, id: i64, who: &str) -> Result<(), Refused> {
        let n = self.db.execute("UPDATE drawings SET archived = 1, locked_by = NULL, locked_at = NULL WHERE id = ?1 AND archived = 0", [id]).unwrap_or(0);
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
            q("SELECT COUNT(*) FROM drawings WHERE archived = 0"),
            q("SELECT COUNT(*) FROM versions"),
            q("SELECT COUNT(*) FROM drawings WHERE archived = 0 AND locked_by IS NOT NULL"),
        )
    }
}

// ---- moving in from CAD's own server ----------------------------------------

/// What a move brought in.
#[derive(Debug, PartialEq)]
pub struct Moved {
    pub drawings: i64, // WHAT: drawings, archived ones too. WHY: nothing is left behind
    pub versions: i64, // WHAT: every version of each. WHY: the history comes with them
    pub files: i64, // WHAT: drawing files copied. WHY: each checked by its SHA-256
    pub released: i64, // WHAT: drawings that were checked out there. WHY: checked in here, since the accounts are the Office Server's now
}

/// Moves Excalibur CAD's server's drawings (its data folder, `from`) into
/// this server's cad part (`to`). Only into an empty one, so nothing here is
/// ever overwritten; the old folder is only read, never changed, so the old
/// server can be started again if anything is wrong. Every file is checked
/// against its SHA-256 as it is copied.
pub fn move_from(from: &Path, to: &Path) -> Result<Moved, String> {
    let old_db = from.join("cad.sqlite");
    if !old_db.is_file() {
        return Err(format!("{} isn't Excalibur CAD's server's data folder (no cad.sqlite in it).", from.display())); // WHAT: the wrong folder. WHY: said plainly, nothing touched
    }
    let old = Connection::open_with_flags(&old_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| format!("reading {}: {e}", old_db.display()))?; // WHAT: read only. WHY: the old server's drawings are never changed by the move
    let mut new = Store::open(to).map_err(|e| format!("opening this server's CAD drawings: {e}"))?;
    let here: i64 = new.db.query_row("SELECT COUNT(*) FROM drawings", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if here > 0 {
        return Err("This server already has CAD drawings; a move only goes into an empty one, so nothing here is overwritten.".into()); // WHAT: refused. WHY: two offices' drawings never mix, and nothing is overwritten
    }
    let old_blobs = from.join("drawings");
    let mut moved = Moved { drawings: 0, versions: 0, files: 0, released: 0 };
    let read_err = |e: rusqlite::Error| format!("reading the old drawings: {e}");
    let mut st = old.prepare("SELECT id, folder, name, locked_by, archived, created_by, created_at FROM drawings ORDER BY id").map_err(read_err)?;
    let drawings: Vec<(i64, String, String, Option<String>, i64, String, i64)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).map_err(read_err)?.collect::<Result<_, _>>().map_err(read_err)?;
    let mut st = old.prepare("SELECT drawing, version, sha256, size, saved_by, saved_at, note FROM versions ORDER BY drawing, version").map_err(read_err)?;
    let versions: Vec<(i64, i64, String, i64, String, i64, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).map_err(read_err)?.collect::<Result<_, _>>().map_err(read_err)?;
    for (_, _, sha, size, _, _, _) in &versions {
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("a version names a file that isn't a SHA-256 ({sha:.20}…); nothing was moved")); // WHAT: a damaged row. WHY: never becomes a path
        }
        let bytes = std::fs::read(old_blobs.join(&sha[..2]).join(sha)).map_err(|e| format!("drawing file {sha:.12}… is missing from the old folder ({e}); nothing was moved"))?; // WHAT: the old file. WHY: every version must come
        if crate::store::hex(&Sha256::digest(&bytes)) != sha.to_ascii_lowercase() || bytes.len() as i64 != *size {
            return Err(format!("drawing file {sha:.12}… doesn't match its fingerprint; nothing was moved")); // WHAT: a damaged file. WHY: never brought in as if it were good
        }
        new.put_blob(&bytes).map_err(|e| format!("copying a drawing file: {e}"))?; // WHAT: copied, written beside then moved into place. WHY: the same way a save writes it
        moved.files += 1;
    }
    let tx = new.db.transaction().map_err(|e| e.to_string())?; // WHAT: all rows or none. WHY: a move cut short leaves this server's CAD drawings empty, not half there
    for (id, folder, name, locked_by, archived, created_by, created_at) in &drawings {
        tx.execute("INSERT INTO drawings (id, folder, name, locked_by, locked_at, archived, created_by, created_at) VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?5, ?6)", params![id, folder, name, archived, created_by, created_at]).map_err(|e| format!("moving drawing {id}: {e}"))?; // WHAT: the same id, folder and name, checked in. WHY: CAD's links keep working; the old sign-in names aren't accounts here
        moved.drawings += 1;
        if locked_by.is_some() {
            moved.released += 1; // WHAT: counted. WHY: the person is told which were out
        }
    }
    for (drawing, version, sha, size, saved_by, saved_at, note) in &versions {
        tx.execute("INSERT INTO versions (drawing, version, sha256, size, saved_by, saved_at, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![drawing, version, sha.to_ascii_lowercase(), size, saved_by, saved_at, note]).map_err(|e| format!("moving a version of drawing {drawing}: {e}"))?; // WHAT: every version, who saved it and when. WHY: the history comes along
        moved.versions += 1;
    }
    tx.execute("INSERT INTO log (at, who, what, detail) VALUES (?1, 'move', 'moved-in', ?2)", params![now(), format!("{} drawings, {} versions from {}", moved.drawings, moved.versions, from.display())]).map_err(|e| e.to_string())?; // WHAT: the move itself, in the log. WHY: where these drawings came from, and when
    tx.commit().map_err(|e| format!("finishing the move: {e}"))?;
    if let Ok(mut st) = old.prepare("SELECT at, who, what, detail FROM log ORDER BY rowid") {
        let rows: Vec<(i64, String, String, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map(|it| it.flatten().collect()).unwrap_or_default(); // WHAT: the old server's log. WHY: who did what before the move
        for (at, who, what, detail) in rows {
            let _ = new.db.execute("INSERT INTO log (at, who, what, detail) VALUES (?1, ?2, ?3, ?4)", params![at, who, what, detail]); // WHAT: kept beside this server's. WHY: the history isn't lost
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
    Store::open(&folder(&server.config.data)).map_err(|e| Denied::broke("open the CAD drawings", e)) // WHAT: one connection per request. WHY: SQLite with WAL; the locks are in the rows, not the connection
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

pub async fn list(State(server): State<Shared>, headers: HeaderMap) -> Answer<Json<Vec<Drawing>>> {
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

pub async fn create(State(server): State<Shared>, headers: HeaderMap, Query(at): Query<Place>, body: Bytes) -> Answer<Json<Drawing>> {
    on(&server)?;
    let who = crate::api::writer(&server, &headers)?; // WHAT: someone who may change drawings. WHY: a viewer opens, never adds
    blocking(&server, move |s| {
        let id = s.create(&at.folder, &at.name, &body, &who.email)?;
        s.drawing(id).ok_or(Refused::NotFound)
    })
    .await
    .map(Json)
}

pub async fn one(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Answer<Json<Drawing>> {
    on(&server)?;
    crate::api::caller(&server, &headers)?;
    let id = drawing_id(&id)?; // WHAT: the drawing's number. WHY: read once the caller is known
    blocking(&server, move |s| s.drawing(id).ok_or(Refused::NotFound)).await.map(Json)
}

#[derive(Deserialize)]
pub struct Which {
    version: Option<i64>,
}

pub async fn file(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>, Query(which): Query<Which>) -> Answer<Response> {
    on(&server)?;
    crate::api::caller(&server, &headers)?;
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

pub async fn save(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>, Query(saving): Query<Saving>, body: Bytes) -> Answer<Json<Drawing>> {
    on(&server)?;
    let who = crate::api::writer(&server, &headers)?;
    let id = drawing_id(&id)?; // WHAT: the drawing's number. WHY: read once the caller is known
    blocking(&server, move |s| {
        s.save(id, saving.base.ok_or_else(|| Refused::Bad("Say which version this was opened from (base).".into()))?, &body, &who.email, &saving.note)?;
        s.drawing(id).ok_or(Refused::NotFound)
    })
    .await
    .map(Json)
}

pub async fn lock(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Answer<Json<Drawing>> {
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

pub async fn unlock(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>, Query(release): Query<Release>) -> Answer<Json<Drawing>> {
    on(&server)?;
    let who = if release.force {
        crate::api::administrator(&server, &headers)? // WHAT: forcing someone else's drawing back in. WHY: an administrator's call only
    } else {
        crate::api::writer(&server, &headers)?
    };
    let id = drawing_id(&id)?; // WHAT: the drawing's number. WHY: read once the caller is known
    blocking(&server, move |s| {
        s.unlock(id, &who.email, release.force)?;
        s.drawing(id).ok_or(Refused::NotFound)
    })
    .await
    .map(Json)
}

pub async fn versions(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Answer<Json<Vec<Version>>> {
    on(&server)?;
    crate::api::caller(&server, &headers)?;
    let id = drawing_id(&id)?; // WHAT: the drawing's number. WHY: read once the caller is known
    blocking(&server, move |s| s.versions(id)).await.map(Json)
}

#[derive(Deserialize)]
pub struct Restoring {
    version: Option<i64>, // WHAT: the version to bring back. WHY: required, and refused after the caller is known
}

pub async fn restore(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>, Query(restoring): Query<Restoring>) -> Answer<Json<Drawing>> {
    on(&server)?;
    let who = crate::api::writer(&server, &headers)?;
    let id = drawing_id(&id)?; // WHAT: the drawing's number. WHY: read once the caller is known
    blocking(&server, move |s| {
        s.restore(id, restoring.version.ok_or_else(|| Refused::Bad("Say which version to bring back.".into()))?, &who.email)?;
        s.drawing(id).ok_or(Refused::NotFound)
    })
    .await
    .map(Json)
}

pub async fn archive(State(server): State<Shared>, headers: HeaderMap, UrlPath(id): UrlPath<String>) -> Answer<Json<serde_json::Value>> {
    on(&server)?;
    let who = crate::api::administrator(&server, &headers)?; // WHAT: hiding a drawing from everyone. WHY: an administrator's call only
    let id = drawing_id(&id)?; // WHAT: the drawing's number. WHY: read once the caller is known
    blocking(&server, move |s| s.archive(id, &who.email)).await?;
    Ok(Json(serde_json::json!({ "archived": id })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_store() -> Store {
        let n: u64 = rand::random();
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("cad-{n:016x}")); // WHAT: under target. WHY: never the shared temp folder
        Store::open(&dir).unwrap()
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
        assert_eq!(s.restore(id, 1, "pat@example.com"), Err(Refused::NotYours), "restoring is a save, so it needs the drawing out");
        s.archive(id, "admin@example.com").unwrap();
        assert!(s.drawing(id).is_none() && s.drawings().is_empty(), "archived is hidden");
        assert_eq!(s.counts(), (0, 2, 0), "and its versions kept");
    }

    /// An old CAD server's data folder, made the way CAD's server makes it.
    fn an_old_cad_server() -> PathBuf {
        let n: u64 = rand::random();
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("old-cad-{n:016x}"));
        let mut old = Store::open(&dir).unwrap(); // WHAT: the same tables CAD's server has for drawings. WHY: a move reads only those
        old.db.execute_batch("CREATE TABLE users (name TEXT PRIMARY KEY, pass TEXT NOT NULL, admin INTEGER NOT NULL DEFAULT 0, disabled INTEGER NOT NULL DEFAULT 0, created INTEGER NOT NULL);").unwrap(); // WHAT: CAD's own accounts table. WHY: present in the old folder, never moved
        let a = old.create("Jobs/2640", "Base plate.excad", b"v1", "sam").unwrap();
        old.lock(a, "sam").unwrap();
        old.save(a, 1, b"v2", "sam", "holes").unwrap(); // WHAT: two versions, still checked out. WHY: the history and the lock both matter
        let b = old.create("", "Old.excad", b"old", "pat").unwrap();
        old.archive(b, "pat").unwrap(); // WHAT: an archived drawing. WHY: moved too, still hidden
        dir
    }

    #[test]
    fn a_cad_server_moves_in_whole_and_its_folder_is_left_as_it_was() {
        let from = an_old_cad_server();
        let before = std::fs::read(from.join("cad.sqlite")).unwrap();
        let to = a_store_folder();
        let moved = move_from(&from, &to).unwrap();
        assert_eq!(moved, Moved { drawings: 2, versions: 3, files: 3, released: 1 });
        let here = Store::open(&to).unwrap();
        let listed = here.drawings();
        assert_eq!(listed.len(), 1, "the archived one stays hidden");
        assert_eq!((listed[0].folder.as_str(), listed[0].name.as_str(), listed[0].version, listed[0].locked_by.clone()), ("Jobs/2640", "Base plate.excad", 2, None));
        assert_eq!(here.file(1, Some(1)).unwrap().1, b"v1", "every version came");
        assert_eq!(here.versions(1).unwrap()[0].note, "holes");
        assert_eq!(std::fs::read(from.join("cad.sqlite")).unwrap(), before, "the old server's data is only read");
        assert!(move_from(&from, &to).unwrap_err().contains("already has CAD drawings"), "never twice, never over drawings here");
    }

    #[test]
    fn a_damaged_or_missing_file_stops_the_move_before_anything_is_moved() {
        let from = an_old_cad_server();
        let sha = Store::open(&from).unwrap().versions(1).unwrap()[0].sha256.clone();
        std::fs::write(from.join("drawings").join(&sha[..2]).join(&sha), b"tampered").unwrap(); // WHAT: a file changed on disk. WHY: it must not come in as if it were good
        let to = a_store_folder();
        assert!(move_from(&from, &to).unwrap_err().contains("doesn't match its fingerprint"));
        assert!(Store::open(&to).unwrap().drawings().is_empty(), "nothing moved");
        assert!(move_from(&to.join("nowhere"), &a_store_folder()).unwrap_err().contains("isn't Excalibur CAD's server's data folder"));
    }

    fn a_store_folder() -> PathBuf {
        let n: u64 = rand::random();
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("cad-to-{n:016x}"))
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
