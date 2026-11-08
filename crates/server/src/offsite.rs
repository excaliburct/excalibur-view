//! The off-site backup, and the drill that proves it comes back.
//!
//! A copy of the drawings on the same machine is not a backup: a dead disk, a
//! fire or ransomware takes both. This sends a copy somewhere else every
//! night, locked on this machine with a key this machine can never open, and
//! the drill brings it back into a scratch folder and checks it.
//!
//! How it is laid out where it goes (a folder, or a cloud bucket through rclone):
//!
//! - `daily/`, `weekly/`, `monthly/`: one small *catalog* per night, the
//!   database (a consistent copy) and a manifest, packed, gzipped and
//!   encrypted with age to the public key(s) in `age-recipients.txt`. Kept
//!   7 / 4 / 12, counted rather than aged, so a job that stops for a month
//!   never empties the bucket.
//! - `blobs/`: each drawing file, encrypted on its own, sent once and never
//!   deleted by this job. Drawings are kept by the digest of their contents
//!   and never change, so the second night sends only what is new. The object
//!   name is a salted digest, so the bucket doesn't say which files it holds.
//!
//! What it never does: hold the private key (a private key in the recipients
//! file is refused), send anything unencrypted but the small sidecar of sizes
//! and fingerprints, mark a backup that isn't confirmed where it went, or
//! delete anything whose name isn't one of its own.
//!
//! The server runs it itself each night once `HYPERVIEW_OFFSITE_TARGET` and
//! `HYPERVIEW_OFFSITE_WORK` are set (`HYPERVIEW_OFFSITE_AT` says when), and a
//! person can run it now with `hyperview-server --offsite-backup`. The drill is
//! `hyperview-server --offsite-drill --identity KEY --scratch DIR`. The steps
//! are in README.md, under "Off-site backups".

use std::collections::{BTreeMap, BTreeSet}; // WHAT: sorted maps and sets. WHY: the manifest and listings come out in the same order every time.
use std::io::{Read, Write}; // WHAT: byte streams. WHY: the archive is streamed into age, never held in memory.
use std::path::{Path, PathBuf}; // WHAT: file paths. WHY: Windows and Linux both.
use std::process::{Command, Stdio}; // WHAT: running age and rclone. WHY: proven programs, never a shell.

use anyhow::{anyhow, bail, Context, Result}; // WHAT: errors with their reasons. WHY: every failure says why in the log.
use rusqlite::{Connection, OpenFlags}; // WHAT: SQLite. WHY: the consistent copy and the checks.
use serde::{Deserialize, Serialize}; // WHAT: JSON in and out. WHY: the manifest, the sidecar and the markers.
use sha2::{Digest, Sha256}; // WHAT: SHA-256. WHY: fingerprints, and the drawings' own names.

// ---------------------------------------------------------------- what this product backs up

/// One product's shape: what its data folder holds and how to check it.
pub struct Product {
    pub id: &'static str,                                         // WHAT: the short name, in object names. WHY: one bucket can hold several products' folders.
    pub database: &'static str,                                   // WHAT: the SQLite file in the data folder. WHY: copied consistently each night.
    pub blobs: &'static str,                                      // WHAT: the folder of files kept by digest. WHY: sent once each.
    pub counts: &'static [&'static str],                          // WHAT: the tables counted. WHY: the drill compares them with the manifest.
    pub references: fn(&Connection) -> Result<BTreeSet<String>>,  // WHAT: every digest the database points at. WHY: each must be in the backup.
    pub record: fn(&Connection) -> Result<Option<(String, String)>>, // WHAT: one real record and its file's digest. WHY: the drill opens it.
}

/// Excalibur View's shape.
pub const VIEW: Product = Product {
    id: "view",                                                   // WHAT: objects are named view-…. WHY: says whose they are.
    database: "hyperview.sqlite",                                 // WHAT: the store's file. WHY: Config::database.
    blobs: "files",                                               // WHAT: the drawings by digest. WHY: Config::blobs.
    counts: &["people", "projects", "sets", "sheets", "markups", "chests", "folders", "files", "audit"], // WHAT: what the drill counts. WHY: the records a shop would miss.
    references: view_references,                                 // WHAT: the digest columns. WHY: below.
    record: view_record,                                          // WHAT: the newest drawing set. WHY: the record the drill opens.
};

fn view_references(db: &Connection) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new(); // WHAT: the digests. WHY: a set, so a shared file counts once.
    for (table, column) in [("sets", "digest"), ("chests", "digest"), ("plugins", "digest"), ("files", "digest")] { // WHAT: every table holding a file's digest. WHY: releases are left out: a server fetches those again from the feed.
        let sql = format!("SELECT {column} FROM {table}"); // WHAT: the column. WHY: names are constants above, never input.
        let mut stmt = db.prepare(&sql)?; // WHAT: prepare it. WHY: run once per table.
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?; // WHAT: each value. WHY: collected below.
        for d in rows { out.insert(d?); } // WHAT: keep it. WHY: the set of files the database needs.
    }
    Ok(out) // WHAT: all of them. WHY: the backup checks each is on disk and in the bucket.
}

fn view_record(db: &Connection) -> Result<Option<(String, String)>> {
    use rusqlite::OptionalExtension; // WHAT: a query that may find nothing. WHY: an empty server has no set to open.
    Ok(db.query_row("SELECT id, digest FROM sets ORDER BY uploaded DESC, id LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?))).optional()?) // WHAT: the newest set's id and file. WHY: the newest is the one most likely to be missing if anything is.
}

// ---------------------------------------------------------------- names and the keep rule

const KEEP: [(&str, usize); 3] = [("daily", 7), ("weekly", 4), ("monthly", 12)]; // WHAT: how many of each are kept. WHY: Creede's rule, the same as Arc Valley's.
const EXT: &str = ".tar.gz.age"; // WHAT: the catalog's ending. WHY: says what it is: a gzipped tar, encrypted with age.

/// The catalog's name for a moment, in UTC, to the second.
pub fn object_name(product: &str, t: time::OffsetDateTime) -> String {
    let t = t.to_offset(time::UtcOffset::UTC); // WHAT: in UTC. WHY: UTC never repeats an hour, so two runs never share a name.
    format!("{product}-{:04}{:02}{:02}-{:02}{:02}{:02}{EXT}", t.year(), u8::from(t.month()), t.day(), t.hour(), t.minute(), t.second()) // WHAT: product-yyyymmdd-hhmmss. WHY: sorts by date.
}

/// The moment a name stands for, or `None` for anything that isn't exactly one of ours.
pub fn parse_name(product: &str, name: &str) -> Option<time::OffsetDateTime> {
    let rest = name.strip_prefix(product)?.strip_prefix('-')?.strip_suffix(EXT)?; // WHAT: the date part. WHY: a stranger's name comes back None.
    let b = rest.as_bytes(); // WHAT: its bytes. WHY: checked one by one.
    if b.len() != 15 || b[8] != b'-' || !b.iter().enumerate().all(|(i, c)| i == 8 || c.is_ascii_digit()) { return None; } // WHAT: exactly yyyymmdd-hhmmss. WHY: nothing else is ours.
    let n = |a: usize, z: usize| rest[a..z].parse::<u32>().ok(); // WHAT: a number from the name. WHY: each field below.
    let month = time::Month::try_from(n(4, 6)? as u8).ok()?; // WHAT: the month. WHY: a 13th month is refused.
    let date = time::Date::from_calendar_date(n(0, 4)? as i32, month, n(6, 8)? as u8).ok()?; // WHAT: the day. WHY: a 32nd day is refused, not rolled over.
    let t = time::Time::from_hms(n(9, 11)? as u8, n(11, 13)? as u8, n(13, 15)? as u8).ok()?; // WHAT: the time. WHY: the same.
    Some(time::PrimitiveDateTime::new(date, t).assume_utc()) // WHAT: the moment. WHY: names are written in UTC.
}

/// Whether `name` is one of ours and dated no more than a day after `now`. A name from the future is a clock gone wrong or a
/// planted object: seven "2099" names must never push the real catalogs out of the keep, or be the one a drill picks.
pub fn believable(product: &str, name: &str, now: time::OffsetDateTime) -> bool {
    parse_name(product, name).is_some_and(|t| t <= now + time::Duration::days(1)) // WHAT: ours, and not from the future. WHY: as above.
}

/// What to copy to weekly/ and monthly/, and what to delete, once tonight's catalog is safe.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub weekly: bool,                    // WHAT: copy tonight's to weekly/. WHY: the first of its ISO week.
    pub monthly: bool,                   // WHAT: copy tonight's to monthly/. WHY: the first of its month.
    pub deletes: Vec<(String, String)>,  // WHAT: (folder, name) past the keep. WHY: never tonight's, never a stranger's.
}

/// The keep rule, on its own so the tests can attack it.
pub fn plan(product: &str, listing: &BTreeMap<String, Vec<String>>, tonight: &str) -> Result<Plan> {
    let when = parse_name(product, tonight).ok_or_else(|| anyhow!("not a catalog name: {tonight}"))?; // WHAT: tonight's date. WHY: the plan never runs on a name it can't date.
    let ours = |f: &str| -> Vec<String> { listing.get(f).map(|v| v.iter().filter(|n| believable(product, n, when)).cloned().collect()).unwrap_or_default() }; // WHAT: our catalogs in a folder, none dated past tomorrow. WHY: sidecars, strangers and planted far-future names are never counted or deleted.
    let week = |t: time::OffsetDateTime| t.to_iso_week_date(); // WHAT: (year, week, day). WHY: weeks start on Monday and belong to their Thursday's year.
    let weekly = !ours("weekly").iter().filter_map(|n| parse_name(product, n)).any(|t| { let (y, w, _) = week(t); let (y2, w2, _) = week(when); y == y2 && w == w2 }); // WHAT: this week has none yet. WHY: the first good one stands for the week.
    let monthly = !ours("monthly").iter().filter_map(|n| parse_name(product, n)).any(|t| t.year() == when.year() && t.month() == when.month()); // WHAT: this month has none yet. WHY: the same, for the month.
    let mut deletes = Vec::new(); // WHAT: what goes. WHY: filled per folder.
    for (folder, keep) in KEEP { // WHAT: each folder. WHY: each keeps its own count.
        let mut names = ours(folder); // WHAT: our catalogs there. WHY: only ours are pruned.
        let adds = folder == "daily" || (folder == "weekly" && weekly) || (folder == "monthly" && monthly); // WHAT: tonight's lands here. WHY: it counts toward the keep.
        if adds && !names.iter().any(|n| n == tonight) { names.push(tonight.to_string()); } // WHAT: count it. WHY: keeping 7 includes tonight's.
        names.sort(); names.reverse(); // WHAT: newest first. WHY: names sort by date.
        for n in names.into_iter().skip(keep) { if n != tonight { deletes.push((folder.to_string(), n)); } } // WHAT: past the keep, never tonight's. WHY: the copy just made survives whatever the clock says.
    }
    Ok(Plan { weekly, monthly, deletes }) // WHAT: the plan. WHY: the job carries it out.
}

// ---------------------------------------------------------------- the public half only

/// The first file in the work folder holding an age private key, if any. `bin` (age's own program names the format) and
/// the job's logs are skipped, and so is anything over 64 KB: a key file is a few lines.
pub fn private_key_in(work: &Path) -> Option<PathBuf> {
    let mut stack = vec![work.to_path_buf()]; // WHAT: folders still to look in. WHY: the whole work folder, not only the recipients file.
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue }; // WHAT: its entries. WHY: an unreadable folder holds nothing this can see.
        for e in rd.flatten() {
            let (path, name) = (e.path(), e.file_name().to_string_lossy().into_owned()); // WHAT: the entry. WHY: judged below.
            let Ok(m) = std::fs::symlink_metadata(&path) else { continue }; // WHAT: the entry itself, never what a link points at. WHY: a link can't send the search outside.
            if m.is_dir() { if !(dir == work && name.eq_ignore_ascii_case("bin")) { stack.push(path); } continue; } // WHAT: look inside folders but bin. WHY: age.exe holds the words of its own format.
            if !m.is_file() || m.len() > 64 * 1024 || name.starts_with("offsite.log") { continue; } // WHAT: small plain files only, never the log. WHY: a key file is small, and the log may quote a refusal.
            if std::fs::read(&path).is_ok_and(|b| String::from_utf8_lossy(&b).to_ascii_uppercase().contains(&["AGE", "SECRET", "KEY", ""].join("-"))) { return Some(path); } // WHAT: a private key's line. WHY: it would open every backup, and this machine is what they must survive.
        }
    }
    None
}

/// Reads the recipients file. Only age public keys (age1…) may be in it.
pub fn check_recipients(text: &str) -> Result<usize> {
    if text.to_ascii_uppercase().contains("AGE-SECRET-KEY-") { bail!("the recipients file holds a PRIVATE key (AGE-SECRET-KEY-…). Only the public line (age1…) belongs on this server. Remove it now, and keep the private key with Creede, off this machine."); } // WHAT: refuse a private key. WHY: the key that opens every backup must never sit on the machine they are meant to survive.
    let keys: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect(); // WHAT: the key lines. WHY: age-keygen's comments are allowed.
    const CH: &str = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"; // WHAT: the letters an age key uses (bech32). WHY: anything else is a typo.
    if let Some(bad) = keys.iter().find(|k| !(k.len() == 62 && k.starts_with("age1") && k[4..].chars().all(|c| CH.contains(c)))) { bail!("the recipients file has a line that is not an age public key (age1 and 58 letters and digits): {}…", &bad.chars().take(12).collect::<String>()); } // WHAT: refuse a bad line, naming only its start. WHY: a typo would fail at night, or encrypt to the wrong thing.
    if keys.is_empty() { bail!("the recipients file has no age public key in it (a line starting age1)."); } // WHAT: refuse an empty file. WHY: someone must be able to open the backup.
    Ok(keys.len()) // WHAT: how many keys can open it. WHY: said in the log.
}

// ---------------------------------------------------------------- where it goes

/// Where backups go: a folder (a USB disk, a NAS, or the tests), or an rclone remote (R2, Backblaze B2).
pub enum Target {
    Folder(PathBuf),                                         // WHAT: a plain folder. WHY: tests, and a second disk.
    Rclone { exe: PathBuf, config: PathBuf, remote: String }, // WHAT: a cloud bucket through rclone. WHY: rclone speaks R2 and B2, and checks every upload.
}

#[cfg(test)]
thread_local! {
    /// The tests' bucket that says yes and keeps nothing: uploads under this folder are silently lost.
    pub(crate) static LOSE_UPLOADS: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; // WHAT: a folder whose uploads vanish. WHY: proves the read-back, not the upload's answer, decides what is marked.
}

fn key_ok(key: &str) -> bool {
    !key.is_empty() && key.split('/').all(|p| !p.is_empty() && p != "." && p != ".." && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))) // WHAT: folder/name of plain letters only. WHY: a name can never climb out of the target.
}

impl Target {
    /// `rclone:REMOTE:bucket/folder` for rclone (its program and settings in WORK), anything else a folder.
    pub fn parse(spec: &str, work: &Path) -> Result<Target> {
        if let Some(remote) = spec.strip_prefix("rclone:") { // WHAT: an rclone remote. WHY: the cloud.
            if !remote.contains(':') || remote.contains("..") { bail!("an rclone target looks like rclone:r2:bucket/folder, not {spec}"); } // WHAT: refuse a malformed remote. WHY: say so before anything runs.
            if !remote.starts_with(|c: char| c.is_ascii_alphanumeric()) || remote.chars().any(|c| c.is_whitespace() || c.is_control()) { bail!("an rclone remote starts with a letter or digit and has no spaces, not {spec}"); } // WHAT: refuse a remote rclone would read as an option. WHY: rclone:--config=... must never change how rclone runs.
            if remote.contains('=') || remote.contains(',') { bail!("an rclone target names a remote from rclone.conf; settings written into it (with = or ,) are refused"); } // WHAT: refuse inline settings. WHY: a key written into the target would be shown in the log and the ops report as where the backup went.
            let exe = work.join("bin").join(if cfg!(windows) { "rclone.exe" } else { "rclone" }); // WHAT: rclone beside the job. WHY: never whatever is on the PATH.
            return Ok(Target::Rclone { exe, config: work.join("rclone.conf"), remote: remote.trim_end_matches('/').to_string() }); // WHAT: the remote and its settings file. WHY: Creede makes rclone.conf on the box; this program never reads it, rclone does.
        }
        if spec.trim().is_empty() { bail!("no target given"); } // WHAT: refuse nothing. WHY: a backup must go somewhere.
        Ok(Target::Folder(PathBuf::from(spec))) // WHAT: a folder. WHY: the tests, a USB disk or a NAS.
    }

    /// Where it went, in words, for the ops report. No keys, no account numbers.
    pub fn words(&self) -> String {
        match self { Target::Folder(p) => format!("folder {}", p.display()), Target::Rclone { remote, .. } => format!("off-site bucket {remote}") } // WHAT: a short description. WHY: Fleet and the log say where.
    }

    fn rclone(&self, args: &[&str]) -> Result<String> {
        let Target::Rclone { exe, config, .. } = self else { bail!("not an rclone target") }; // WHAT: only for rclone. WHY: a folder needs none of this.
        let out = Command::new(exe).arg("--config").arg(config).args(args).stdin(Stdio::null()).output().with_context(|| format!("running {}", exe.display()))?; // WHAT: run rclone with this job's settings. WHY: the service account has no rclone settings of its own.
        if !out.status.success() { bail!("rclone {} failed: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).lines().rev().take(3).collect::<Vec<_>>().join(" | ")); } // WHAT: its last lines on failure. WHY: the reason, not just a code.
        Ok(String::from_utf8_lossy(&out.stdout).into_owned()) // WHAT: its output. WHY: listings are JSON.
    }

    fn remote(&self, key: &str) -> Result<String> {
        if !key_ok(key) { bail!("refusing an object name: {key}"); } // WHAT: check the name. WHY: never a path outside our folders.
        match self { Target::Rclone { remote, .. } => Ok(format!("{remote}/{key}")), Target::Folder(_) => bail!("not an rclone target") } // WHAT: the full remote path. WHY: rclone's form.
    }

    fn folder_path(&self, key: &str) -> Result<PathBuf> {
        if !key_ok(key) { bail!("refusing an object name: {key}"); } // WHAT: check the name. WHY: never a path outside the folder.
        match self { Target::Folder(root) => Ok(key.split('/').fold(root.clone(), |p, s| p.join(s))), Target::Rclone { .. } => bail!("not a folder target") } // WHAT: the file's path. WHY: one folder per part.
    }

    /// Sends one file, never over an object that is already there.
    pub fn put(&self, local: &Path, key: &str) -> Result<()> {
        match self {
            Target::Folder(_) => {
                let dst = self.folder_path(key)?; // WHAT: where it lands. WHY: checked above.
                if dst.exists() { bail!("{key} is already there; a backup is never written over"); } // WHAT: refuse an overwrite. WHY: what was sent stays as it was sent.
                #[cfg(test)]
                if LOSE_UPLOADS.with(|l| l.get()).is_some_and(|f| key.starts_with(f)) { return Ok(()); } // WHAT: the tests' lossy bucket says yes and keeps nothing. WHY: the read-back must catch it.
                if let Some(p) = dst.parent() { std::fs::create_dir_all(p)?; } // WHAT: its folder. WHY: the first one makes it.
                let part = dst.with_extension("part"); // WHAT: a temporary name. WHY: a half copy is never taken for a whole one.
                std::fs::copy(local, &part).with_context(|| format!("copying to {}", part.display()))?; // WHAT: copy it. WHY: the upload.
                std::fs::File::open(&part)?.sync_all()?; // WHAT: onto the disk. WHY: a power cut must not leave a short file under a good name.
                std::fs::rename(&part, &dst)?; // WHAT: the real name. WHY: only now is it whole.
                Ok(())
            }
            Target::Rclone { .. } => { let r = self.remote(key)?; self.rclone(&["copyto", &local.to_string_lossy(), &r, "--s3-no-check-bucket", "--retries", "5", "--low-level-retries", "20"]).map(|_| ()) } // WHAT: rclone sends it. WHY: it checks the bytes arrived and retries a dropped connection.
        }
    }

    /// Sends every file in a local folder into a folder of the target.
    pub fn put_all(&self, local_dir: &Path, folder: &str) -> Result<()> {
        match self {
            Target::Folder(_) => { for e in std::fs::read_dir(local_dir)? { let e = e?; self.put(&e.path(), &format!("{folder}/{}", e.file_name().to_string_lossy()))?; } Ok(()) } // WHAT: one by one. WHY: a folder is quick.
            Target::Rclone { .. } => { let r = self.remote(folder)?; self.rclone(&["copy", &local_dir.to_string_lossy(), &r, "--s3-no-check-bucket", "--ignore-existing", "--retries", "5", "--low-level-retries", "20", "--transfers", "4"]).map(|_| ()) } // WHAT: one rclone run for the lot. WHY: thousands of drawings would be thousands of starts; never over an existing object.
        }
    }

    /// The objects in a folder: name and size.
    pub fn list(&self, folder: &str) -> Result<Vec<(String, u64)>> {
        match self {
            Target::Folder(_) => {
                let dir = self.folder_path(folder)?; // WHAT: the folder. WHY: checked.
                if !dir.exists() { return Ok(Vec::new()); } // WHAT: none yet. WHY: the first night.
                let mut out = Vec::new(); // WHAT: the list. WHY: filled below.
                for e in std::fs::read_dir(dir)? { let e = e?; let m = e.metadata()?; if m.is_file() { out.push((e.file_name().to_string_lossy().into_owned(), m.len())); } } // WHAT: each file. WHY: folders inside are not ours.
                Ok(out)
            }
            Target::Rclone { .. } => {
                #[derive(Deserialize)]
                struct Item { #[serde(rename = "Name")] name: String, #[serde(rename = "Size")] size: i64 } // WHAT: rclone's listing. WHY: only the two fields used.
                let r = self.remote(folder)?; // WHAT: the remote folder. WHY: checked.
                let text = self.rclone(&["lsjson", &r, "--files-only"])?; // WHAT: list it. WHY: what is really there.
                let items: Vec<Item> = serde_json::from_str(if text.trim().is_empty() { "[]" } else { &text })?; // WHAT: read the JSON. WHY: an empty answer is no objects.
                Ok(items.into_iter().map(|i| (i.name, i.size.max(0) as u64)).collect()) // WHAT: name and size. WHY: the size confirms an upload.
            }
        }
    }

    /// A copy inside the target (weekly/, monthly/). Nothing is sent again.
    pub fn copy(&self, from: &str, to: &str) -> Result<()> {
        match self {
            Target::Folder(_) => { let src = self.folder_path(from)?; self.put(&src, to) } // WHAT: a file copy. WHY: the same rules as a put.
            Target::Rclone { .. } => { let (a, b) = (self.remote(from)?, self.remote(to)?); self.rclone(&["copyto", &a, &b, "--s3-no-check-bucket"]).map(|_| ()) } // WHAT: a copy on the server side. WHY: nothing leaves the box again.
        }
    }

    /// Deletes one object. Only the keep rule calls this, only for our own names.
    pub fn delete(&self, key: &str) -> Result<()> {
        match self {
            Target::Folder(_) => { let p = self.folder_path(key)?; std::fs::remove_file(&p).with_context(|| format!("removing {}", p.display())) } // WHAT: remove the file. WHY: the keep rule.
            Target::Rclone { .. } => { let r = self.remote(key)?; self.rclone(&["deletefile", &r]).map(|_| ()) } // WHAT: delete it. WHY: a bucket lock may refuse it, and that is the lock working.
        }
    }

    /// Fetches one object to a local file.
    pub fn get(&self, key: &str, local: &Path) -> Result<()> {
        match self {
            Target::Folder(_) => { let p = self.folder_path(key)?; std::fs::copy(&p, local).with_context(|| format!("reading {}", p.display())).map(|_| ()) } // WHAT: copy it here. WHY: the drill reads a local copy.
            Target::Rclone { .. } => { let r = self.remote(key)?; self.rclone(&["copyto", &r, &local.to_string_lossy()]).map(|_| ()) } // WHAT: download it. WHY: the same.
        }
    }

    /// Fetches many objects of one folder into a local folder.
    pub fn get_many(&self, folder: &str, names: &[String], local_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(local_dir)?; // WHAT: the folder. WHY: they land there.
        match self {
            Target::Folder(_) => { for n in names { self.get(&format!("{folder}/{n}"), &local_dir.join(n))?; } Ok(()) } // WHAT: one by one. WHY: a folder is quick.
            Target::Rclone { .. } => {
                if names.iter().any(|n| !key_ok(n) || n.contains('/')) { bail!("refusing an object name in the list"); } // WHAT: check every name. WHY: the list is a file rclone reads.
                let list = local_dir.join(".wanted.txt"); // WHAT: the names, one per line. WHY: rclone's --files-from.
                std::fs::write(&list, names.join("\n"))?; // WHAT: write it. WHY: one rclone run for the lot.
                let r = self.remote(folder)?; // WHAT: the remote folder. WHY: checked.
                let res = self.rclone(&["copy", &r, &local_dir.to_string_lossy(), "--files-from", &list.to_string_lossy(), "--retries", "5", "--transfers", "4"]); // WHAT: download them. WHY: many starts would be slow.
                let _ = std::fs::remove_file(&list); // WHAT: remove the list. WHY: tidy.
                res.map(|_| ())
            }
        }
    }
}

// ---------------------------------------------------------------- locking it: age

/// The program that encrypts and decrypts. Only age in use; the tests have a stand-in.
pub enum Cipher {
    Age(PathBuf), // WHAT: the age program. WHY: a small, audited tool whose public key can lock and never unlock.
    #[cfg(test)]
    Fake,         // WHAT: the tests' stand-in. WHY: unit tests run on machines without age; real age is tested too where it is installed.
}

impl Cipher {
    /// Encrypts what `fill` writes, to the recipients file, into `out`. Nothing unencrypted touches the disk.
    pub fn encrypt_stream(&self, recipients: &Path, out: &Path, fill: impl FnOnce(&mut dyn Write) -> Result<()>) -> Result<()> {
        match self {
            Cipher::Age(exe) => {
                let mut child = Command::new(exe).arg("-R").arg(recipients).arg("-o").arg(out).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().with_context(|| format!("starting {}", exe.display()))?; // WHAT: age reads the stream and writes the encrypted file. WHY: the server locks with the public key only.
                let mut sin = child.stdin.take().ok_or_else(|| anyhow!("age has no input"))?; // WHAT: age's input. WHY: the archive is written into it.
                let filled = fill(&mut sin); // WHAT: write everything. WHY: one stream from the files to age.
                drop(sin); // WHAT: close age's input. WHY: age writes the end of the file when its input ends.
                let done = child.wait_with_output()?; // WHAT: wait for age. WHY: its result decides.
                filled?; // WHAT: a failure while writing. WHY: a half archive is never kept.
                if !done.status.success() { bail!("age failed: {}", String::from_utf8_lossy(&done.stderr).trim()); } // WHAT: age's own failure. WHY: say why.
                Ok(())
            }
            #[cfg(test)]
            Cipher::Fake => { let keys = std::fs::read_to_string(recipients)?; let mut buf = Vec::new(); fill(&mut buf)?; std::fs::write(out, fake::seal(&keys, &buf)) .map_err(Into::into) } // WHAT: the stand-in. WHY: the same shape as age's output.
        }
    }

    /// Encrypts one file into another. Returns the SHA-256 of the bytes that went in, as they went.
    pub fn encrypt_file(&self, recipients: &Path, input: &Path, out: &Path) -> Result<String> {
        let mut h = Sha256::new(); // WHAT: the fingerprint of what is sent. WHY: a file changed between its check and its sending is caught.
        self.encrypt_stream(recipients, out, |w| { let mut f = std::fs::File::open(input)?; let mut buf = vec![0u8; 1 << 20]; loop { let n = f.read(&mut buf)?; if n == 0 { break; } h.update(&buf[..n]); w.write_all(&buf[..n])?; } Ok(()) })?; // WHAT: stream the file through, fingerprinting it. WHY: a big drawing is never held in memory.
        Ok(hex(&h.finalize())) // WHAT: the fingerprint. WHY: compared with the file's name.
    }

    /// Opens a passphrase-locked file (Creede's locked key): age asks for the passphrase on the console itself.
    pub fn unlock(&self, input: &Path, out: &Path) -> Result<()> {
        match self {
            Cipher::Age(exe) => {
                let file = create_private(out).with_context(|| format!("making {}", out.display()))?; // WHAT: the opened key's file, owner-only before a byte is in it. WHY: it opens every backup.
                let done = Command::new(exe).arg("-d").arg(input).stdin(Stdio::inherit()).stdout(file).stderr(Stdio::inherit()).status(); // WHAT: age prompts for the passphrase and writes into that file. WHY: this program never sees the passphrase, and age never makes the file itself.
                let done = match done { Ok(d) => d, Err(e) => { let _ = std::fs::remove_file(out); return Err(anyhow!("starting {}: {e}", exe.display())); } }; // WHAT: age would not start. WHY: leave nothing behind.
                if !done.success() { let _ = std::fs::remove_file(out); bail!("age could not open the key file: wrong passphrase?"); } // WHAT: refuse, leaving nothing. WHY: nothing else can run.
                Ok(())
            }
            #[cfg(test)]
            Cipher::Fake => { let b = std::fs::read(input)?; let inner = b.strip_prefix(b"FAKEPASS1\n").filter(|i| i.starts_with(b"# public key:")).ok_or_else(|| anyhow!("age could not open the key file: wrong passphrase?"))?; create_private(out)?.write_all(inner).map_err(Into::into) } // WHAT: the stand-in's locked file. WHY: tests the one-prompt path.
        }
    }

    /// Decrypts with Creede's private key (an unlocked key: see `Drill::run`, which opens a locked one once).
    pub fn decrypt_file(&self, identity: &Path, input: &Path, out: &Path) -> Result<()> {
        match self {
            Cipher::Age(exe) => {
                let file = create_private(out).with_context(|| format!("making {}", out.display()))?; // WHAT: the output, owner-only before a byte is in it. WHY: a restored file is as private as the original.
                let done = Command::new(exe).arg("-d").arg("-i").arg(identity).arg(input).stdin(Stdio::inherit()).stdout(file).stderr(Stdio::piped()).output(); // WHAT: age decrypts into that file. WHY: age never makes the file itself, so it is never open to others.
                let done = match done { Ok(d) => d, Err(e) => { let _ = std::fs::remove_file(out); return Err(anyhow!("starting {}: {e}", exe.display())); } }; // WHAT: age would not start. WHY: leave nothing behind.
                if !done.status.success() { let _ = std::fs::remove_file(out); bail!("could not decrypt (is this the right key?): {}", String::from_utf8_lossy(&done.stderr).trim()); } // WHAT: refuse, leaving nothing. WHY: the wrong key opens nothing.
                Ok(())
            }
            #[cfg(test)]
            Cipher::Fake => { let id = std::fs::read_to_string(identity)?; let data = std::fs::read(input)?; let plain = fake::open(&id, &data).ok_or_else(|| anyhow!("could not decrypt (is this the right key?)"))?; create_private(out)?.write_all(&plain).map_err(Into::into) } // WHAT: the stand-in. WHY: the wrong key must fail here too.
        }
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! The tests' stand-in for age: "FAKEAGE1", the recipients, then the bytes scrambled with a key from them. Not
    //! encryption; it only lets the tests check what this module does with an encrypted stream on machines without age.
    use sha2::{Digest, Sha256}; // WHAT: a key stream. WHY: scrambles the bytes so the tests can see nothing plain leaks.
    fn stream(keys: &str, n: usize) -> Vec<u8> { let mut out = Vec::with_capacity(n + 32); let mut c = 0u64; while out.len() < n { out.extend_from_slice(&Sha256::new().chain_update(keys.as_bytes()).chain_update(c.to_le_bytes()).finalize()); c += 1; } out.truncate(n); out } // WHAT: bytes from the keys. WHY: the same keys undo it.
    fn recips(text: &str) -> String { text.lines().map(str::trim).filter(|l| l.starts_with("age1")).collect::<Vec<_>>().join(",") } // WHAT: the public keys, in order. WHY: the "lock".
    pub fn seal(keys: &str, data: &[u8]) -> Vec<u8> { let k = recips(keys); let mut out = format!("FAKEAGE1\n{k}\n").into_bytes(); out.extend(data.iter().zip(stream(&k, data.len())).map(|(a, b)| a ^ b)); out } // WHAT: the stand-in's output. WHY: the header says who may open it.
    pub fn open(identity: &str, data: &[u8]) -> Option<Vec<u8>> { let pubkey = identity.lines().find_map(|l| l.trim().strip_prefix("# public key: "))?.trim().to_string(); let rest = data.strip_prefix(b"FAKEAGE1\n")?; let nl = rest.iter().position(|&b| b == b'\n')?; let k = std::str::from_utf8(&rest[..nl]).ok()?; if !k.split(',').any(|x| x == pubkey) { return None; } Some(rest[nl + 1..].iter().zip(stream(k, rest.len() - nl - 1)).map(|(a, b)| a ^ b).collect()) } // WHAT: undo it, only for a listed key. WHY: the wrong key must fail.
}

// ---------------------------------------------------------------- the archive: plain tar, written and read by hand

/// Writes a plain ustar archive: regular files only, short plain names.
struct TarOut<'a> { w: &'a mut dyn Write } // WHAT: the stream it writes into. WHY: gzip, then age.

impl TarOut<'_> {
    fn file(&mut self, name: &str, size: u64, body: &mut dyn Read) -> Result<()> {
        if name.len() > 99 || !key_ok(name) { bail!("an archive name must be short and plain: {name}"); } // WHAT: the names we write. WHY: the reader refuses anything else.
        if size >= 0o77777777777 { bail!("{name} is too big for the archive ({size} bytes)"); } // WHAT: ustar's size limit (8 GiB). WHY: refuse rather than write a broken archive.
        let mut h = [0u8; 512]; // WHAT: the header block. WHY: ustar's fixed layout.
        h[..name.len()].copy_from_slice(name.as_bytes()); // WHAT: the name. WHY: field 0.
        h[100..108].copy_from_slice(b"0000600\0"); // WHAT: owner-only file mode. WHY: a restored copy is not handed to everyone.
        h[108..116].copy_from_slice(b"0000000\0"); h[116..124].copy_from_slice(b"0000000\0"); // WHAT: owner and group 0. WHY: meaningless across machines.
        h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes()); // WHAT: the size in octal. WHY: ustar's form.
        h[136..148].copy_from_slice(b"00000000000\0"); // WHAT: no date. WHY: the manifest carries the one date that matters.
        h[156] = b'0'; // WHAT: a regular file. WHY: nothing else is ever written.
        h[257..263].copy_from_slice(b"ustar\0"); h[263..265].copy_from_slice(b"00"); // WHAT: the format marker. WHY: any tar reads it.
        h[148..156].copy_from_slice(b"        "); // WHAT: blanks while summing. WHY: the checksum's rule.
        let sum: u32 = h.iter().map(|&b| b as u32).sum(); // WHAT: the checksum. WHY: readers check it.
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes()); // WHAT: write it. WHY: ustar's form.
        self.w.write_all(&h)?; // WHAT: the header. WHY: before its bytes.
        let copied = std::io::copy(&mut body.take(size), self.w)?; // WHAT: the file's bytes. WHY: streamed.
        if copied != size { bail!("{name} changed size while it was packed"); } // WHAT: refuse a short file. WHY: the header promised its size.
        let pad = (512 - (size % 512) as usize) % 512; self.w.write_all(&vec![0u8; pad])?; // WHAT: pad to a block. WHY: ustar's rule.
        Ok(())
    }
    fn end(&mut self) -> Result<()> { self.w.write_all(&[0u8; 1024])?; Ok(()) } // WHAT: two empty blocks. WHY: the end marker.
}

/// Reads an archive into a folder. Refuses anything that isn't a short plain file name: a damaged or planted
/// archive can never write outside `into`.
fn untar(r: &mut dyn Read, into: &Path, limit: u64) -> Result<Vec<String>> {
    let mut names = Vec::new(); // WHAT: what came out. WHY: the caller checks it.
    let mut total = 0u64; // WHAT: bytes so far. WHY: a planted archive can't fill the disk.
    loop {
        let mut h = [0u8; 512]; // WHAT: one header. WHY: read block by block.
        r.read_exact(&mut h).context("the archive is cut short")?; // WHAT: read it. WHY: an archive must end with its end marker.
        if h.iter().all(|&b| b == 0) { return Ok(names); } // WHAT: the end. WHY: everything was read.
        let field = |a: usize, z: usize| std::str::from_utf8(&h[a..z]).ok().map(|t| t.trim_matches(|c: char| c == '\0' || c == ' ').to_string()); // WHAT: a header field as text. WHY: numbers in ustar are octal text.
        let stored = field(148, 156).and_then(|t| u32::from_str_radix(&t, 8).ok()).ok_or_else(|| anyhow!("a damaged archive header"))?; // WHAT: the header's own checksum. WHY: a damaged header is refused.
        let mut blank = h; blank[148..156].copy_from_slice(b"        "); // WHAT: blanks where the sum goes. WHY: the rule it was made with.
        if blank.iter().map(|&b| b as u32).sum::<u32>() != stored { bail!("a damaged archive header"); } // WHAT: compare. WHY: never act on a damaged header.
        if h[156] != b'0' && h[156] != 0 { bail!("the archive holds something that isn't a plain file; refusing it"); } // WHAT: files only. WHY: links could point outside the folder.
        let end = h[..100].iter().position(|&b| b == 0).unwrap_or(100); // WHAT: the name's end. WHY: names are NUL-padded.
        let name = std::str::from_utf8(&h[..end])?.to_string(); // WHAT: the name. WHY: checked next.
        if !key_ok(&name) || h[345] != 0 { bail!("the archive holds a name this program never writes: {name:?}; refusing it"); } // WHAT: only our plain names, no prefix field. WHY: no ../, no absolute path, no drive letter.
        let size = field(124, 136).and_then(|t| u64::from_str_radix(&t, 8).ok()).ok_or_else(|| anyhow!("a damaged size in the archive"))?; // WHAT: its size. WHY: how much to read.
        total += size; if total > limit { bail!("the archive is bigger than it said it would be; refusing it"); } // WHAT: the cap. WHY: never fill the disk on a planted archive.
        let path = name.split('/').fold(into.to_path_buf(), |p, s| p.join(s)); // WHAT: where it goes. WHY: inside `into`, by construction.
        if !path.starts_with(into) || path.components().count() != into.components().count() + name.split('/').count() { bail!("an archive name would land outside its folder: {name:?}; refusing it"); } // WHAT: checked again, by path. WHY: never written outside, whatever the name check missed.
        if let Some(p) = path.parent() { create_private_dir(p)?; } // WHAT: its folder, owner-only. WHY: made as needed.
        let mut f = create_private(&path).with_context(|| format!("{name} appears twice, or can't be written"))?; // WHAT: a new owner-only file, never over one. WHY: a name twice is a planted archive, and a restored database is never readable by others.
        let n = std::io::copy(&mut r.take(size), &mut f)?; // WHAT: its bytes. WHY: streamed.
        if n != size { bail!("the archive is cut short"); } // WHAT: a short body. WHY: refused.
        let pad = (512 - (size % 512)) % 512; std::io::copy(&mut r.take(pad), &mut std::io::sink())?; // WHAT: skip the padding. WHY: to the next header.
        names.push(name); // WHAT: note it. WHY: returned.
    }
}

// ---------------------------------------------------------------- the manifest

/// Inside the encrypted catalog: everything the drill needs, and the counts it checks.
#[derive(Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,                     // WHAT: "excalibur-offsite-1". WHY: a later format is refused, not misread.
    pub product: String,                    // WHAT: "view". WHY: the drill checks it restored the right product.
    pub name: String,                       // WHAT: the catalog's own name. WHY: a renamed object is noticed.
    #[serde(rename = "createdAt")]
    pub created_at: String,                 // WHAT: when the data was copied. WHY: the backup's age.
    #[serde(rename = "appVersion")]
    pub app_version: String,                // WHAT: the server's version. WHY: restore with the same or newer.
    #[serde(rename = "databaseSha256")]
    pub database_sha256: String,            // WHAT: the database copy's fingerprint. WHY: the drill checks the bytes.
    pub counts: BTreeMap<String, i64>,      // WHAT: rows per table. WHY: the drill compares.
    pub salt: String,                       // WHAT: the salt in the drawings' object names. WHY: the drill finds each one.
    pub blobs: Vec<(String, u64)>,          // WHAT: every drawing file: digest and size. WHY: the drill checks each is in the bucket.
    #[serde(rename = "missingOnDisk")]
    pub missing_on_disk: Vec<String>,       // WHAT: files the database names that weren't on this disk. WHY: damage found by the backup is said, never hidden.
}

/// Beside the catalog, unencrypted: sizes and fingerprints only, never a name or a record.
#[derive(Serialize, Deserialize)]
pub struct Sidecar {
    pub format: String,                     // WHAT: the format. WHY: as above.
    pub name: String,                       // WHAT: the catalog's name. WHY: matched on download.
    #[serde(rename = "createdAt")]
    pub created_at: String,                 // WHAT: when. WHY: shown without the key.
    #[serde(rename = "appVersion")]
    pub app_version: String,                // WHAT: which version. WHY: the same.
    #[serde(rename = "ageSha256")]
    pub age_sha256: String,                 // WHAT: the encrypted file's fingerprint. WHY: a damaged download is caught before decrypting.
    #[serde(rename = "ageBytes")]
    pub age_bytes: u64,                     // WHAT: its size. WHY: the upload is confirmed by it.
    #[serde(rename = "blobCount")]
    pub blob_count: usize,                  // WHAT: how many drawing files. WHY: a quick sense of size.
    #[serde(rename = "blobBytes")]
    pub blob_bytes: u64,                    // WHAT: their total. WHY: the same.
}

/// The two markers in the data folder the ops report reads.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Marker {
    #[serde(rename = "lastGood", default)]
    pub last_good: Option<String>,          // WHAT: when the last confirmed backup's data was copied. WHY: backupDays.
    #[serde(default)]
    pub r#where: Option<String>,            // WHAT: where it went, in words. WHY: Fleet shows it.
    #[serde(default)]
    pub object: Option<String>,             // WHAT: which catalog. WHY: the drill and a person can find it.
    #[serde(default)]
    pub ok: Option<bool>,                   // WHAT: a drill's result. WHY: only a pass counts.
    #[serde(default)]
    pub at: Option<String>,                 // WHAT: when the drill ran. WHY: restoreTested.
}

pub const BACKUP_MARKER: &str = "offsite-backup.json"; // WHAT: written after a confirmed backup. WHY: the ops report's backupDays.
pub const DRILL_MARKER: &str = "offsite-drill.json";   // WHAT: written after a passed drill. WHY: the ops report's restoreTested.

/// What the ops report says about off-site backups: each field `None` when unknown, never a guess.
#[derive(Serialize, Default, Clone, Debug, PartialEq)]
pub struct Report {
    #[serde(rename = "lastGood")]
    pub last_good: Option<String>,      // WHAT: when the last confirmed backup was taken. WHY: how old.
    pub r#where: Option<String>,        // WHAT: where it went. WHY: "backed up" means nothing without where.
    #[serde(rename = "restoreTested")]
    pub restore_tested: Option<String>, // WHAT: when a drill last passed. WHY: a backup never restored is unproven.
}

fn past_time(s: &Option<String>) -> Option<time::OffsetDateTime> {
    let t = time::OffsetDateTime::parse(s.as_deref()?, &time::format_description::well_known::Rfc3339).ok()?; // WHAT: read the time. WHY: a garbled one is unknown.
    (t <= time::OffsetDateTime::now_utc() + time::Duration::hours(1)).then_some(t) // WHAT: refuse the future. WHY: a backup from tomorrow isn't believed.
}

fn read_marker(path: &Path) -> Option<Marker> {
    let m = std::fs::symlink_metadata(path).ok()?; // WHAT: the file itself. WHY: a link is not ours.
    if !m.is_file() || m.len() > 64 * 1024 { return None; } // WHAT: a small plain file only. WHY: anything else is unknown, not believed.
    serde_json::from_slice(&std::fs::read(path).ok()?).ok() // WHAT: parse it. WHY: a damaged file is unknown.
}

/// The ops report's off-site section, read from the data folder.
pub fn report(data: &Path) -> Report {
    let b = read_marker(&data.join(BACKUP_MARKER)).unwrap_or_default(); // WHAT: the backup marker. WHY: unknown when absent.
    let d = read_marker(&data.join(DRILL_MARKER)).unwrap_or_default(); // WHAT: the drill marker. WHY: the same.
    let iso = |t: time::OffsetDateTime| t.format(&time::format_description::well_known::Rfc3339).ok(); // WHAT: back to text. WHY: one form in the report.
    Report {
        last_good: past_time(&b.last_good).and_then(iso),                                             // WHAT: a real past time, or None. WHY: never a guess.
        r#where: b.r#where.filter(|_| past_time(&b.last_good).is_some()).map(|w| w.chars().take(200).collect()), // WHAT: where, with a good time only. WHY: half a marker is no marker.
        restore_tested: if d.ok == Some(true) { past_time(&d.at).and_then(iso) } else { None },       // WHAT: a passed drill's time only. WHY: a failed drill proves nothing.
    }
}

/// Days since the last confirmed off-site backup, or `None` for never.
pub fn backup_days(data: &Path) -> Option<f64> {
    let t = past_time(&read_marker(&data.join(BACKUP_MARKER))?.last_good)?; // WHAT: its time. WHY: unknown when absent or damaged.
    Some(((time::OffsetDateTime::now_utc() - t).as_seconds_f64() / 86_400.0).max(0.0)) // WHAT: in days. WHY: Fleet's backupDays.
}

// ---------------------------------------------------------------- the job

/// Everything one run needs.
pub struct Job<'a> {
    pub product: &'a Product, // WHAT: which product. WHY: its files and checks.
    pub data: PathBuf,        // WHAT: the live data folder. WHY: what is backed up, and where the markers go.
    pub work: PathBuf,        // WHAT: the job's own folder (outside the data). WHY: programs, public key, spool and log.
    pub target: Target,       // WHAT: where it goes. WHY: a folder or a bucket.
    pub cipher: Cipher,       // WHAT: age. WHY: the lock.
    pub version: String,      // WHAT: this server's version. WHY: in the manifest.
    pub mark: bool,           // WHAT: whether a good run and a passed drill are marked. WHY: only for the server's own target; a hand run elsewhere moves nothing.
}

/// A run's end: an exit code and its reason. 0 good · 5 good, with a weekly/monthly or prune warning · 2 refused
/// before anything was marked · 3 the upload failed or did not read back · 4 bad usage.
#[derive(Debug)]
pub struct Exit { pub code: i32, pub why: String } // WHAT: code and reason. WHY: the scheduled task shows the code; the log shows the reason.

fn refuse(code: i32, why: impl std::fmt::Display) -> Exit { Exit { code, why: why.to_string() } } // WHAT: make one. WHY: short.

impl Job<'_> {
    fn own(&self) -> PathBuf { self.work.join(self.product.id) } // WHAT: this product's corner of the work folder. WHY: several products can share one work folder.

    /// One line to the console and the job's log.
    pub fn log(&self, line: &str) {
        let line = format!("{}  {line}", time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap_or_default()); // WHAT: stamped. WHY: read the next morning.
        println!("{line}"); // WHAT: to the console. WHY: a person running it sees it.
        let path = self.own().join("offsite.log"); // WHAT: the log file. WHY: beside the job.
        if std::fs::metadata(&path).map(|m| m.len() > 4 * 1024 * 1024).unwrap_or(false) { let _ = std::fs::rename(&path, self.own().join("offsite.log.1")); } // WHAT: one old log kept past 4 MB. WHY: a log must never fill the disk.
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) { let _ = writeln!(f, "{line}"); } // WHAT: append. WHY: best effort; the console still has it.
    }

    /// The nightly backup. Returns the exit code to end with.
    pub fn backup(&self) -> Exit {
        let own = self.own(); // WHAT: the job's folder. WHY: lock, stage and spool live there.
        if inside(&own, &self.data) || inside(&self.data, &own) { return refuse(4, "the work folder and the data folder must be apart: a backup inside what it backs up grows forever"); } // WHAT: refuse overlap. WHY: the spool would be backed up into itself.
        if let Err(e) = create_private_dir(&own).and_then(|_| create_private_dir(&own.join("spool"))) { return refuse(2, format!("could not make {}: {e}", own.display())); } // WHAT: make it, owner-only. WHY: the first run; the stage inside holds the database unencrypted.
        let lock = match hold_lock(&own.join("offsite.lock")) { Ok(l) => l, Err(e) => return e }; // WHAT: one run at a time, held by the system. WHY: a crashed run's lock goes with it; a first upload may run past any age a stale-lock rule could pick.
        let stage = own.join("stage"); // WHAT: the plaintext stage. WHY: deleted at the start and the end of every run.
        let _ = std::fs::remove_dir_all(&stage); // WHAT: clear what a killed run left. WHY: the database copy is unencrypted.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.backup_locked(&stage))).unwrap_or_else(|p| Err(refuse(3, format!("the backup stopped part way: {}", panic_words(p.as_ref()))))); // WHAT: the run itself; a fault inside is a failed run. WHY: a failure is always reported and cleaned up, never escapes.
        let _ = std::fs::remove_dir_all(&stage); // WHAT: delete the plaintext stage. WHY: it holds the database unencrypted.
        let _ = std::fs::remove_dir_all(own.join("outgoing")); // WHAT: encrypted drawings not yet sent. WHY: tomorrow lists the bucket again.
        drop(lock); // WHAT: free the lock; the file stays. WHY: deleting it could let two runs each hold a lock on a different file.
        match &result { Ok(0) => self.log("DONE. Tonight's backup is off this machine and marked."), Ok(c) => self.log(&format!("DONE, with housekeeping warnings above (exit {c}). Tonight's backup is safe and marked.")), Err(e) => self.log(&format!("FAILED: {}", e.why)) } // WHAT: the last line. WHY: the one to read in the morning.
        match result { Ok(c) => Exit { code: c, why: String::new() }, Err(e) => e } // WHAT: the result. WHY: the exit code.
    }

    fn backup_locked(&self, stage: &Path) -> std::result::Result<i32, Exit> {
        #[cfg(test)]
        if FAULT.with(|f| f.get()) { panic!("a fault planted by a test"); } // WHAT: the tests' fault. WHY: proves a fault becomes a failed run.
        let own = self.own(); // WHAT: the job's folder. WHY: spool and salt.
        let recipients = self.work.join("age-recipients.txt"); // WHAT: the public key file. WHY: Creede puts it there.
        let keys = check_recipients(&std::fs::read_to_string(&recipients).unwrap_or_default()).map_err(|e| refuse(2, format!("{e} ({})", recipients.display())))?; // WHAT: check it first. WHY: refuse before anything is copied.
        if let Some(found) = private_key_in(&self.work) { return Err(refuse(2, format!("{} holds a PRIVATE key. Only the public line (age1...) belongs on this server: move that file off this machine, to Creede", found.display()))); } // WHAT: a private key anywhere in the work folder. WHY: the key that opens every backup must never sit beside them.
        if let Target::Rclone { config, exe, .. } = &self.target { if !config.exists() || !exe.exists() { return Err(refuse(2, format!("rclone or its settings are missing ({} / {}). Creede sets these up once (README.md, Off-site backups).", exe.display(), config.display()))); } } // WHAT: the bucket's settings. WHY: say what's missing, not an rclone error at night.
        if let Cipher::Age(exe) = &self.cipher { if !exe.exists() { return Err(refuse(2, format!("age is missing ({})", exe.display()))); } } // WHAT: the lock program. WHY: the same.
        self.log(&format!("off-site backup of {} starting -> {} ({keys} key{} can open it; none of them is on this machine)", self.product.id, self.target.words(), if keys > 1 { "s" } else { "" })); // WHAT: say what is about to happen. WHY: the log reads like a story.

        // 1. a consistent copy of the database, while the server goes on working
        let started = time::OffsetDateTime::now_utc(); // WHAT: when the data is frozen. WHY: the backup's age counts from here.
        create_private_dir(stage).map_err(|e| refuse(2, e))?; // WHAT: the stage, owner-only. WHY: the database copy goes there unencrypted.
        let live_db = self.data.join(self.product.database); // WHAT: the live database. WHY: copied.
        if !live_db.is_file() { return Err(refuse(2, format!("there is no database at {}; is this the right data folder?", live_db.display()))); } // WHAT: refuse a wrong folder. WHY: an empty backup must never be marked good.
        let copy = stage.join(self.product.database); // WHAT: the stage copy. WHY: packed into the catalog.
        snapshot(&live_db, &copy).map_err(|e| refuse(2, format!("could not copy the database: {e:#}")))?; // WHAT: SQLite copies itself consistently. WHY: a file copy of a database in use can be one that won't open.
        let (counts, refs) = check_database(self.product, &copy).map_err(|e| refuse(2, format!("the database copy did not check out: {e:#}")))?; // WHAT: open it and count. WHY: a copy that won't open is never sent.
        self.log(&format!("database copied and checked: {}", counts.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "))); // WHAT: the counts. WHY: the log shows what was backed up.

        // 2. the drawing files: the ones on disk, each sent once
        let mut on_disk = list_blobs(&self.data.join(self.product.blobs)).map_err(|e| refuse(2, format!("could not list the files: {e:#}")))?; // WHAT: every file by digest. WHY: what must be in the bucket.
        let mut missing_on_disk: Vec<String> = refs.iter().filter(|d| !on_disk.contains_key(*d)).cloned().collect(); // WHAT: files the database names that aren't here. WHY: damage the backup must report.
        if !missing_on_disk.is_empty() { self.log(&format!("WARNING: {} file(s) the database names are not on this disk (first: {:.12}). They can't be backed up; the rest is.", missing_on_disk.len(), missing_on_disk[0])); } // WHAT: say so. WHY: never hidden.
        let salt = salt(&own).map_err(|e| refuse(2, format!("could not read or make the name salt: {e:#}")))?; // WHAT: the salt for object names. WHY: the bucket doesn't say which files it holds.
        let there: BTreeMap<String, u64> = self.target.list("blobs").map_err(|e| refuse(3, format!("could not list the bucket's files: {e:#}")))?.into_iter().collect(); // WHAT: what the bucket already holds. WHY: send only what is new.
        let index_path = own.join("blob-sizes.json"); // WHAT: each sent file's encrypted size, by object name. WHY: the read-back compares exact sizes, also for files sent on earlier nights.
        let mut sizes: BTreeMap<String, u64> = read_sizes(&index_path).map_err(|e| refuse(2, format!("{e:#}")))?; // WHAT: the sizes so far. WHY: as above.
        let outgoing = own.join("outgoing"); // WHAT: encrypted drawings waiting to go. WHY: sent in batches.
        let _ = std::fs::remove_dir_all(&outgoing); // WHAT: start empty. WHY: a killed run's leftovers are made again.
        let (mut sent, mut sent_bytes, mut batch_bytes) = (0usize, 0u64, 0u64); // WHAT: tallies. WHY: the log and the batch size.
        let mut damaged = Vec::new(); // WHAT: files whose bytes don't match their name. WHY: never sent, and reported.
        let mut warned = 0usize; // WHAT: warnings that leave tonight's backup good but not clean. WHY: exit 5, for someone to look at.
        let send = |sizes: &mut BTreeMap<String, u64>| -> std::result::Result<(), Exit> { // WHAT: send one batch and note each file's encrypted size. WHY: the sizes the read-back expects.
            let mut batch = BTreeMap::new(); // WHAT: this batch's sizes. WHY: noted only once sent.
            for e in std::fs::read_dir(&outgoing).map_err(|e| refuse(2, e))?.flatten() { batch.insert(e.file_name().to_string_lossy().into_owned(), e.metadata().map_err(|e| refuse(2, e))?.len()); } // WHAT: each encrypted file's size. WHY: as sent.
            self.target.put_all(&outgoing, "blobs").map_err(|e| refuse(3, format!("sending files failed: {e:#}")))?; // WHAT: send them. WHY: off the box.
            sizes.extend(batch); write_atomic(&index_path, &serde_json::to_vec(&*sizes).map_err(|e| refuse(2, e))?).map_err(|e| refuse(2, format!("could not keep the sizes of the files sent: {e:#}")))?; // WHAT: keep the sizes. WHY: tomorrow's read-back checks them too.
            let _ = std::fs::remove_dir_all(&outgoing); Ok(()) // WHAT: empty the spool. WHY: never holds the whole data.
        };
        for (digest, (path, size)) in &on_disk { // WHAT: each file. WHY: the new ones are sent.
            let obj = object_for(&salt, digest); // WHAT: its object name. WHY: salted.
            if there.contains_key(&obj) { continue; } // WHAT: already there. WHY: files never change, so once is enough; the manifest keeps the fingerprint already in the bucket.
            if !matches_digest(path, digest).unwrap_or(false) { self.log(&format!("WARNING: {:.12} on disk doesn't match its own fingerprint; not sent", digest)); damaged.push(digest.clone()); continue; } // WHAT: check before sending. WHY: never send damaged bytes as good ones.
            #[cfg(test)]
            if CHANGE_WHILE_SENT.with(|c| c.get()) { std::fs::write(path, b"changed while it was being sent").map_err(|e| refuse(2, e))?; } // WHAT: the tests' change between the check and the send. WHY: the fingerprint taken while sending must catch it.
            std::fs::create_dir_all(&outgoing).map_err(|e| refuse(2, e))?; // WHAT: the batch folder. WHY: made when needed.
            let out = outgoing.join(&obj); // WHAT: the encrypted file. WHY: sent with the batch.
            let as_sent = self.cipher.encrypt_file(&recipients, path, &out).map_err(|e| refuse(2, format!("encrypting a file failed: {e:#}")))?; // WHAT: lock it, fingerprinting the bytes as they go. WHY: nothing leaves unencrypted, and what left is known.
            if as_sent != *digest { let _ = std::fs::remove_file(&out); self.log(&format!("WARNING: {:.12} changed while it was being sent; not sent tonight", digest)); damaged.push(digest.clone()); continue; } // WHAT: the bytes sent aren't the ones named. WHY: an object must hold exactly what its name says.
            sent += 1; sent_bytes += *size; batch_bytes += *size; // WHAT: count it. WHY: the log and the batch.
            if batch_bytes > 1 << 30 { send(&mut sizes)?; batch_bytes = 0; } // WHAT: send each gigabyte. WHY: the spool never holds the whole data.
        }
        if outgoing.exists() { send(&mut sizes)?; } // WHAT: the last batch. WHY: everything new is sent.
        if !damaged.is_empty() || !missing_on_disk.is_empty() { warned += 1; } // WHAT: something the database names isn't in tonight's backup. WHY: good with a warning, never plain good.
        for d in &damaged { on_disk.remove(d); missing_on_disk.push(d.clone()); } // WHAT: damaged files are not in this backup. WHY: the manifest lists only what the bucket holds.
        let after: BTreeMap<String, u64> = self.target.list("blobs").map_err(|e| refuse(3, format!("could not read the bucket back: {e:#}")))?.into_iter().collect(); // WHAT: read the bucket back. WHY: only what is really there counts.
        let unknown = on_disk.keys().filter(|d| !sizes.contains_key(&object_for(&salt, d))).count(); // WHAT: files in the bucket whose sent size this job never noted. WHY: their size can't be checked.
        if unknown > 0 { return Err(refuse(3, format!("{unknown} file(s) in the target have no record of the size they were sent at ({}); not marking. If that file was lost, move it and name-salt.txt aside: the next run sends every file again under new names", index_path.display()))); } // WHAT: refuse. WHY: only an exact size proves the object whole.
        let absent = on_disk.keys().filter(|d| { let o = object_for(&salt, d); after.get(&o) != sizes.get(&o) }).count(); // WHAT: files not there at exactly the size sent. WHY: a short or padded object is not the one sent.
        if absent > 0 { return Err(refuse(3, format!("{absent} file(s) are not in the target at the size they were sent; not marking"))); } // WHAT: refuse. WHY: a catalog must never name a file the target doesn't hold.
        self.log(&format!("files: {} in all, {sent} new sent ({:.1} MB), every one read back", on_disk.len(), sent_bytes as f64 / 1048576.0)); // WHAT: the tally. WHY: upload time and cost follow from it.

        // 3. the catalog: database and manifest, packed, gzipped and locked
        let name = object_name(self.product.id, started); // WHAT: tonight's name. WHY: dated to the frozen data.
        let manifest = Manifest { format: "excalibur-offsite-1".into(), product: self.product.id.into(), name: name.clone(), created_at: started.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(), app_version: self.version.clone(), database_sha256: sha256_file(&copy).map_err(|e| refuse(2, e))?, counts: counts.clone(), salt: salt.clone(), blobs: on_disk.iter().map(|(d, (_, s))| (d.clone(), *s)).collect(), missing_on_disk }; // WHAT: the manifest. WHY: what the drill checks against.
        let mj = serde_json::to_vec_pretty(&manifest).map_err(|e| refuse(2, e))?; // WHAT: as JSON. WHY: packed first.
        let spool = own.join("spool"); let part = spool.join(format!("{name}.part")); let out = spool.join(&name); // WHAT: where the encrypted catalog is made. WHY: .part until it is whole.
        let db_len = std::fs::metadata(&copy).map_err(|e| refuse(2, e))?.len(); // WHAT: the copy's size. WHY: the tar header.
        let packed = self.cipher.encrypt_stream(&recipients, &part, |w| { let mut gz = flate2::write::GzEncoder::new(w, flate2::Compression::new(6)); { let mut t = TarOut { w: &mut gz }; t.file("manifest.json", mj.len() as u64, &mut &mj[..])?; t.file(self.product.database, db_len, &mut std::fs::File::open(&copy)?)?; t.end()?; } gz.finish()?; Ok(()) }); // WHAT: manifest and database, tarred, gzipped, into age. WHY: never unencrypted on disk.
        if let Err(e) = packed { let _ = std::fs::remove_file(&part); return Err(refuse(2, format!("packing or encrypting failed: {e:#}"))); } // WHAT: remove a half file. WHY: never sent.
        std::fs::rename(&part, &out).map_err(|e| refuse(2, e))?; // WHAT: its real name. WHY: only now whole.
        let age_sha = sha256_file(&out).map_err(|e| refuse(2, e))?; let age_bytes = std::fs::metadata(&out).map_err(|e| refuse(2, e))?.len(); // WHAT: fingerprint and size. WHY: the sidecar and the read-back.
        let side = Sidecar { format: manifest.format.clone(), name: name.clone(), created_at: manifest.created_at.clone(), app_version: self.version.clone(), age_sha256: age_sha.clone(), age_bytes, blob_count: on_disk.len(), blob_bytes: on_disk.values().map(|(_, s)| s).sum() }; // WHAT: the unencrypted facts. WHY: no names, no records.
        let side_path = spool.join(format!("{name}.json")); std::fs::write(&side_path, serde_json::to_vec_pretty(&side).map_err(|e| refuse(2, e))?).map_err(|e| refuse(2, e))?; // WHAT: write it. WHY: sent beside the catalog.

        // 4. send the catalog and read it back
        self.target.put(&out, &format!("daily/{name}")).map_err(|e| refuse(3, format!("the upload failed: {e:#}")))?; // WHAT: send it. WHY: off the box.
        self.target.put(&side_path, &format!("daily/{name}.json")).map_err(|e| refuse(3, format!("the sidecar upload failed: {e:#}")))?; // WHAT: and its sidecar. WHY: the drill checks the fingerprint.
        let daily = self.target.list("daily").map_err(|e| refuse(3, format!("could not read the bucket back: {e:#}")))?; // WHAT: read back. WHY: the proof.
        if !daily.iter().any(|(n, s)| *n == name && *s == age_bytes) { return Err(refuse(3, format!("the bucket does not hold tonight's catalog at full size ({age_bytes} bytes); not marking"))); } // WHAT: refuse a short or missing one. WHY: only a confirmed copy counts.
        self.log(&format!("catalog sent and read back: daily/{name} ({age_bytes} bytes)")); // WHAT: note it. WHY: the important line.

        // 5. mark it: only now does Fleet hear of it, and only for the target the server is set to
        if !self.mark { self.log(&format!("sent to {}, which isn't this server's own off-site target: not marked, backupDays unchanged", self.target.words())); } else { // WHAT: a hand run elsewhere. WHY: a USB disk tried once must not stand for the nightly off-site copy.
        let marker = Marker { last_good: Some(manifest.created_at.clone()), r#where: Some(self.target.words()), object: Some(format!("daily/{name}")), ..Marker::default() }; // WHAT: the marker. WHY: backupDays and where.
        write_atomic(&self.data.join(BACKUP_MARKER), &serde_json::to_vec_pretty(&marker).map_err(|e| refuse(3, e))?).map_err(|e| refuse(3, format!("sent, but could not write the marker: {e:#}")))?; // WHAT: write it whole. WHY: Fleet never reads half a file.
        } // WHAT: the end of the marking. WHY: skipped for another target.

        // 6. weekly / monthly copies and the keep rule
        let mut house = warned; // WHAT: housekeeping failures, and the warnings above. WHY: exit 5, after the backup is already safe.
        let mut listing = BTreeMap::new(); // WHAT: each folder's names. WHY: the keep rule.
        for f in ["daily", "weekly", "monthly"] { match self.target.list(f) { Ok(v) => { listing.insert(f.to_string(), v.into_iter().map(|(n, _)| n).collect::<Vec<_>>()); } Err(e) => { house += 1; self.log(&format!("WARNING: could not list {f}: {e:#}")); listing.insert(f.to_string(), Vec::new()); } } } // WHAT: list each. WHY: a folder that won't list is skipped, not guessed.
        let p = plan(self.product.id, &listing, &name).map_err(|e| refuse(3, e))?; // WHAT: the plan. WHY: the keep rule.
        for (g, want) in [("weekly", p.weekly), ("monthly", p.monthly)] { if want { // WHAT: the first of its week or month. WHY: it stands for that week or month.
            match self.target.copy(&format!("daily/{name}"), &format!("{g}/{name}")).and_then(|_| self.target.copy(&format!("daily/{name}.json"), &format!("{g}/{name}.json"))) { Ok(()) => self.log(&format!("{g} copy: {g}/{name}")), Err(e) => { house += 1; self.log(&format!("WARNING: the {g} copy failed (tomorrow tries again): {e:#}")); } } // WHAT: copy inside the target. WHY: nothing is sent again.
        } }
        for (folder, n) in &p.deletes { // WHAT: past the keep. WHY: 7 / 4 / 12.
            match self.target.delete(&format!("{folder}/{n}")) { Ok(()) => { let _ = self.target.delete(&format!("{folder}/{n}.json")); self.log(&format!("pruned {folder}/{n}")); } Err(e) => { house += 1; self.log(&format!("WARNING: could not delete {folder}/{n} (a bucket lock may still hold it; tomorrow tries again): {e:#}")); } } // WHAT: delete it and its sidecar. WHY: a lock refusing is the lock working.
        }

        // 7. the spool keeps tonight's encrypted catalog only, for a quick restore on this box
        if let Ok(rd) = std::fs::read_dir(&spool) { for e in rd.flatten() { let n = e.file_name().to_string_lossy().into_owned(); if n != name && n != format!("{name}.json") { let _ = std::fs::remove_file(e.path()); } } } // WHAT: older ones go. WHY: they are in the bucket already.
        Ok(if house > 0 { 5 } else { 0 }) // WHAT: 0, or 5 for housekeeping. WHY: the scheduled task shows a non-zero result to look at.
    }
}

/// Takes the run's lock: the file opened (Windows: with no sharing) and locked by the system, so it is released when the
/// run ends or the process dies. `Err` is the exit to end with.
fn hold_lock(path: &Path) -> std::result::Result<std::fs::File, Exit> {
    let mut o = std::fs::OpenOptions::new(); o.read(true).write(true).create(true).truncate(false); // WHAT: open or make it, never emptied. WHY: its contents don't matter, only the hold.
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::share_mode(&mut o, 0); // WHAT: no sharing. WHY: a second run can't even open it.
    let f = match o.open(path) { Ok(f) => f, Err(e) if e.raw_os_error() == Some(32) => return Err(refuse(2, format!("another off-site backup is still running ({})", path.display()))), Err(e) => return Err(refuse(2, format!("could not open the lock {}: {e}", path.display()))) }; // WHAT: open it; Windows' sharing violation is a run going. WHY: say which.
    match f.try_lock() { Ok(()) => Ok(f), Err(std::fs::TryLockError::WouldBlock) => Err(refuse(2, format!("another off-site backup is still running ({})", path.display()))), Err(std::fs::TryLockError::Error(e)) => Err(refuse(2, format!("could not lock {}: {e}", path.display()))) } // WHAT: the system's lock. WHY: never two at once.
}

/// A fault's message, for the log.
fn panic_words(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "an unexpected fault".into()) // WHAT: the text a panic carries. WHY: the log says what stopped it.
}

#[cfg(test)]
thread_local! {
    /// The tests' fault inside a run, and the tests' file changed while it is sent.
    pub(crate) static FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; // WHAT: panic inside the run. WHY: proves it becomes exit 3.
    pub(crate) static CHANGE_WHILE_SENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; // WHAT: rewrite a file between its check and its encryption. WHY: proves the fingerprint taken while sending decides.
}

/// SQLite's own consistent copy of a database in use (VACUUM INTO), then made a single file.
pub fn snapshot(live: &Path, to: &Path) -> Result<()> {
    let db = Connection::open_with_flags(live, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?; // WHAT: read-only. WHY: the backup can never change the live data.
    db.busy_timeout(std::time::Duration::from_secs(30))?; // WHAT: wait for a writer. WHY: the server may be mid-save.
    db.execute("VACUUM INTO ?1", [to.to_string_lossy()])?; // WHAT: copy it. WHY: one transaction, so the copy is one moment, whole.
    Ok(())
}

/// Opens a copy, checks it, counts it, and lists the files it names.
pub fn check_database(product: &Product, path: &Path) -> Result<(BTreeMap<String, i64>, BTreeSet<String>)> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?; // WHAT: read-only. WHY: checking never changes it.
    let ok: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?; // WHAT: SQLite's own check. WHY: a damaged copy is caught here.
    if ok != "ok" { bail!("integrity check: {ok}"); } // WHAT: refuse it. WHY: the same.
    let mut counts = BTreeMap::new(); // WHAT: rows per table. WHY: the manifest.
    for t in product.counts { counts.insert(t.to_string(), db.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get::<_, i64>(0))?); } // WHAT: count each. WHY: table names are constants.
    let refs = (product.references)(&db)?; // WHAT: the files it names. WHY: each must be backed up.
    if let Some(bad) = refs.iter().find(|d| !is_digest(d)) { bail!("the database names a file that isn't a digest: {bad:.16}"); } // WHAT: refuse a strange one. WHY: digests become paths.
    Ok((counts, refs))
}

fn is_digest(s: &str) -> bool { s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) } // WHAT: 64 lowercase hex letters. WHY: the store's names.

/// Every file in the store, by digest: its path and size. `aa/bb/digest`, nothing else.
pub fn list_blobs(root: &Path) -> Result<BTreeMap<String, (PathBuf, u64)>> {
    let mut out = BTreeMap::new(); // WHAT: the files. WHY: sorted, so runs are repeatable.
    if !root.exists() { return Ok(out); } // WHAT: none yet. WHY: a new server.
    for a in std::fs::read_dir(root)? { let a = a?; if !a.file_type()?.is_dir() { continue; } // WHAT: the first level. WHY: two letters each.
        for b in std::fs::read_dir(a.path())? { let b = b?; if !b.file_type()?.is_dir() { continue; } // WHAT: the second level. WHY: the same.
            for f in std::fs::read_dir(b.path())? { let f = f?; let n = f.file_name().to_string_lossy().into_owned(); let ft = f.file_type()?; // WHAT: each file. WHY: named by its digest.
                if ft.is_file() && is_digest(&n) { out.insert(n, (f.path(), f.metadata()?.len())); } // WHAT: a stored file. WHY: .part files and strangers are left out.
            } } }
    Ok(out)
}

fn matches_digest(path: &Path, digest: &str) -> Result<bool> { Ok(sha256_file(path)? == digest) } // WHAT: the file is what its name says. WHY: never send damaged bytes as good ones.

/// The object name of a drawing file: a salted digest, so the bucket doesn't say which files it holds.
pub fn object_for(salt: &str, digest: &str) -> String { format!("{}.age", hex(&Sha256::new().chain_update(salt.as_bytes()).chain_update(b":").chain_update(digest.as_bytes()).finalize())) } // WHAT: sha256(salt:digest).age. WHY: the same file gets the same name every night.

fn salt(own: &Path) -> Result<String> {
    let path = own.join("name-salt.txt"); // WHAT: kept in the job's folder. WHY: the same names every night; it also travels inside each catalog.
    if let Ok(s) = std::fs::read_to_string(&path) { let s = s.trim().to_string(); if s.len() == 64 && is_digest(&s) { return Ok(s); } bail!("{} is damaged; move it aside and the next run makes a new one (every file is then sent again)", path.display()); } // WHAT: read it. WHY: never silently replaced.
    let s = hex(&rand::random::<[u8; 32]>()); // WHAT: 32 random bytes. WHY: unguessable.
    write_atomic(&path, s.as_bytes())?; Ok(s) // WHAT: keep it. WHY: tomorrow uses it.
}

/// The encrypted sizes of the files sent, by object name. A missing file is an empty record; a damaged one is refused.
fn read_sizes(path: &Path) -> Result<BTreeMap<String, u64>> {
    match std::fs::read(path) { Ok(b) => serde_json::from_slice(&b).with_context(|| format!("{} is damaged; move it and name-salt.txt aside and the next run sends every file again", path.display())), Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()), Err(e) => Err(anyhow!("reading {}: {e}", path.display())) } // WHAT: read it. WHY: never silently replaced.
}

fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() } // WHAT: bytes as hex. WHY: names and fingerprints.

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?; // WHAT: open it. WHY: streamed.
    let mut h = Sha256::new(); let mut buf = vec![0u8; 1 << 20]; // WHAT: the hash and a buffer. WHY: a big file is never held whole.
    loop { let n = f.read(&mut buf)?; if n == 0 { break; } h.update(&buf[..n]); } // WHAT: read through. WHY: the fingerprint.
    Ok(hex(&h.finalize()))
}

/// A new file only its owner can read, owner-only from the moment it exists (never made open and then closed up).
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new(); o.write(true).create_new(true); // WHAT: a new file, never over one. WHY: a name twice is refused, not overwritten.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600); // WHAT: owner read and write only, set as it is made. WHY: a restored database or drawing is as private as the original, even for a moment.
    o.open(path) // WHAT: make it. WHY: the caller writes into it.
}

/// A folder (and any parents it needs) only its owner can open.
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new(); b.recursive(true); // WHAT: make it and its parents. WHY: the first run.
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700); // WHAT: owner only, set as it is made. WHY: the stage and the restore hold plaintext.
    b.create(path) // WHAT: make it. WHY: an existing folder is fine.
}

/// A new folder, made owner-only as it is made and refused if anything is already there. Unix: 0700. Windows: inheritance
/// removed, full control for the person running it and the system only.
fn new_private_dir(path: &Path) -> Result<()> {
    if let Some(up) = path.parent().filter(|p| !p.as_os_str().is_empty()) { std::fs::create_dir_all(up)?; } // WHAT: its parent. WHY: an ordinary folder above it.
    let mut b = std::fs::DirBuilder::new(); // WHAT: not recursive. WHY: an existing folder is an error, not reused.
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700); // WHAT: owner only, set as it is made. WHY: never open to others, even for a moment.
    b.create(path).with_context(|| format!("making {}", path.display()))?; // WHAT: make it. WHY: fails if anything is already there.
    #[cfg(windows)]
    windows_owner_only(path)?; // WHAT: the person and the system only. WHY: ProgramData and the drive's root let every account read.
    Ok(())
}

/// Windows: replaces a new folder's permissions with full control for the person running this and the system, nothing
/// inherited. icacls and whoami are run by their full paths.
#[cfg(windows)]
fn windows_owner_only(path: &Path) -> Result<()> {
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("System32"); // WHAT: Windows' own programs. WHY: never found on a PATH somebody could change.
    let who = Command::new(system.join("whoami.exe")).args(["/user", "/fo", "csv", "/nh"]).stdin(Stdio::null()).output().context("asking whoami who is running this")?; // WHAT: this account's SID. WHY: a SID works in any language and can't be a look-alike name.
    let sid = String::from_utf8_lossy(&who.stdout).split(',').next_back().map(|s| s.trim().trim_matches('"').to_string()).filter(|s| s.starts_with("S-1-") && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).ok_or_else(|| anyhow!("whoami gave no SID"))?; // WHAT: the SID, checked. WHY: only a well-formed SID reaches icacls.
    let done = Command::new(system.join("icacls.exe")).arg(path).args(["/inheritance:r", "/grant:r"]).arg(format!("*{sid}:(OI)(CI)F")).arg("*S-1-5-18:(OI)(CI)F").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().context("running icacls")?; // WHAT: nothing inherited; this account and the system, for everything inside. WHY: the restored files inherit it.
    if !done.success() { bail!("icacls could not make it owner-only ({done})"); } // WHAT: refuse the drill. WHY: never restore into a folder others can read.
    Ok(())
}

/// The first link or reparse point under `dir`, if any.
fn link_inside(dir: &Path) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()]; // WHAT: folders to look in. WHY: the whole restore.
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { return Some(e.path()) }; // WHAT: the entry itself. WHY: one that can't be looked at isn't trusted.
            #[cfg(windows)]
            if std::os::windows::fs::MetadataExt::file_attributes(&m) & 0x400 != 0 { return Some(e.path()); } // WHAT: FILE_ATTRIBUTE_REPARSE_POINT. WHY: a junction points elsewhere like a link.
            if m.file_type().is_symlink() { return Some(e.path()); } // WHAT: a link. WHY: it would point outside the restore.
            if m.is_dir() { stack.push(e.path()); } // WHAT: look inside. WHY: every level.
        }
    }
    None
}

const SIDECAR_CAP: u64 = 64 * 1024; // WHAT: the largest sidecar read. WHY: a real one is a few hundred bytes.
const MANIFEST_CAP: u64 = 64 * 1024 * 1024; // WHAT: the largest manifest read. WHY: a million files' digests fit; a planted giant is refused unread.

/// Reads a file only if it is a plain file within `cap` bytes.
fn read_capped(path: &Path, cap: u64) -> Result<Vec<u8>> {
    let m = std::fs::symlink_metadata(path).with_context(|| format!("reading {}", path.display()))?; // WHAT: its size, before reading. WHY: refused unread.
    if !m.is_file() || m.len() > cap { bail!("{} is {} bytes, over the {cap} allowed; refusing it", path.display(), m.len()); } // WHAT: the cap. WHY: never read a planted giant into memory.
    Ok(std::fs::read(path)?)
}

/// The manifest as this program writes it: the product's own tables counted, every fingerprint 64 lowercase hex.
fn valid_manifest(product: &Product, man: &Manifest) -> Result<()> {
    let tables: BTreeSet<&str> = man.counts.keys().map(String::as_str).collect(); // WHAT: the tables it counts. WHY: compared below.
    if tables != product.counts.iter().copied().collect::<BTreeSet<_>>() { bail!("its counts are for other tables"); } // WHAT: exactly the product's tables. WHY: a count missing is a check skipped.
    if man.counts.values().any(|n| *n < 0) { bail!("a count is below zero"); } // WHAT: real counts. WHY: written by SQLite's count.
    if !is_digest(&man.database_sha256) || !is_digest(&man.salt) { bail!("the database fingerprint or the salt is not 64 hex digits"); } // WHAT: the two fingerprints. WHY: they name and check files.
    if let Some((d, _)) = man.blobs.iter().find(|(d, _)| !is_digest(d)) { bail!("a file fingerprint is not 64 hex digits: {:.16}", d); } // WHAT: every file. WHY: digests become paths and object names.
    if let Some(d) = man.missing_on_disk.iter().find(|d| !is_digest(d)) { bail!("a missing file's fingerprint is not 64 hex digits: {:.16}", d); } // WHAT: the same. WHY: compared with the database.
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp"); // WHAT: beside it. WHY: one rename puts it in place.
    std::fs::write(&tmp, bytes)?; std::fs::rename(&tmp, path)?; Ok(()) // WHAT: write, then rename. WHY: a reader never sees half a file.
}

fn real(p: &Path) -> PathBuf {
    let mut base = p.to_path_buf(); let mut rest = Vec::new(); // WHAT: the nearest folder that exists, and what is below it. WHY: a folder not made yet still has a real place.
    while !base.exists() { match (base.file_name().map(|n| n.to_os_string()), base.parent().map(Path::to_path_buf)) { (Some(n), Some(up)) => { rest.push(n); base = up; } _ => break } } // WHAT: climb to one that exists. WHY: only that can be resolved.
    let mut out = std::fs::canonicalize(&base).unwrap_or(base); // WHAT: its real path. WHY: links and .. can't hide overlap.
    for n in rest.into_iter().rev() { out.push(n); } out // WHAT: put the rest back. WHY: the full path.
}

fn inside(a: &Path, b: &Path) -> bool { real(a).starts_with(real(b)) } // WHAT: a is b or under it, by real paths. WHY: overlap check that links can't fool.

// ---------------------------------------------------------------- the drill

/// What a drill is asked to do.
pub struct Drill<'a> {
    pub job: Job<'a>,          // WHAT: the product, target and cipher. WHY: the same as the backup's.
    pub identity: PathBuf,     // WHAT: Creede's private key (on a USB stick). WHY: never kept on the server.
    pub scratch: PathBuf,      // WHAT: an empty folder for the restore. WHY: never the live data.
    pub object: Option<String>,// WHAT: a catalog to drill, or the newest. WHY: any copy can be proven.
    pub full: bool,            // WHAT: every file, or a sample. WHY: a full restore is the disaster case; a sample is the monthly check.
    pub sample: usize,         // WHAT: how many files to check when not full. WHY: proves the files open without fetching them all.
    pub keep: bool,            // WHAT: keep the restored copy. WHY: the real restore uses --full --keep.
}

impl Drill<'_> {
    /// Runs the drill. RESTORE DRILL PASSED, or the reason it didn't.
    pub fn run(&self) -> Exit {
        if std::fs::symlink_metadata(&self.scratch).is_ok() { return refuse(4, format!("the scratch folder {} must not exist yet: the drill makes it, owner-only", self.scratch.display())); } // WHAT: refuse any folder, file or link already there. WHY: a folder made earlier may be open to others, or hold anything.
        if inside(&self.scratch, &self.job.data) || inside(&self.job.data, &self.scratch) { return refuse(4, "the scratch folder must be apart from the live data folder"); } // WHAT: refuse overlap. WHY: a restore must never land on the real data.
        if let Err(e) = new_private_dir(&self.scratch) { let _ = std::fs::remove_dir_all(&self.scratch); return refuse(4, format!("could not make {} owner-only: {e:#}", self.scratch.display())); } // WHAT: make it, owner-only from the start. WHY: the restore goes there in plaintext, and inherits it.
        let key = match OpenedKey::open(&self.job.cipher, &self.identity) { Ok(k) => k, Err(e) => { let _ = std::fs::remove_dir_all(&self.scratch); return refuse(2, format!("{e:#}")); } }; // WHAT: a locked key opened once. WHY: a drill decrypts many files; one passphrase, not one per file.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_in(&key.path))).unwrap_or_else(|p| Err(refuse(3, format!("the drill stopped part way: {}", panic_words(p.as_ref()))))); // WHAT: the drill; a fault inside is a failed drill. WHY: the clean-up below runs whatever happened.
        drop(key); // WHAT: delete the opened key now. WHY: it opens every backup.
        if !self.keep || r.is_err() { let _ = std::fs::remove_dir_all(&self.scratch); } // WHAT: delete the restored copy. WHY: it is plaintext.
        match r {
            Ok(summary) if !self.job.mark => { self.job.log(&format!("RESTORE DRILL PASSED (from {}, not this server's own target: not marked): {summary}", self.job.target.words())); Exit { code: 0, why: String::new() } } // WHAT: a drill of another target. WHY: restoreTested speaks for the server's own off-site copy only.
            Ok(summary) => {
                let m = Marker { ok: Some(true), at: time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).ok(), object: Some(summary.clone()), ..Marker::default() }; // WHAT: the drill marker. WHY: restoreTested.
                if let Err(e) = write_atomic(&self.job.data.join(DRILL_MARKER), &serde_json::to_vec_pretty(&m).unwrap_or_default()) { self.job.log(&format!("WARNING: passed, but could not write {}: {e:#}", DRILL_MARKER)); } // WHAT: write it. WHY: Fleet shows when a restore was last proven.
                self.job.log(&format!("RESTORE DRILL PASSED: {summary}")); Exit { code: 0, why: String::new() } // WHAT: the result. WHY: the line to look for.
            }
            Err(e) => { self.job.log(&format!("RESTORE DRILL FAILED: {}", e.why)); e } // WHAT: the reason. WHY: a failed drill marks nothing.
        }
    }

    fn run_in(&self, identity: &Path) -> std::result::Result<String, Exit> {
        let j = &self.job; let dl = self.scratch.join("download"); std::fs::create_dir_all(&dl).map_err(|e| refuse(4, e))?; // WHAT: the download folder. WHY: inside the scratch.
        let name = match &self.object { Some(o) => o.clone(), None => { let mut all = Vec::new(); for f in ["daily", "weekly", "monthly"] { for (n, _) in j.target.list(f).map_err(|e| refuse(3, format!("could not list the bucket: {e:#}")))? { if believable(j.product.id, &n, time::OffsetDateTime::now_utc()) { all.push(format!("{f}/{n}")); } } } all.sort_by_key(|k| k.rsplit('/').next().unwrap_or("").to_string()); all.pop().ok_or_else(|| refuse(3, "there is no backup in the target to drill"))? } }; // WHAT: the catalog, or the newest anywhere. WHY: --latest by default.
        let base = name.rsplit('/').next().unwrap_or(&name).to_string(); // WHAT: its own name. WHY: checked against the manifest.
        if parse_name(j.product.id, &base).is_none() || !key_ok(&name) { return Err(refuse(4, format!("{name} isn't one of this product's catalogs"))); } // WHAT: refuse a stranger. WHY: only our names are fetched.
        let enc = dl.join(&base); j.target.get(&name, &enc).map_err(|e| refuse(3, format!("download failed: {e:#}")))?; j.target.get(&format!("{name}.json"), &dl.join("sidecar.json")).map_err(|e| refuse(3, format!("its sidecar is missing: {e:#}")))?; // WHAT: fetch both. WHY: the sidecar has the fingerprint.
        let side: Sidecar = serde_json::from_slice(&read_capped(&dl.join("sidecar.json"), SIDECAR_CAP).map_err(|e| refuse(2, format!("the sidecar: {e:#}")))?).map_err(|e| refuse(2, format!("the sidecar doesn't read: {e}")))?; // WHAT: read it, if small. WHY: the fingerprint to compare; a planted huge one is refused unread.
        if side.name != base || sha256_file(&enc).map_err(|e| refuse(2, e))? != side.age_sha256 { return Err(refuse(2, "the downloaded catalog is not the one that was uploaded (its fingerprint differs)")); } // WHAT: compare. WHY: a damaged or swapped download is caught before decrypting.
        j.log(&format!("downloaded {name}; its fingerprint matches")); // WHAT: note it. WHY: step one passed.
        let tgz = dl.join("catalog.tar.gz"); j.cipher.decrypt_file(identity, &enc, &tgz).map_err(|e| refuse(2, format!("{e:#}")))?; // WHAT: decrypt with Creede's key. WHY: only that key opens it.
        let restored = self.scratch.join("restored"); // WHAT: the restored data folder. WHY: what a real restore copies into place.
        let mut gz = flate2::read::GzDecoder::new(std::fs::File::open(&tgz).map_err(|e| refuse(2, e))?); // WHAT: gunzip. WHY: the catalog is gzipped.
        let names = untar(&mut gz, &restored, 64 << 30).map_err(|e| refuse(2, format!("unpacking failed: {e:#}")))?; let _ = std::fs::remove_file(&tgz); // WHAT: unpack safely, then drop the plaintext tarball. WHY: only our names, never outside.
        if names.len() != 2 || !names.iter().any(|n| n == "manifest.json") || !names.iter().any(|n| n == j.product.database) { return Err(refuse(2, format!("the catalog holds {names:?}, not a manifest and a database"))); } // WHAT: exactly the two files. WHY: anything else is not ours.
        let man: Manifest = serde_json::from_slice(&read_capped(&restored.join("manifest.json"), MANIFEST_CAP).map_err(|e| refuse(2, format!("the manifest: {e:#}")))?).map_err(|e| refuse(2, format!("the manifest doesn't read: {e}")))?; let _ = std::fs::remove_file(restored.join("manifest.json")); // WHAT: read it if within the cap, then leave a clean data folder. WHY: what the drill checks against.
        valid_manifest(j.product, &man).map_err(|e| refuse(2, format!("the manifest is not one this program writes: {e:#}")))?; // WHAT: every field as written. WHY: a planted manifest can't steer what is fetched or where it goes.
        if man.format != "excalibur-offsite-1" || man.product != j.product.id || man.name != base { return Err(refuse(2, "the manifest is for another product, format or catalog")); } // WHAT: the right one. WHY: never check against the wrong manifest.
        let db = restored.join(j.product.database); // WHAT: the restored database. WHY: checked next.
        if sha256_file(&db).map_err(|e| refuse(2, e))? != man.database_sha256 { return Err(refuse(2, "the restored database's fingerprint differs from the manifest")); } // WHAT: compare. WHY: the bytes are the ones backed up.
        let (counts, refs) = check_database(j.product, &db).map_err(|e| refuse(2, format!("the restored database doesn't check out: {e:#}")))?; // WHAT: open, check, count. WHY: it must open.
        if counts != man.counts { return Err(refuse(2, format!("the restored counts differ from the manifest: {counts:?} vs {:?}", man.counts))); } // WHAT: compare. WHY: nothing lost.
        let listed: BTreeSet<String> = man.blobs.iter().map(|(d, _)| d.clone()).collect(); // WHAT: the files the manifest lists. WHY: checked against the database and the bucket.
        if let Some(d) = refs.iter().find(|d| !listed.contains(*d) && !man.missing_on_disk.contains(*d)) { return Err(refuse(2, format!("the database names a file the manifest doesn't list ({:.12})", d))); } // WHAT: every named file accounted for. WHY: a hole is found now, not on the bad day.
        let there: BTreeMap<String, u64> = j.target.list("blobs").map_err(|e| refuse(3, format!("could not list the bucket's files: {e:#}")))?.into_iter().collect(); // WHAT: the bucket's files. WHY: each listed one must be there.
        if let Some((d, _)) = man.blobs.iter().find(|(d, s)| there.get(&object_for(&man.salt, d)).map(|&b| b <= *s).unwrap_or(true)) { return Err(refuse(2, format!("a file the backup lists is not in the bucket at full size ({:.12})", d))); } // WHAT: all present. WHY: a missing file is a hole in the backup.
        let record = (j.product.record)(&Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| refuse(2, e))?).map_err(|e| refuse(2, e))?; // WHAT: one real record and its file. WHY: opened below.
        let mut want: Vec<String> = if self.full { man.blobs.iter().map(|(d, _)| d.clone()).collect() } else { let mut v: Vec<String> = man.blobs.iter().map(|(d, _)| d.clone()).collect(); let mut r = rand::thread_rng(); rand::seq::SliceRandom::shuffle(&mut v[..], &mut r); v.truncate(self.sample); v }; // WHAT: every file, or a random sample. WHY: monthly checks needn't fetch it all.
        if let Some((_, d)) = &record { if listed.contains(d) && !want.contains(d) { want.push(d.clone()); } } // WHAT: the record's own file too. WHY: the record is opened whole.
        let enc_dir = dl.join("blobs"); let objs: Vec<String> = want.iter().map(|d| object_for(&man.salt, d)).collect(); j.target.get_many("blobs", &objs, &enc_dir).map_err(|e| refuse(3, format!("downloading files failed: {e:#}")))?; // WHAT: fetch them. WHY: decrypted and checked next.
        let files = restored.join(j.product.blobs); // WHAT: the restored file store. WHY: the same layout as the live one.
        for d in &want { // WHAT: each fetched file. WHY: it must decrypt and match its name.
            let to = files.join(&d[0..2]).join(&d[2..4]); let out = to.join(d); // WHAT: its place. WHY: aa/bb/digest.
            if !is_digest(d) || !out.starts_with(&self.scratch) { return Err(refuse(2, "a restored file would land outside the scratch folder; refusing it")); } // WHAT: stays inside. WHY: a restore never writes anywhere else.
            create_private_dir(&to).map_err(|e| refuse(2, e))?; // WHAT: its folder, owner-only. WHY: plaintext inside.
            j.cipher.decrypt_file(identity, &enc_dir.join(object_for(&man.salt, d)), &out).map_err(|e| refuse(2, format!("a file wouldn't decrypt: {e:#}")))?; // WHAT: decrypt it. WHY: with the same key.
            if sha256_file(&out).map_err(|e| refuse(2, e))? != *d { return Err(refuse(2, format!("a restored file doesn't match its fingerprint ({:.12})", d))); } // WHAT: compare. WHY: every byte the same.
        }
        let _ = std::fs::remove_dir_all(&dl); // WHAT: drop the downloads. WHY: encrypted copies no longer needed.
        if let Some(l) = link_inside(&self.scratch) { return Err(refuse(2, format!("{} is a link or reparse point inside the scratch folder; refusing to open the restore", l.display()))); } // WHAT: no links anywhere in the restore. WHY: nothing it opens may point elsewhere.
        let opened = match &record { // WHAT: open one real record through the server's own store. WHY: the app reads it, not just this check.
            Some((id, d)) => { let store = crate::Store::open(&db, &files).map_err(|e| refuse(2, format!("the server could not open the restored data: {e:#}")))?; let bytes = store.get_blob(d).map_err(|e| refuse(2, format!("the server could not read record {id}'s file: {e:#}")))?; format!("record {id} opened ({} bytes, {})", bytes.len(), if bytes.starts_with(b"%PDF") { "a PDF" } else { "a file" }) } // WHAT: the newest set, its drawing read. WHY: proof the restore is usable.
            None => { crate::Store::open(&db, &files).map_err(|e| refuse(2, format!("the server could not open the restored data: {e:#}")))?; "no records yet; the server opened the data".to_string() } // WHAT: an empty server. WHY: still proves it opens.
        };
        Ok(format!("{name}: counts match ({}), {} of {} files fetched and fingerprint-checked{}, every file present in the target, {opened}", counts.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "), want.len(), man.blobs.len(), if self.full { " (full restore)" } else { "" })) // WHAT: the summary. WHY: the log and the marker say what was proven.
    }
}

/// Creede's key for one drill: a locked key (age's own format) is opened once, beside itself on his USB stick, never on
/// the server's disk, and deleted when this is dropped, pass or fail. An unlocked key is used as it is.
struct OpenedKey { path: PathBuf, delete: bool } // WHAT: the key file to decrypt with, and whether it is ours to delete. WHY: an opened key is deleted, the original never.

impl OpenedKey {
    fn open(cipher: &Cipher, identity: &Path) -> Result<OpenedKey> {
        let mut head = [0u8; 21]; let n = std::fs::File::open(identity).with_context(|| format!("reading the key file {}", identity.display()))?.read(&mut head)?; // WHAT: the file's first bytes. WHY: a locked key starts with age's header.
        if !(head[..n].starts_with(b"age-encryption.org/v1") || head[..n].starts_with(b"FAKEPASS1")) { return Ok(OpenedKey { path: identity.to_path_buf(), delete: false }); } // WHAT: not locked. WHY: used as it is.
        let out = identity.parent().unwrap_or(Path::new(".")).join(format!(".drill-key-{}.txt", hex(&rand::random::<[u8; 6]>()))); // WHAT: beside the key, on the stick. WHY: the opened key never touches the server's disk.
        cipher.unlock(identity, &out)?; Ok(OpenedKey { path: out, delete: true }) // WHAT: age asks for the passphrase once. WHY: one prompt for the whole drill.
    }
}

impl Drop for OpenedKey { fn drop(&mut self) { if self.delete { let _ = std::fs::remove_file(&self.path); } } } // WHAT: delete the opened key. WHY: it opens every backup; even a panic removes it.

// ---------------------------------------------------------------- the settings, the command line and the night

/// The job the settings describe: the data folder, the work folder and the target. `None` when either setting is empty.
pub fn job_from(product: &'static Product, data: PathBuf, work: Option<&str>, target: Option<&str>, version: &str) -> Result<Option<Job<'static>>> {
    let (Some(work), Some(spec)) = (work.filter(|w| !w.trim().is_empty()), target.filter(|t| !t.trim().is_empty())) else { return Ok(None) }; // WHAT: both set, or off. WHY: off until someone sets it up.
    let work = std::path::absolute(work).with_context(|| format!("the work folder {work}"))?; // WHAT: the work folder as a full path. WHY: age and rclone are run by their full path, never looked up on the PATH.
    let target = Target::parse(spec, &work)?; // WHAT: where it goes. WHY: a malformed target is refused before anything runs.
    let age = work.join("bin").join(if cfg!(windows) { "age.exe" } else { "age" }); // WHAT: age beside the job. WHY: never whatever is on the PATH.
    Ok(Some(Job { product, data, work, target, cipher: Cipher::Age(age), version: version.to_string(), mark: true })) // WHAT: the job, marking. WHY: the same for the night, the command and the drill; the command clears `mark` for another target.
}

/// The time of night, "1:30" or "01:30", 24-hour, the server's own clock.
pub fn night_time(at: &str) -> Result<chrono::NaiveTime> {
    let t = at.trim(); // WHAT: without spaces. WHY: a setting typed with a space still reads.
    let ok = t.len() >= 4 && t.len() <= 5 && t.as_bytes()[t.len() - 3] == b':' && t.chars().filter(|c| *c != ':').all(|c| c.is_ascii_digit()); // WHAT: H:MM or HH:MM only. WHY: "1.30" or "1:30am" is refused, not guessed.
    let parsed = if ok { chrono::NaiveTime::parse_from_str(t, "%H:%M").ok() } else { None }; // WHAT: read it. WHY: 25:00 or 1:61 is refused.
    parsed.ok_or_else(|| anyhow!("HYPERVIEW_OFFSITE_AT is \"{at}\", which isn't a time of day as hours and minutes, 24-hour (\"01:30\")")) // WHAT: say what is wanted. WHY: found as the server starts, not at night.
}

/// Checks the off-site settings as the server starts: a typo is found now, not at night.
pub fn check_settings(data: &Path, work: Option<&str>, target: Option<&str>, at: &str) -> Result<()> {
    night_time(at)?; // WHAT: the time reads. WHY: the night must come.
    let target = target.filter(|t| !t.trim().is_empty()); // WHAT: the target, if set. WHY: without one the backup is off.
    let Some(spec) = target else { return Ok(()) }; // WHAT: off. WHY: nothing else to check.
    let Some(work) = work.filter(|w| !w.trim().is_empty()) else { bail!("HYPERVIEW_OFFSITE_TARGET is set, so HYPERVIEW_OFFSITE_WORK must be too: a folder apart from the data folder for the backup's programs, key and log") }; // WHAT: refuse a half setting. WHY: the backup can't run without its folder.
    let work = std::path::absolute(work)?; // WHAT: the full path. WHY: compared below.
    if inside(&work, data) || inside(data, &work) { bail!("HYPERVIEW_OFFSITE_WORK ({}) must be apart from the data folder ({}): a backup inside what it backs up grows forever", work.display(), data.display()); } // WHAT: refuse overlap. WHY: the spool would back itself up.
    Target::parse(spec, &work)?; // WHAT: the target reads. WHY: a malformed one is refused now.
    Ok(())
}

/// Whether tonight's backup is due: past the time, none confirmed since it, and no failed try in the last hour.
pub fn due(now: chrono::NaiveDateTime, at: chrono::NaiveTime, last_good: Option<chrono::NaiveDateTime>, last_try: Option<chrono::NaiveDateTime>) -> bool {
    let from = now.date().and_time(at); // WHAT: today at the time. WHY: once a day, from then on.
    if now < from { return false; } // WHAT: not yet. WHY: last night's covers until then.
    if last_good.is_some_and(|g| g >= from) { return false; } // WHAT: done today. WHY: one a day.
    !last_try.is_some_and(|t| now - t < chrono::Duration::hours(1)) // WHAT: a failed try under an hour ago waits. WHY: tried again gently, not every look.
}

/// How often the server looks whether tonight's backup is due.
pub const LOOK_EVERY: std::time::Duration = std::time::Duration::from_secs(5 * 60); // WHAT: five minutes. WHY: close enough to the time, and nothing to a server.

/// Tonight's backup, run by the server itself: once a day, the first look after the time, retried an hour after a failure.
pub async fn nightly(server: crate::api::Shared) {
    let Ok(at) = night_time(&server.config.offsite_at) else { return }; // WHAT: the time. WHY: checked as the server started; never guessed.
    let mut last_try: Option<chrono::NaiveDateTime> = None; // WHAT: the last failed run. WHY: retried an hour later.
    loop {
        tokio::time::sleep(LOOK_EVERY).await; // WHAT: wait between looks. WHY: never a busy loop.
        let now = chrono::Local::now().naive_local(); // WHAT: the office's clock. WHY: "01:30" means the office's half past one.
        let good = report(&server.config.data).last_good.and_then(|g| chrono::DateTime::parse_from_rfc3339(&g).ok()).map(|g| g.with_timezone(&chrono::Local).naive_local()); // WHAT: the last confirmed backup, on the office's clock. WHY: compared with tonight's time.
        if !due(now, at, good, last_try) { continue; } // WHAT: not due. WHY: once a night.
        let job = match job_from(&VIEW, server.config.data.clone(), server.config.offsite_work.as_deref(), server.config.offsite_target.as_deref(), crate::VERSION) { Ok(Some(j)) => j, Ok(None) => return, Err(e) => { tracing::error!("the off-site backup's settings don't read: {e:#}"); return; } }; // WHAT: the job. WHY: the settings were checked as the server started.
        last_try = Some(now); // WHAT: note the try. WHY: a failure waits an hour.
        match tokio::task::spawn_blocking(move || job.backup()).await { // WHAT: run it off the request threads. WHY: it reads and sends for minutes or hours.
            Ok(e) if e.code == 0 || e.code == 5 => tracing::info!("tonight's off-site backup ended with exit {} (0 good, 5 good with a warning)", e.code), // WHAT: say so in the server's log. WHY: beside the job's own log.
            Ok(e) => tracing::error!("tonight's off-site backup did not finish (exit {}): {}. Nothing is marked; it tries again in an hour", e.code, e.why), // WHAT: say why. WHY: Fleet's backupDays also stays old until one passes.
            Err(e) => tracing::error!("tonight's off-site backup stopped part way: {e}"), // WHAT: a panic in the job. WHY: the server keeps serving.
        }
    }
}

/// Whether a run to `given` stands for the server's own off-site copy: only when it is the configured target.
pub fn marks(configured: Option<&str>, given: Option<&str>) -> bool {
    matches!((configured.map(str::trim), given.map(str::trim)), (Some(c), Some(g)) if !c.is_empty() && c == g) // WHAT: the same target as the setting. WHY: anything else is a hand run elsewhere.
}

/// `--offsite-backup [TARGET]` and `--offsite-drill [TARGET] --identity KEY --scratch DIR [...]`, with the server's settings.
pub fn main_from_args(product: &'static Product, config: &crate::Config, version: &str, args: &[String]) -> i32 {
    let usage = "usage: hyperview-server --offsite-backup [TARGET] [--work DIR] [--data DIR]\n       hyperview-server --offsite-drill [TARGET] --identity KEYFILE --scratch EMPTYDIR [--work DIR] [--data DIR] [--object daily/NAME] [--full] [--sample N] [--keep]"; // WHAT: the two forms. WHY: printed on a mistake.
    let Some(mode) = args.first().filter(|m| *m == "--offsite-backup" || *m == "--offsite-drill") else { eprintln!("{usage}"); return 4; }; // WHAT: which command. WHY: refuse anything else.
    let mut opt: BTreeMap<String, String> = BTreeMap::new(); let mut flags = BTreeSet::new(); let mut spec: Option<String> = None; let mut i = 1; // WHAT: the options. WHY: read below.
    while i < args.len() { let a = &args[i]; if ["--full", "--keep"].contains(&a.as_str()) { flags.insert(a.clone()); i += 1; } else if ["--data", "--work", "--identity", "--scratch", "--object", "--sample"].contains(&a.as_str()) && i + 1 < args.len() { opt.insert(a.clone(), args[i + 1].clone()); i += 2; } else if i == 1 && !a.starts_with("--") { spec = Some(a.clone()); i += 1; } else { eprintln!("unknown or incomplete option {a}\n{usage}"); return 4; } } // WHAT: a target first, then --name value pairs and two flags. WHY: a mistyped option must not be ignored.
    let data = opt.get("--data").map(PathBuf::from).unwrap_or_else(|| config.data.clone()); // WHAT: the data folder the server uses, or the one given. WHY: the installed program finds its own drawings.
    let work = opt.get("--work").cloned().or_else(|| config.offsite_work.clone()); // WHAT: the work folder given, or the setting. WHY: the same as the night's.
    let target = spec.or_else(|| config.offsite_target.clone()); // WHAT: the target given, or the setting. WHY: a person can try another, such as a USB disk.
    let job = match job_from(product, data, work.as_deref(), target.as_deref(), version) { Ok(Some(j)) => j, Ok(None) => { eprintln!("The off-site backup isn't set up: HYPERVIEW_OFFSITE_TARGET and HYPERVIEW_OFFSITE_WORK are empty (README.md, Off-site backups)."); return 4; } Err(e) => { eprintln!("{e:#}"); return 4; } }; // WHAT: the job, or why not. WHY: say what is missing.
    let mut job = job; job.mark = marks(config.offsite_target.as_deref(), target.as_deref()) && opt.get("--data").is_none_or(|d| Path::new(d) == config.data); // WHAT: mark only the server's own target and data. WHY: a backup by hand to a USB disk must not move backupDays.
    let exit = if mode == "--offsite-backup" { job.backup() } else { // WHAT: the backup now, or the drill. WHY: the two commands.
        let (Some(id), Some(scratch)) = (opt.get("--identity"), opt.get("--scratch")) else { eprintln!("{usage}"); return 4; }; // WHAT: the key and the scratch folder. WHY: both required.
        let sample = match opt.get("--sample").map(|s| s.parse::<usize>()) { None => 20, Some(Ok(n)) => n, Some(Err(_)) => { eprintln!("--sample takes a number\n{usage}"); return 4; } }; // WHAT: how many files a sample drill checks. WHY: 20 by default; a typo is refused.
        Drill { job, identity: PathBuf::from(id), scratch: PathBuf::from(scratch), object: opt.get("--object").cloned(), full: flags.contains("--full"), sample, keep: flags.contains("--keep") }.run() // WHAT: run it. WHY: Creede, with the key on a USB stick.
    };
    if exit.code != 0 && exit.code != 5 && !exit.why.is_empty() { eprintln!("{}", exit.why); } // WHAT: the reason on failure. WHY: a person running it sees why.
    exit.code
}

#[cfg(test)]
mod tests;
