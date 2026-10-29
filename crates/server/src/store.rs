//! Where it all lives: a SQLite file and a folder of drawings.
//!
//! SQLite because a six-person fabricator should not have to run a database
//! server, and because a single file is something a person can back up by
//! copying it. It goes a very long way; when a company outgrows it, the
//! schema here is ordinary SQL and moves.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

pub struct Store {
    db: std::sync::Mutex<Connection>,
    blobs: PathBuf,
}

/// The schema. Written out rather than migrated from nothing each time, so
/// what the server expects can be read in one place.
const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS people (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    email       TEXT NOT NULL UNIQUE,
    password    TEXT NOT NULL,
    role        TEXT NOT NULL,
    created     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    token       TEXT PRIMARY KEY,
    person      TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    issued      TEXT NOT NULL,
    expires     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
    id          TEXT PRIMARY KEY,
    number      TEXT NOT NULL,
    name        TEXT NOT NULL,
    created     TEXT NOT NULL
);

-- Who is on a project, when a project has been given a list at all.
--
-- The rule this table exists to make possible: **a project with nobody on it
-- is everybody's**. One shop is one room until somebody says otherwise, which
-- is how every server has worked until now and how most shops want it. The
-- moment one name goes on a project, that project is for the people named on
-- it, and it stops being listed for anybody else.
--
-- Administrators see everything regardless. Somebody has to be able to find a
-- job that the only person on it left the company over.
CREATE TABLE IF NOT EXISTS project_people (
    project     TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    person      TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    added       TEXT NOT NULL,
    added_by    TEXT NOT NULL,
    PRIMARY KEY (project, person)
);

CREATE TABLE IF NOT EXISTS sets (
    id          TEXT PRIMARY KEY,
    project     TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    filename    TEXT NOT NULL,
    digest      TEXT NOT NULL,
    bytes       INTEGER NOT NULL,
    sheets      INTEGER NOT NULL,
    uploaded    TEXT NOT NULL,
    uploaded_by TEXT NOT NULL,
    revision    INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS sheets (
    set_id      TEXT NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    page        INTEGER NOT NULL,
    number      TEXT NOT NULL,
    title       TEXT NOT NULL,
    width       REAL NOT NULL,
    height      REAL NOT NULL,
    rotation    INTEGER NOT NULL,
    scale       TEXT,
    PRIMARY KEY (set_id, page)
);

-- A markup is the annotation dictionary. Everything beside it is derived, and
-- is here so an integration can filter and total without parsing PDF.
CREATE TABLE IF NOT EXISTS markups (
    id          TEXT PRIMARY KEY,
    set_id      TEXT NOT NULL REFERENCES sets(id) ON DELETE CASCADE,
    page        INTEGER NOT NULL,
    subject     TEXT NOT NULL,
    kind        TEXT NOT NULL,
    caption     TEXT NOT NULL,
    author      TEXT NOT NULL,
    colour      TEXT NOT NULL,
    created     TEXT NOT NULL,
    dictionary  BLOB NOT NULL,
    revision    INTEGER NOT NULL,
    removed     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS markups_by_revision ON markups(set_id, revision);

CREATE TABLE IF NOT EXISTS chests (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    filename    TEXT NOT NULL,
    digest      TEXT NOT NULL,
    bytes       INTEGER NOT NULL,
    tools       INTEGER NOT NULL,
    sets        INTEGER NOT NULL,
    uploaded    TEXT NOT NULL,
    shared      INTEGER NOT NULL DEFAULT 1
);

-- Plugins the office hands every seat, one row per plugin id: adding a newer
-- version of one replaces the older. The file itself is a blob.
CREATE TABLE IF NOT EXISTS plugins (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    version     TEXT NOT NULL,
    digest      TEXT NOT NULL,
    bytes       INTEGER NOT NULL,
    key         TEXT NOT NULL,
    uploaded    TEXT NOT NULL,
    uploaded_by TEXT NOT NULL
);

-- Where to tell another program that a drawing set's takeoff changed, and
-- the secret its notices are signed with. See notices.rs.
CREATE TABLE IF NOT EXISTS notices (
    id          TEXT PRIMARY KEY,
    url         TEXT NOT NULL,
    secret      TEXT NOT NULL,
    created     TEXT NOT NULL,
    created_by  TEXT NOT NULL,
    last_status TEXT,
    last_at     TEXT
);

CREATE TABLE IF NOT EXISTS releases (
    version     TEXT NOT NULL,
    channel     TEXT NOT NULL,
    platform    TEXT NOT NULL,
    published   TEXT NOT NULL,
    notes       TEXT NOT NULL,
    bytes       INTEGER NOT NULL,
    digest      TEXT NOT NULL,
    signature   TEXT NOT NULL,
    signing_key TEXT NOT NULL,
    minimum_api INTEGER NOT NULL DEFAULT 1,
    -- A release nobody has approved is not offered to anybody. One seat tries
    -- it first; the other five carry on working.
    ready       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (version, platform)
);

CREATE TABLE IF NOT EXISTS settings (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL
);

-- What happened on this server, and who did it. Kept because an office
-- handling Controlled Unclassified Information is asked, by an auditor, to
-- show exactly this: who reached which drawing, and when.
--
-- Append only. There is no route in the program that edits or deletes a row,
-- which is the property that makes it worth anything. Whoever administers the
-- machine can of course reach the database - that is true of every log there
-- has ever been, and is why an auditor asks who administers the machine.
--
-- `person` is kept as the id and the email as it read at the time, so a row
-- still says who did it after somebody leaves and their account is removed.
-- An invitation: one code, for one person, used once.
--
-- The join code is one server-wide secret that names nobody, never expires
-- and is not used up. An invitation is the opposite of all four: it names who
-- it is for, it carries the role they will get, it stops working on a date,
-- and using it kills it.
--
-- No email is sent, and none ever will be. Sending mail means an SMTP server,
-- a sending reputation, a bounce nobody sees and a support call when a shop's
-- spam filter eats it. An administrator already has a way to reach the person
-- they are inviting -- they work together. What they need is something to
-- send, not something to send it with.
CREATE TABLE IF NOT EXISTS invites (
    code        TEXT PRIMARY KEY,
    email       TEXT NOT NULL,
    name        TEXT NOT NULL,
    role        TEXT NOT NULL,
    made        TEXT NOT NULL,
    made_by     TEXT NOT NULL,
    expires     TEXT NOT NULL,
    used        TEXT,
    used_by     TEXT
);

CREATE TABLE IF NOT EXISTS audit (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    at          TEXT NOT NULL,
    person      TEXT,
    email       TEXT NOT NULL,
    action      TEXT NOT NULL,
    subject     TEXT NOT NULL DEFAULT '',
    detail      TEXT NOT NULL DEFAULT '',
    address     TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS audit_at ON audit (at);
CREATE INDEX IF NOT EXISTS audit_person ON audit (person, at);

-- Groups of people an administrator makes ("Shop", "Field"), and who is in
-- each. A person can be in several. A folder given to groups is seen by the
-- people in them and by the office. See folders.rs.
CREATE TABLE IF NOT EXISTS groups (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    made        TEXT NOT NULL,
    made_by     TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS group_people (
    grp         TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    person      TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
    added       TEXT NOT NULL,
    added_by    TEXT NOT NULL,
    PRIMARY KEY (grp, person)
);

-- A project's folders, written down so an empty one shows, and so each can
-- say who sees it. `key` is the folder as compared (case and spacing aside),
-- `path` as it was first written. `see` is its own rule, NULL when it takes
-- its parent's; a folder with no rule anywhere above it is everybody's, as
-- every folder was before folders had rules. Never removed.
CREATE TABLE IF NOT EXISTS folders (
    project     TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    key         TEXT NOT NULL,
    path        TEXT NOT NULL,
    see         TEXT,
    set_in      TEXT,
    set_by      TEXT,
    set_at      TEXT,
    made        TEXT NOT NULL,
    made_by     TEXT NOT NULL,
    PRIMARY KEY (project, key)
);

-- Files in a project's folders that are not drawing sets: Word, Excel,
-- pictures, models. The bytes are kept once, by their digest, as a drawing's
-- are. One saved again under the same name in the same folder replaces the
-- one before, which is kept as its history.
CREATE TABLE IF NOT EXISTS files (
    id          TEXT PRIMARY KEY,
    project     TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    folder      TEXT,
    name        TEXT NOT NULL,
    digest      TEXT NOT NULL,
    bytes       INTEGER NOT NULL,
    modified    TEXT,
    uploaded    TEXT NOT NULL,
    uploaded_by TEXT NOT NULL,
    replaces    TEXT,
    replaced_by TEXT
);
CREATE INDEX IF NOT EXISTS files_by_project ON files(project);

-- Keys for programs rather than people: an assistant that reads the takeoffs
-- (MCP only, read-only), or another system such as FabWire that uses the API
-- as the person who made the key. Stored as a digest, shown once, never
-- expiring on their own, taken back one at a time.
CREATE TABLE IF NOT EXISTS keys (
    id          TEXT PRIMARY KEY,
    digest      TEXT NOT NULL UNIQUE,
    person      TEXT NOT NULL REFERENCES people(id),
    purpose     TEXT NOT NULL,
    name        TEXT NOT NULL,
    made        TEXT NOT NULL,
    last_used   TEXT
);
"#;

/// An invitation as it is written down.
#[derive(Clone, Debug)]
pub struct Invite {
    pub code: String,
    pub email: String,
    pub name: String,
    pub role: String,
    pub expires: String,
    /// When it was used, if it has been. A used invitation is dead.
    pub used: Option<String>,
}

impl Store {
    pub fn open(database: &Path, blobs: &Path) -> Result<Store> {
        if let Some(parent) = database.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("making {}", parent.display()))?;
        }
        std::fs::create_dir_all(blobs).with_context(|| format!("making {}", blobs.display()))?;
        let db = Connection::open(database)
            .with_context(|| format!("opening {}", database.display()))?;
        db.execute_batch(SCHEMA).context("setting up the database")?;
        upgrade(&db).context("bringing the database up to date")?;
        Ok(Store {
            db: std::sync::Mutex::new(db),
            blobs: blobs.to_path_buf(),
        })
    }

    /// An in-memory store, for tests.
    pub fn temporary(blobs: &Path) -> Result<Store> {
        std::fs::create_dir_all(blobs)?;
        let db = Connection::open_in_memory()?;
        db.execute_batch(SCHEMA)?;
        upgrade(&db)?;
        Ok(Store {
            db: std::sync::Mutex::new(db),
            blobs: blobs.to_path_buf(),
        })
    }

    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let db = self.db.lock().map_err(|_| anyhow!("the database lock was poisoned"))?;
        f(&db)
    }

    // ---- files -----------------------------------------------------------

    /// Where a file with this digest lives. Two levels of directory, because a
    /// hundred thousand files in one folder is slow on every filesystem people
    /// actually run.
    pub fn blob_path(&self, digest: &str) -> PathBuf {
        self.blobs.join(&digest[0..2]).join(&digest[2..4]).join(digest)
    }

    /// Stores bytes under their own digest, and says what that digest is.
    /// Uploading the same drawing twice costs one copy, not two.
    pub fn put_blob(&self, bytes: &[u8]) -> Result<String> {
        let digest = hex(&Sha256::digest(bytes));
        let path = self.blob_path(&digest);
        if path.exists() {
            return Ok(digest);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Written beside and moved into place, so a half-written drawing is
        // never visible under a digest that promises the whole one.
        let temp = path.with_extension("part");
        std::fs::write(&temp, bytes)?;
        std::fs::rename(&temp, &path)?;
        Ok(digest)
    }

    pub fn get_blob(&self, digest: &str) -> Result<Vec<u8>> {
        // A digest is used to build a path, so it gets checked before it is.
        if !is_digest(digest) {
            return Err(anyhow!("that is not a digest"));
        }
        let path = self.blob_path(digest);
        std::fs::read(&path).with_context(|| format!("reading {}", path.display()))
    }

    pub fn blob_exists(&self, digest: &str) -> bool {
        is_digest(digest) && self.blob_path(digest).exists()
    }

    // ---- invitations ------------------------------------------------------

    /// Writes an invitation down. The code is the secret; everything else is
    /// what it turns into.
    pub fn make_invite(
        &self,
        code: &str,
        email: &str,
        name: &str,
        role: &str,
        by: &str,
        expires: &str,
    ) -> Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO invites (code, email, name, role, made, made_by, expires)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![code, email, name, role, crate::audit::now(), by, expires],
            )?;
            Ok(())
        })
    }

    /// An invitation by its code, whatever state it is in. The caller decides
    /// what to do about used or expired, because the answer it gives somebody
    /// guessing has to be the same in every case.
    pub fn invite(&self, code: &str) -> Result<Option<Invite>> {
        self.with(|db| {
            Ok(db
                .query_row(
                    "SELECT code, email, name, role, expires, used FROM invites WHERE code = ?1",
                    params![code],
                    |r| {
                        Ok(Invite {
                            code: r.get(0)?,
                            email: r.get(1)?,
                            name: r.get(2)?,
                            role: r.get(3)?,
                            expires: r.get(4)?,
                            used: r.get(5)?,
                        })
                    },
                )
                .optional()?)
        })
    }

    /// Marks one used. Returns false when somebody got there first, which is
    /// what makes it single-use even if two people press at once.
    pub fn use_invite(&self, code: &str, person: &str) -> Result<bool> {
        self.with(|db| {
            let changed = db.execute(
                "UPDATE invites SET used = ?1, used_by = ?2
                 WHERE code = ?3 AND used IS NULL",
                params![crate::audit::now(), person, code],
            )?;
            Ok(changed == 1)
        })
    }

    /// Every invitation that has not been used, newest first.
    pub fn invites_outstanding(&self) -> Result<Vec<Invite>> {
        self.with(|db| {
            let mut statement = db.prepare(
                "SELECT code, email, name, role, expires, used FROM invites
                 WHERE used IS NULL ORDER BY made DESC",
            )?;
            let rows = statement.query_map([], |r| {
                Ok(Invite {
                    code: r.get(0)?,
                    email: r.get(1)?,
                    name: r.get(2)?,
                    role: r.get(3)?,
                    expires: r.get(4)?,
                    used: r.get(5)?,
                })
            })?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    /// Throws one away before it is used, for an invitation sent to the wrong
    /// person or to somebody who never joined.
    pub fn drop_invite(&self, code: &str) -> Result<()> {
        self.with(|db| {
            db.execute("DELETE FROM invites WHERE code = ?1", params![code])?;
            Ok(())
        })
    }

    // ---- who is on a project ---------------------------------------------

    /// Everybody named on a project. Empty means nobody has been named, which
    /// means it is everybody's.
    pub fn people_on(&self, project: &str) -> Result<Vec<String>> {
        self.with(|db| {
            let mut statement =
                db.prepare("SELECT person FROM project_people WHERE project = ?1 ORDER BY added")?;
            let rows = statement.query_map(params![project], |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    /// Puts somebody on a project. Doing it twice is not an error.
    pub fn put_on_project(&self, project: &str, person: &str, by: &str) -> Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO project_people (project, person, added, added_by)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(project, person) DO NOTHING",
                params![project, person, crate::audit::now(), by],
            )?;
            Ok(())
        })
    }

    /// Takes somebody off. Taking the last person off makes the project
    /// everybody's again, which is the same rule read backwards and is worth
    /// knowing before doing it.
    pub fn take_off_project(&self, project: &str, person: &str) -> Result<()> {
        self.with(|db| {
            db.execute(
                "DELETE FROM project_people WHERE project = ?1 AND person = ?2",
                params![project, person],
            )?;
            Ok(())
        })
    }

    /// Whether this person may see this project.
    ///
    /// The one place the rule lives, so a listing and a direct fetch can
    /// never disagree -- which is the failure that matters, because a project
    /// somebody cannot see in a list but can open by guessing its id is not
    /// access control, it is a tidier list.
    pub fn may_see(&self, project: &str, person: &str, is_admin: bool) -> Result<bool> {
        if is_admin {
            return Ok(true);
        }
        self.with(|db| {
            let named: i64 = db.query_row(
                "SELECT COUNT(*) FROM project_people WHERE project = ?1",
                params![project],
                |r| r.get(0),
            )?;
            if named == 0 {
                return Ok(true);
            }
            let mine: i64 = db.query_row(
                "SELECT COUNT(*) FROM project_people WHERE project = ?1 AND person = ?2",
                params![project, person],
                |r| r.get(0),
            )?;
            Ok(mine > 0)
        })
    }

    // ---- settings --------------------------------------------------------

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.with(|db| {
            Ok(db
                .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?)
        })
    }

    /// Writes one line into the audit log. Never fails the thing it is
    /// recording: a log that can stop somebody opening a drawing is a log
    /// that gets switched off.
    pub fn audit(&self, entry: &crate::audit::Entry) -> Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO audit (at, person, email, action, subject, detail, address)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    entry.at,
                    entry.person,
                    entry.email,
                    entry.action,
                    entry.subject,
                    entry.detail,
                    entry.address
                ],
            )?;
            Ok(())
        })
    }

    /// The log, newest first, for an administrator or an export.
    pub fn audit_since(&self, since: Option<&str>, limit: usize) -> Result<Vec<crate::audit::Entry>> {
        self.with(|db| {
            let mut statement = db.prepare(
                "SELECT at, person, email, action, subject, detail, address
                 FROM audit
                 WHERE (?1 IS NULL OR at >= ?1)
                 ORDER BY at DESC, id DESC
                 LIMIT ?2",
            )?;
            let rows = statement.query_map(params![since, limit as i64], |r| {
                Ok(crate::audit::Entry {
                    at: r.get(0)?,
                    person: r.get(1)?,
                    email: r.get(2)?,
                    action: r.get(3)?,
                    subject: r.get(4)?,
                    detail: r.get(5)?,
                    address: r.get(6)?,
                })
            })?;
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
        })
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.with(|db| {
            db.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
            Ok(())
        })
    }

    /// The next revision for a drawing set, bumped as one step with whatever
    /// is being written into it.
    pub fn bump(db: &Connection, set: &str) -> Result<u64> {
        db.execute("UPDATE sets SET revision = revision + 1 WHERE id = ?1", params![set])?;
        let revision: i64 = db.query_row(
            "SELECT revision FROM sets WHERE id = ?1",
            params![set],
            |r| r.get(0),
        )?;
        Ok(revision as u64)
    }

    pub fn is_empty(&self) -> Result<bool> {
        self.with(|db| {
            let people: i64 = db.query_row("SELECT count(*) FROM people", [], |r| r.get(0))?;
            Ok(people == 0)
        })
    }
}

/// Columns added after the first servers went out.
///
/// `CREATE TABLE IF NOT EXISTS` leaves an existing table exactly as it was, so
/// a server that has been running since 0.5 would never grow a column added
/// to the schema above. Each one is added here, once, if it is not there —
/// nothing is ever dropped or rewritten, and a database that already has them
/// is left alone.
fn upgrade(db: &Connection) -> Result<()> {
    // Who a project belongs to in another program: "fabwire:bid:2630". A
    // program that asks for the same reference twice gets the same project,
    // which is what stops a retried push from making a second one.
    add_column(db, "projects", "reference", "TEXT")?;
    // When a job was put away. Archived is not deleted: its drawings, markups
    // and takeoff stay exactly as they were, and it can be brought back.
    add_column(db, "projects", "archived", "TEXT")?;
    // A drawing re-issued under the same name supersedes the one before it.
    // Both are kept; the newer one is the one that counts.
    add_column(db, "sets", "supersedes", "TEXT")?;
    add_column(db, "sets", "superseded_by", "TEXT")?;
    // A markup brought forward from the revision before this one, so the
    // takeoff on a revised sheet is not lost and is plainly marked as needing
    // a look.
    add_column(db, "markups", "carried_from", "TEXT")?;
    // Where a project sits in the Studio's list (`Bids/2026`, `Active Jobs`),
    // and the folder a drawing came from in the job's own files. Both are the
    // other program's to say; none is where everything was before.
    add_column(db, "projects", "place", "TEXT")?;
    add_column(db, "sets", "folder", "TEXT")?;
    // When a key stops working, for a key given an end. None is never, which
    // is every key made before keys could end, so none of them is broken.
    add_column(db, "keys", "expires", "TEXT")?; // WHAT: the optional end of a key. WHY: a key for a program can be made to run out on its own.
    db.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS projects_by_reference
             ON projects(reference) WHERE reference IS NOT NULL;
         CREATE INDEX IF NOT EXISTS sets_by_project ON sets(project);",
    )?;
    Ok(())
}

fn add_column(db: &Connection, table: &str, column: &str, declaration: &str) -> Result<()> {
    let mut statement = db.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = statement
        .query_map([], |r| r.get::<_, String>(1))?
        .flatten()
        .any(|name| name == column);
    if !exists {
        db.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {declaration}"))?;
    }
    Ok(())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A digest and nothing else: sixty-four lower-case hex characters.
///
/// This is the check that stops a digest arriving from outside being used to
/// walk out of the file store. Anything that is not exactly a digest is not a
/// digest.
pub fn is_digest(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Store, tempdir::Dir) {
        let dir = tempdir::Dir::new();
        let store = Store::temporary(&dir.path().join("files")).unwrap();
        (store, dir)
    }

    #[test]
    fn the_same_file_twice_is_stored_once() {
        let (store, _dir) = store();
        let a = store.put_blob(b"a drawing").unwrap();
        let b = store.put_blob(b"a drawing").unwrap();
        assert_eq!(a, b);
        assert_eq!(store.get_blob(&a).unwrap(), b"a drawing");
    }

    #[test]
    fn a_digest_that_is_really_a_path_is_refused() {
        let (store, _dir) = store();
        for nonsense in [
            "../../../etc/passwd",
            "..",
            "/etc/passwd",
            "AABBCCDD",
            "",
            &"a".repeat(63),
            &format!("{}/", "a".repeat(63)),
        ] {
            assert!(!is_digest(nonsense), "{nonsense} should not pass as a digest");
            assert!(store.get_blob(nonsense).is_err(), "{nonsense} was read anyway");
        }
        assert!(is_digest(&"a".repeat(64)));
        // Upper case is not what we write, so it is not what we accept.
        assert!(!is_digest(&"A".repeat(64)));
    }

    #[test]
    fn a_fresh_server_knows_that_nobody_has_signed_up_yet() {
        let (store, _dir) = store();
        assert!(store.is_empty().unwrap());
    }

    #[test]
    fn a_setting_can_be_written_and_read_back() {
        let (store, _dir) = store();
        assert_eq!(store.setting("pin").unwrap(), None);
        store.set_setting("pin", "1.4.2").unwrap();
        assert_eq!(store.setting("pin").unwrap().as_deref(), Some("1.4.2"));
        store.set_setting("pin", "1.5.0").unwrap();
        assert_eq!(store.setting("pin").unwrap().as_deref(), Some("1.5.0"));
    }
}

/// A scratch directory that cleans up after itself.
#[cfg(test)]
pub mod tempdir {
    use std::path::{Path, PathBuf};

    pub struct Dir(PathBuf);

    impl Dir {
        pub fn new() -> Dir {
            let n: u64 = rand::random();
            let path = std::env::temp_dir().join(format!("hyperview-test-{n:016x}"));
            std::fs::create_dir_all(&path).expect("a scratch directory");
            Dir(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
