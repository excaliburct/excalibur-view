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

/// Finds one real record and its file's digest: what the drill opens.
pub type RecordFn = fn(&Connection) -> Result<Option<(String, String)>>; // WHAT: the record finder's type. WHY: named, so the product's shape reads plainly.

/// One product's shape: what its data folder holds and how to check it.
pub struct Product {
    pub id: &'static str, // WHAT: the short name, in object names. WHY: one bucket can hold several products' folders.
    pub database: &'static str, // WHAT: the SQLite file in the data folder. WHY: copied consistently each night.
    pub blobs: &'static str,    // WHAT: the folder of files kept by digest. WHY: sent once each.
    pub counts: &'static [&'static str], // WHAT: the tables counted. WHY: the drill compares them with the manifest.
    pub references: fn(&Connection) -> Result<BTreeSet<String>>, // WHAT: every digest the database points at. WHY: each must be in the backup.
    pub record: RecordFn, // WHAT: one real record and its file's digest. WHY: the drill opens it.
}

/// Excalibur View's shape.
pub const VIEW: Product = Product {
    id: "view",                   // WHAT: objects are named view-…. WHY: says whose they are.
    database: "hyperview.sqlite", // WHAT: the store's file. WHY: Config::database.
    blobs: "files",               // WHAT: the drawings by digest. WHY: Config::blobs.
    counts: &[
        "people", "projects", "sets", "sheets", "markups", "chests", "folders", "files", "audit", "cad_drawings", "cad_versions", "arch_jobs", "arch_checkins",
    ], // WHAT: what the drill counts, CAD's drawings and versions and Archetype's jobs and check-ins too. WHY: the records a shop would miss.
    references: view_references,  // WHAT: the digest columns. WHY: below.
    record: view_record,          // WHAT: the newest drawing set. WHY: the record the drill opens.
};

fn view_references(db: &Connection) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new(); // WHAT: the digests. WHY: a set, so a shared file counts once.
    for (table, column) in [
        ("sets", "digest"),
        ("chests", "digest"),
        ("plugins", "digest"),
        ("files", "digest"),
        ("cad_versions", "sha256"),
        ("arch_files", "digest"),
    ] {
        // WHAT: every table holding a file's digest. WHY: releases are left out: a server fetches those again from the feed.
        let sql = format!("SELECT {column} FROM {table}"); // WHAT: the column. WHY: names are constants above, never input.
        let mut stmt = db.prepare(&sql)?; // WHAT: prepare it. WHY: run once per table.
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?; // WHAT: each value. WHY: collected below.
        for d in rows {
            out.insert(d?);
        } // WHAT: keep it. WHY: the set of files the database needs.
    }
    Ok(out) // WHAT: all of them. WHY: the backup checks each is on disk and in the bucket.
}

fn view_record(db: &Connection) -> Result<Option<(String, String)>> {
    use rusqlite::OptionalExtension; // WHAT: a query that may find nothing. WHY: an empty server has no set to open.
    Ok(db
        .query_row(
            "SELECT id, digest FROM sets ORDER BY uploaded DESC, id LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?) // WHAT: the newest set's id and file. WHY: the newest is the one most likely to be missing if anything is.
}

// ---------------------------------------------------------------- names and the keep rule

const KEEP: [(&str, usize); 3] = [("daily", 7), ("weekly", 4), ("monthly", 12)]; // WHAT: how many of each are kept. WHY: Creede's rule, the same as Arc Valley's.
const EXT: &str = ".tar.gz.age"; // WHAT: the catalog's ending. WHY: says what it is: a gzipped tar, encrypted with age.

/// The catalog's name for a moment, in UTC, to the second.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn object_name(product: &str, t: time::OffsetDateTime) -> String {
    let t = t.to_offset(time::UtcOffset::UTC); // WHAT: in UTC. WHY: UTC never repeats an hour, so two runs never share a name.
    format!(
        "{product}-{:04}{:02}{:02}-{:02}{:02}{:02}{EXT}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    ) // WHAT: product-yyyymmdd-hhmmss. WHY: sorts by date.
}

/// The moment a name stands for, or `None` for anything that isn't exactly one of ours.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn parse_name(product: &str, name: &str) -> Option<time::OffsetDateTime> {
    let rest = name
        .strip_prefix(product)?
        .strip_prefix('-')?
        .strip_suffix(EXT)?; // WHAT: the date part. WHY: a stranger's name comes back None.
    let b = rest.as_bytes(); // WHAT: its bytes. WHY: checked one by one.
    if b.len() != 15
        || b[8] != b'-'
        || !b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 8 || c.is_ascii_digit())
    {
        return None;
    } // WHAT: exactly yyyymmdd-hhmmss. WHY: nothing else is ours.
    let n = |a: usize, z: usize| rest[a..z].parse::<u32>().ok(); // WHAT: a number from the name. WHY: each field below.
    let byte = |a: usize, z: usize| u8::try_from(n(a, z)?).ok(); // WHAT: a two-digit field as a byte. WHY: two digits always fit, so nothing is cut as the cast could
    let month = time::Month::try_from(byte(4, 6)?).ok()?; // WHAT: the month. WHY: a 13th month is refused.
    let date = time::Date::from_calendar_date(i32::try_from(n(0, 4)?).ok()?, month, byte(6, 8)?).ok()?; // WHAT: the day, in its four-digit year. WHY: a 32nd day is refused, not rolled over.
    let t = time::Time::from_hms(byte(9, 11)?, byte(11, 13)?, byte(13, 15)?).ok()?; // WHAT: the time. WHY: the same.
    Some(time::PrimitiveDateTime::new(date, t).assume_utc()) // WHAT: the moment. WHY: names are written in UTC.
}

/// Whether `name` is one of ours and dated no more than a day after `now`. A name from the future is a clock gone wrong or a
/// planted object: seven "2099" names must never push the real catalogs out of the keep, or be the one a drill picks.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn believable(product: &str, name: &str, now: time::OffsetDateTime) -> bool {
    parse_name(product, name).is_some_and(|t| t <= now + time::Duration::days(1))
    // WHAT: ours, and not from the future. WHY: as above.
}

/// What to copy to weekly/ and monthly/, and what to delete, once tonight's catalog is safe.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub weekly: bool, // WHAT: copy tonight's to weekly/. WHY: the first of its ISO week.
    pub monthly: bool, // WHAT: copy tonight's to monthly/. WHY: the first of its month.
    pub deletes: Vec<(String, String)>, // WHAT: (folder, name) past the keep. WHY: never tonight's, never a stranger's.
}

/// The keep rule, on its own so the tests can attack it.
///
/// # Errors
///
/// Fails when the rule can't keep tonight's copy, so nothing is deleted.
pub fn plan(product: &str, listing: &BTreeMap<String, Vec<String>>, tonight: &str) -> Result<Plan> {
    let when =
        parse_name(product, tonight).ok_or_else(|| anyhow!("not a catalog name: {tonight}"))?; // WHAT: tonight's date. WHY: the plan never runs on a name it can't date.
    let ours = |f: &str| -> Vec<String> {
        listing
            .get(f)
            .map(|v| {
                v.iter()
                    .filter(|n| believable(product, n, when))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }; // WHAT: our catalogs in a folder, none dated past tomorrow. WHY: sidecars, strangers and planted far-future names are never counted or deleted.
    let week = |t: time::OffsetDateTime| t.to_iso_week_date(); // WHAT: (year, week, day). WHY: weeks start on Monday and belong to their Thursday's year.
    let weekly = !ours("weekly")
        .iter()
        .filter_map(|n| parse_name(product, n))
        .any(|t| {
            let (y, w, _) = week(t);
            let (y2, w2, _) = week(when);
            y == y2 && w == w2
        }); // WHAT: this week has none yet. WHY: the first good one stands for the week.
    let monthly = !ours("monthly")
        .iter()
        .filter_map(|n| parse_name(product, n))
        .any(|t| t.year() == when.year() && t.month() == when.month()); // WHAT: this month has none yet. WHY: the same, for the month.
    let mut deletes = Vec::new(); // WHAT: what goes. WHY: filled per folder.
    for (folder, keep) in KEEP {
        // WHAT: each folder. WHY: each keeps its own count.
        let mut names = ours(folder); // WHAT: our catalogs there. WHY: only ours are pruned.
        let adds =
            folder == "daily" || (folder == "weekly" && weekly) || (folder == "monthly" && monthly); // WHAT: tonight's lands here. WHY: it counts toward the keep.
        if adds && !names.iter().any(|n| n == tonight) {
            names.push(tonight.to_string());
        } // WHAT: count it. WHY: keeping 7 includes tonight's.
        names.sort();
        names.reverse(); // WHAT: newest first. WHY: names sort by date.
        for n in names.into_iter().skip(keep) {
            if n != tonight {
                deletes.push((folder.to_string(), n));
            }
        } // WHAT: past the keep, never tonight's. WHY: the copy just made survives whatever the clock says.
    }
    Ok(Plan {
        weekly,
        monthly,
        deletes,
    }) // WHAT: the plan. WHY: the job carries it out.
}

// ---------------------------------------------------------------- the public half only

/// Why someone other than the system, administrators or this account could change anything in the work folder, or
/// `None`. Every entry is checked, no link followed. Unix: owned by root or this account, no group or other write, and
/// rclone.conf readable by its owner only. Windows: owned by the system, administrators or this account, and no change
/// right for anyone else.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn others_can_change(work: &Path) -> Option<String> {
    let mut stack = vec![work.to_path_buf()]; // WHAT: entries still to look at. WHY: the whole tree, not only the folder, bin and rclone.conf.
    while let Some(p) = stack.pop() {
        let m = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) if p == work && e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => return Some(format!("{} can't be looked at ({e})", p.display())),
        }; // WHAT: the entry itself, never what a link points at. WHY: a missing work folder is said later, as age missing; anything else unreadable is refused.
        if m.file_type().is_symlink() {
            return Some(format!("{} is a link; nothing in the work folder may point elsewhere: replace it with the file itself", p.display()));
        } // WHAT: refuse links. WHY: a link can be pointed at a file another account owns.
        #[cfg(windows)]
        if std::os::windows::fs::MetadataExt::file_attributes(&m) & 0x400 != 0 {
            return Some(format!(
                "{} is a reparse point; nothing in the work folder may point elsewhere",
                p.display()
            ));
        } // WHAT: refuse junctions. WHY: the same.
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let (uid, mode) = (m.uid(), m.permissions().mode() & 0o777); // WHAT: its owner and mode. WHY: judged below.
            if uid != 0 && uid != my_uid() {
                return Some(format!("{} is owned by account {uid}, not root or this server's account ({}): chown it to one of them", p.display(), my_uid()));
            } // WHAT: a stranger's file. WHY: its owner can change it back at any time.
            if mode & 0o022 != 0 {
                return Some(format!(
                    "{} can be changed by other accounts (mode {mode:o}): chmod go-w it",
                    p.display()
                ));
            } // WHAT: group or other write. WHY: another account could swap age or the bucket's settings.
            if p == work.join("rclone.conf") && mode & 0o077 != 0 {
                return Some(format!("{} can be read by other accounts (mode {mode:o}) and holds the bucket's key: chmod 600 it", p.display()));
            }
        } // WHAT: the bucket's key readable by others. WHY: anyone with it can delete the backups.
        #[cfg(windows)]
        if let Some(why) = winacl::others_can_change(&p) {
            return Some(why);
        } // WHAT: the owner and the permissions list. WHY: the same, on Windows.
        if m.is_dir() {
            match std::fs::read_dir(&p) {
                Ok(rd) => {
                    for e in rd {
                        match e {
                            Ok(e) => stack.push(e.path()),
                            Err(e) => return Some(format!("{} can't be read ({e})", p.display())),
                        }
                    }
                }
                Err(e) => return Some(format!("{} can't be read ({e})", p.display())),
            }
        } // WHAT: look inside. WHY: a file planted anywhere is found; what can't be read isn't trusted.
    }
    None
}

/// The account this server runs as (Unix).
#[cfg(unix)]
fn my_uid() -> u32 {
    #[cfg(test)]
    if let Some(u) = PRETEND_UID.with(std::cell::Cell::get) { // WHAT: the account a test pretends to be. WHY: read from its cell
        return u;
    } // WHAT: the tests' other account. WHY: proves a stranger's file is refused, without being root.
    nix::unistd::getuid().as_raw() // WHAT: this process's account. WHY: its own files are trusted.
}

/// Locks the work folder to the system, administrators and this account, nothing inherited (Windows), as the server starts.
pub fn lock_work(work: &Path) {
    match std::fs::create_dir_all(work) { // WHAT: make it if missing. WHY: locked before anything is put in it.
        Err(e) => tracing::warn!("could not make the off-site work folder {}: {e}", work.display()), // WHAT: say so. WHY: the backup then says what is missing.
        #[cfg(windows)]
        Ok(()) => match winacl::me().and_then(|me| winacl::lock(work, &[winacl::SYSTEM, winacl::ADMINISTRATORS, &me])) { Ok(()) => tracing::info!("{} is the system's, administrators' and this server's only", work.display()), Err(e) => tracing::warn!("could not lock the off-site work folder {w}: {e:#}. The backup refuses to run until it is fixed. From a command prompt opened as administrator: icacls \"{w}\" /setowner *S-1-5-32-544 /T /C, then icacls \"{w}\" /inheritance:r /grant:r *S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F, then icacls \"{w}\\*\" /reset /T /C; then run the backup by hand to check", w = work.display()) }, // WHAT: a protected permissions list. WHY: ProgramData lets every account add files.
        #[cfg(not(windows))]
        Ok(()) => {} // WHAT: nothing more off Windows. WHY: the backup checks the folder's mode itself.
    }
}

/// Windows folder permissions through the system's own calls.
#[cfg(windows)]
mod winacl {
    use anyhow::{anyhow, bail, Result}; // WHAT: errors. WHY: said in the log.
    use std::path::Path; // WHAT: paths. WHY: the folders.
    use std::ptr::{null, null_mut}; // WHAT: empty pointers. WHY: the calls' optional parts.
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE}; // WHAT: handles and freeing. WHY: everything the calls hand back is freed.
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        GetNamedSecurityInfoW, ProgressInvokeNever, TreeSetNamedSecurityInfoW, SDDL_REVISION_1,
        SE_FILE_OBJECT, TREE_SEC_INFO_RESET,
    }; // WHAT: permissions by name and in text. WHY: read and set.
    use windows_sys::Win32::Security::{
        AclSizeInformation, GetAce, GetAclInformation, GetSecurityDescriptorControl,
        GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, GetTokenInformation, TokenUser,
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION,
        INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SE_DACL_PROTECTED, TOKEN_QUERY,
        TOKEN_USER,
    }; // WHAT: the security calls and shapes. WHY: as above.
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW; // WHAT: make a folder. WHY: with its permissions already in place.
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken}; // WHAT: this process. WHY: whose account it runs as.

    pub const SYSTEM: &str = "S-1-5-18"; // WHAT: the system account. WHY: a well-known SID, any language.
    pub const ADMINISTRATORS: &str = "S-1-5-32-544"; // WHAT: the administrators group. WHY: the same.
    const CHANGE: u32 = 0x2
        | 0x4
        | 0x10
        | 0x40
        | 0x100
        | 0x1_0000
        | 0x4_0000
        | 0x8_0000
        | 0x1000_0000
        | 0x4000_0000; // WHAT: write data (add file), append (add folder), write extended attributes, delete child, write attributes, delete, change permissions, take ownership, generic all, generic write. WHY: any of them lets an account swap or alter what is there.

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().chain(Some(0)).collect()
    } // WHAT: a NUL-ended wide string. WHY: what the calls take.

    /// A SID as text (S-1-…), freed after. `sid` is one the system handed back, or null.
    fn sid_text(sid: PSID) -> Option<String> {
        if sid.is_null() {
            return None;
        } // WHAT: no SID. WHY: nothing to convert.
          // SAFETY: `sid` points at a SID the system wrote (a token's or a security descriptor's, still held by the caller);
          // ConvertSidToStringSidW allocates a NUL-ended string that is read up to its NUL and then freed once with LocalFree.
        unsafe {
            let mut w: windows_sys::core::PWSTR = null_mut(); // WHAT: the text the system makes. WHY: freed below.
            if ConvertSidToStringSidW(sid, &mut w) == 0 || w.is_null() {
                return None;
            } // WHAT: convert. WHY: compared as text.
            let mut n = 0;
            while *w.add(n) != 0 {
                n += 1;
            } // WHAT: its length. WHY: NUL-ended.
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(w, n));
            if !LocalFree(w as _).is_null() {
                return None;
            }
            Some(s) // WHAT: copy, then free; a failed free is a refusal. WHY: no leak, nothing ignored.
        }
    }

    /// The SID of the account this program runs as.
    pub fn me() -> Result<String> {
        // SAFETY: the token handle is this process's own, opened for query and closed once; the buffer is u64-aligned and
        // as large as GetTokenInformation said, so the TOKEN_USER it writes there is read in bounds.
        let sid = unsafe {
            let mut tok: HANDLE = null_mut(); // WHAT: this process's token. WHY: it names the account.
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) == 0 {
                bail!("could not read this program's own account");
            } // WHAT: open it. WHY: read only.
            let mut need = 0u32;
            let _sizing = GetTokenInformation(tok, TokenUser, null_mut(), 0, &mut need); // WHAT: the size needed; this call fails by design, its answer is `need`. WHY: asked first.
            if need == 0 {
                CloseHandle(tok);
                bail!("could not read this program's own account");
            } // WHAT: no size is a refusal. WHY: nothing ignored.
            let mut buf = vec![0u64; (need as usize).div_ceil(8).max(1)]; // WHAT: room for it, aligned. WHY: TOKEN_USER holds a pointer.
            let ok = GetTokenInformation(
                tok,
                TokenUser,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
                &mut need,
            );
            let closed = CloseHandle(tok); // WHAT: read it, close the token. WHY: no leak.
            if ok == 0 || closed == 0 {
                bail!("could not read this program's own account");
            } // WHAT: refuse either failure. WHY: never guess whose folder it is.
            sid_text((*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid) // WHAT: the SID, while the buffer still holds it. WHY: compared and granted.
        };
        sid.ok_or_else(|| anyhow!("this program's account has no SID")) // WHAT: refuse none. WHY: the same.
    }

    /// Builds a security descriptor from SDDL text and hands it to `with`, freeing it after.
    fn with_descriptor<T>(
        sddl: &str,
        with: impl FnOnce(PSECURITY_DESCRIPTOR) -> Result<T>,
    ) -> Result<T> {
        let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut sd: PSECURITY_DESCRIPTOR = null_mut(); // WHAT: the permissions in SDDL. WHY: one well-checked parser.
                                                       // SAFETY: `text` is NUL-ended and outlives the call; the descriptor the system allocates is used only inside `with`
                                                       // and freed once with LocalFree afterwards, whatever `with` returned.
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                null_mut(),
            ) == 0
            {
                bail!("could not build the permissions {sddl}");
            } // WHAT: parse. WHY: refused if malformed.
            let r = with(sd);
            if !LocalFree(sd as _).is_null() {
                bail!("could not free the permissions {sddl}");
            }
            r // WHAT: use it, then free; a failed free is a refusal. WHY: no leak, nothing ignored.
        }
    }

    fn aces(sids: &[&str]) -> String {
        sids.iter().map(|s| format!("(A;OICI;FA;;;{s})")).collect()
    } // WHAT: full control, inherited by everything inside, for each. WHY: files made inside get the same.

    /// Makes a new folder that is born protected: owned by `owner`, only `sids` allowed, nothing inherited.
    pub fn create_dir(path: &Path, owner: &str, sids: &[&str]) -> Result<()> {
        let name = wide(path.as_os_str()); // WHAT: the folder's name. WHY: NUL-ended for the call.
        with_descriptor(&format!("O:{owner}D:P{}", aces(sids)), |sd| {
            // WHAT: the folder's permissions. WHY: in place at the moment it exists.
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd,
                bInheritHandle: 0,
            }; // WHAT: handed to CreateDirectoryW. WHY: no gap where it is open.
               // SAFETY: `name` is NUL-ended and `sa` points at a live descriptor for the length of the call.
            let ok = unsafe { CreateDirectoryW(name.as_ptr(), &sa) }; // WHAT: make it. WHY: born protected.
            if ok == 0 {
                bail!(
                    "making {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                );
            }
            Ok(()) // WHAT: refuse. WHY: an existing folder is never reused.
        })
    }

    /// Resets the whole tree under `path`: owner Administrators, `sids` only on the folder, protected, and everything inside
    /// inheriting from it alone.
    pub fn lock(path: &Path, sids: &[&str]) -> Result<()> {
        let name = wide(path.as_os_str()); // WHAT: the folder's name. WHY: NUL-ended for the call.
        with_descriptor(&format!("O:BAD:P{}", aces(sids)), |sd| {
            // WHAT: the new owner and list. WHY: as text, parsed by the system.
            let (mut present, mut defaulted, mut dacl, mut owner, mut owner_defaulted): (
                i32,
                i32,
                *mut ACL,
                PSID,
                i32,
            ) = (0, 0, null_mut(), null_mut(), 0); // WHAT: the owner and list inside it. WHY: what TreeSetNamedSecurityInfoW takes.
                                                   // SAFETY: `sd` is live for this closure, the owner and DACL pointers taken from it are used only within it, `name`
                                                   // is NUL-ended, and no progress callback is given.
            let r = unsafe {
                if GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) == 0
                    || present == 0
                    || GetSecurityDescriptorOwner(sd, &mut owner, &mut owner_defaulted) == 0
                    || owner.is_null()
                {
                    1
                } else {
                    TreeSetNamedSecurityInfoW(
                        name.as_ptr(),
                        SE_FILE_OBJECT,
                        OWNER_SECURITY_INFORMATION
                            | DACL_SECURITY_INFORMATION
                            | PROTECTED_DACL_SECURITY_INFORMATION,
                        owner,
                        null_mut(),
                        dacl,
                        null(),
                        TREE_SEC_INFO_RESET,
                        None,
                        ProgressInvokeNever,
                        null(),
                    )
                }
            }; // WHAT: the whole tree: owner, list, inheritance off at the top. WHY: a file planted before setup keeps no owner or entry of its own.
            if r != 0 {
                bail!("could not set the owner and permissions of {} and everything in it (error {r})", path.display());
            }
            Ok(()) // WHAT: say so. WHY: the backup then refuses.
        })
    }

    /// A permissions list read back: the owner, whether it is protected, and each entry (type, flags, rights, SID).
    pub type Perms = (Option<String>, bool, Vec<(u8, u8, u32, String)>); // WHAT: what read returns. WHY: named, so its users read plainly.

    pub fn read(path: &Path) -> Result<Perms> {
        let name = wide(path.as_os_str()); // WHAT: the name. WHY: NUL-ended for the call.
                                           // SAFETY: `name` is NUL-ended; every pointer read (owner, DACL, each ACE and its SID) lies inside the security
                                           // descriptor the system allocated for this call, which is freed once with LocalFree after the last read.
        unsafe {
            let (mut owner, mut dacl, mut sd): (PSID, *mut ACL, PSECURITY_DESCRIPTOR) =
                (null_mut(), null_mut(), null_mut()); // WHAT: what the system hands back. WHY: read below.
            let r = GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut sd,
            ); // WHAT: owner and list. WHY: what decides who can change it.
            if r != 0 {
                bail!(
                    "could not read the permissions of {} (error {r})",
                    path.display()
                );
            } // WHAT: refuse. WHY: unknown is not safe.
            let mut control = 0u16;
            let mut rev = 0u32;
            let read_flags = GetSecurityDescriptorControl(sd, &mut control, &mut rev) != 0; // WHAT: its flags. WHY: whether inheritance is off.
            let mut out = Vec::new();
            let mut ok = read_flags && !dacl.is_null(); // WHAT: the entries; unreadable flags or no list at all is a refusal. WHY: refused below.
            if ok {
                let mut info = ACL_SIZE_INFORMATION {
                    AceCount: 0,
                    AclBytesInUse: 0,
                    AclBytesFree: 0,
                };
                ok = GetAclInformation(
                    dacl,
                    (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                    std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                ) != 0; // WHAT: how many entries. WHY: each is read.
                for i in 0..info.AceCount {
                    let mut ace = null_mut();
                    if GetAce(dacl, i, &mut ace) == 0 {
                        ok = false;
                        break;
                    }
                    let h = *(ace as *const ACE_HEADER); // WHAT: one entry's header. WHY: its kind and flags.
                    let (mask, sid) = if h.AceType == 0 {
                        let a = ace as *const ACCESS_ALLOWED_ACE;
                        (
                            (*a).Mask,
                            sid_text(std::ptr::addr_of!((*a).SidStart) as PSID).unwrap_or_default(),
                        )
                    } else {
                        (0, String::new())
                    }; // WHAT: an allow entry's rights and SID. WHY: others are judged by kind alone.
                    out.push((h.AceType, h.AceFlags, mask, sid));
                }
            } // WHAT: keep it. WHY: returned.
            let who = sid_text(owner);
            if !LocalFree(sd as _).is_null() {
                ok = false;
            } // WHAT: the owner, then free; a failed free is a refusal. WHY: no leak, nothing ignored.
            if !ok {
                bail!("{} has no readable permissions list", path.display());
            } // WHAT: refuse. WHY: an empty or unreadable list lets everyone in.
            Ok((who, control & SE_DACL_PROTECTED != 0, out))
        }
    }

    /// Why someone other than the system, administrators or this account could change `path`, or `None`.
    pub fn others_can_change(path: &Path) -> Option<String> {
        let me = match me() {
            Ok(m) => m,
            Err(e) => return Some(format!("{e:#}")),
        };
        let trusted = [SYSTEM, ADMINISTRATORS, me.as_str()]; // WHAT: who may. WHY: the server's own account and those who run the machine.
        let (owner, _, list) = match read(path) {
            Ok(r) => r,
            Err(e) => return Some(format!("{e:#}")),
        }; // WHAT: its owner and list. WHY: judged below.
        if !owner.as_deref().is_some_and(|o| trusted.contains(&o)) {
            return Some(format!(
                "{} is owned by {}, who can change its permissions back",
                path.display(),
                owner.unwrap_or_else(|| "nobody known".into())
            ));
        } // WHAT: the owner. WHY: an owner can always give itself access.
        for (kind, flags, mask, sid) in list {
            if u32::from(flags) & INHERIT_ONLY_ACE != 0 || kind == 1 {
                continue;
            } // WHAT: skip entries for what is inside, and denials. WHY: neither lets anyone change this one.
            if kind != 0 {
                return Some(format!(
                    "{} has a kind of permission this program doesn't read ({kind}); refusing it",
                    path.display()
                ));
            } // WHAT: anything unusual. WHY: unknown is not safe.
            if mask & CHANGE != 0 && !trusted.contains(&sid.as_str()) {
                return Some(format!("{} can be changed by {sid}", path.display()));
            }
        } // WHAT: a change right for anyone else. WHY: they could swap age or rclone.conf.
        None
    }

    /// Whether `path` is a protected folder owned by `owner` with only `sids` allowed.
    pub fn is_only(path: &Path, owner: &str, sids: &[&str]) -> Result<()> {
        let (o, protected, list) = read(path)?; // WHAT: its permissions. WHY: checked once it is made.
        if !protected {
            bail!("{} inherits permissions from its parent", path.display());
        } // WHAT: inheritance off. WHY: the parent's entries would let others in.
        if o.as_deref() != Some(owner) {
            bail!(
                "{} is owned by {}, not this account",
                path.display(),
                o.unwrap_or_default()
            );
        } // WHAT: the owner. WHY: the same.
        if let Some((k, _, _, sid)) = list
            .iter()
            .find(|(k, _, _, sid)| *k != 0 || !sids.contains(&sid.as_str()))
        {
            bail!("{} lets in {sid} (kind {k})", path.display());
        } // WHAT: only the allowed. WHY: the restore inherits this list.
        Ok(())
    }
}

/// The first file in the work folder holding an age private key, if any. `bin` (age's own program names the format) and
/// the job's logs are skipped, and so is anything over 64 KB: a key file is a few lines.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn private_key_in(work: &Path) -> Option<PathBuf> {
    let mut stack = vec![work.to_path_buf()]; // WHAT: folders still to look in. WHY: the whole work folder, not only the recipients file.
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        }; // WHAT: its entries. WHY: an unreadable folder holds nothing this can see.
        for e in rd.flatten() {
            let (path, name) = (e.path(), e.file_name().to_string_lossy().into_owned()); // WHAT: the entry. WHY: judged below.
            let Ok(m) = std::fs::symlink_metadata(&path) else {
                continue;
            }; // WHAT: the entry itself, never what a link points at. WHY: a link can't send the search outside.
            if m.is_dir() {
                if !(dir == work && name.eq_ignore_ascii_case("bin")) {
                    stack.push(path);
                }
                continue;
            } // WHAT: look inside folders but bin. WHY: age.exe holds the words of its own format.
            if !m.is_file() || m.len() > 64 * 1024 || name.starts_with("offsite.log") {
                continue;
            } // WHAT: small plain files only, never the log. WHY: a key file is small, and the log may quote a refusal.
            if std::fs::read(&path).is_ok_and(|b| {
                String::from_utf8_lossy(&b)
                    .to_ascii_uppercase()
                    .contains(&["AGE", "SECRET", "KEY", ""].join("-"))
            }) {
                return Some(path);
            } // WHAT: a private key's line. WHY: it would open every backup, and this machine is what they must survive.
        }
    }
    None
}

/// Reads the recipients file. Only age public keys (age1…) may be in it.
///
/// # Errors
///
/// Fails when the file has no keys or anything in it is not an age public key.
pub fn check_recipients(text: &str) -> Result<usize> {
    const CH: &str = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"; // WHAT: the letters an age key uses (bech32). WHY: anything else is a typo.
    if text.to_ascii_uppercase().contains("AGE-SECRET-KEY-") {
        bail!("the recipients file holds a PRIVATE key (AGE-SECRET-KEY-…). Only the public line (age1…) belongs on this server. Remove it now, and keep the private key with Creede, off this machine.");
    } // WHAT: refuse a private key. WHY: the key that opens every backup must never sit on the machine they are meant to survive.
    let keys: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect(); // WHAT: the key lines. WHY: age-keygen's comments are allowed.
    if let Some(bad) = keys.iter().find(|k| {
        !(k.len() == 62 && k.starts_with("age1") && k[4..].chars().all(|c| CH.contains(c)))
    }) {
        bail!("the recipients file has a line that is not an age public key (age1 and 58 letters and digits): {}…", bad.chars().take(12).collect::<String>());
    } // WHAT: refuse a bad line, naming only its start. WHY: a typo would fail at night, or encrypt to the wrong thing.
    if keys.is_empty() {
        bail!("the recipients file has no age public key in it (a line starting age1).");
    } // WHAT: refuse an empty file. WHY: someone must be able to open the backup.
    Ok(keys.len()) // WHAT: how many keys can open it. WHY: said in the log.
}

// ---------------------------------------------------------------- where it goes

/// Where backups go: a folder (a USB disk, a NAS, or the tests), or an rclone remote (R2, Backblaze B2).
pub enum Target {
    Folder(PathBuf), // WHAT: a plain folder. WHY: tests, and a second disk.
    Rclone {
        exe: PathBuf,
        config: PathBuf,
        remote: String,
    }, // WHAT: a cloud bucket through rclone. WHY: rclone speaks R2 and B2, and checks every upload.
}

#[cfg(test)]
thread_local! {
    /// The tests' bucket that says yes and keeps nothing: uploads under this folder are silently lost.
    pub(crate) static LOSE_UPLOADS: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; // WHAT: a folder whose uploads vanish. WHY: proves the read-back, not the upload's answer, decides what is marked.
}

fn key_ok(key: &str) -> bool {
    !key.is_empty()
        && key.split('/').all(|p| {
            !p.is_empty()
                && p != "."
                && p != ".."
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        }) // WHAT: folder/name of plain letters only. WHY: a name can never climb out of the target.
}

impl Target {
    /// `rclone:REMOTE:bucket/folder` for rclone (its program and settings in WORK), anything else a folder.
    ///
    /// # Errors
    ///
    /// Fails when the target is malformed.
    pub fn parse(spec: &str, work: &Path) -> Result<Target> {
        if let Some(remote) = spec.strip_prefix("rclone:") {
            // WHAT: an rclone remote. WHY: the cloud.
            if !remote.contains(':') || remote.contains("..") {
                bail!("an rclone target looks like rclone:r2:bucket/folder, not {spec}");
            } // WHAT: refuse a malformed remote. WHY: say so before anything runs.
            if !remote.starts_with(|c: char| c.is_ascii_alphanumeric())
                || remote.chars().any(|c| c.is_whitespace() || c.is_control())
            {
                bail!(
                    "an rclone remote starts with a letter or digit and has no spaces, not {spec}"
                );
            } // WHAT: refuse a remote rclone would read as an option. WHY: rclone:--config=... must never change how rclone runs.
            if remote.contains('=') || remote.contains(',') {
                bail!("an rclone target names a remote from rclone.conf; settings written into it (with = or ,) are refused");
            } // WHAT: refuse inline settings. WHY: a key written into the target would be shown in the log and the ops report as where the backup went.
            let exe = work.join("bin").join(if cfg!(windows) {
                "rclone.exe"
            } else {
                "rclone"
            }); // WHAT: rclone beside the job. WHY: never whatever is on the PATH.
            return Ok(Target::Rclone {
                exe,
                config: work.join("rclone.conf"),
                remote: remote.trim_end_matches('/').to_string(),
            }); // WHAT: the remote and its settings file. WHY: Creede makes rclone.conf on the box; this program never reads it, rclone does.
        }
        if spec.trim().is_empty() {
            bail!("no target given");
        } // WHAT: refuse nothing. WHY: a backup must go somewhere.
        Ok(Target::Folder(std::path::absolute(spec.trim())?)) // WHAT: a folder, by its full path. WHY: the tests, a USB disk or a NAS; compared and shown as one place, wherever this was started.
    }

    /// Where it went, in words, for the ops report. No keys, no account numbers.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn words(&self) -> String {
        match self {
            Target::Folder(p) => format!("folder {}", p.display()),
            Target::Rclone { remote, .. } => format!("off-site bucket {remote}"),
        } // WHAT: a short description. WHY: Fleet and the log say where.
    }

    fn rclone(&self, args: &[&str]) -> Result<String> {
        let Target::Rclone { exe, config, .. } = self else {
            bail!("not an rclone target")
        }; // WHAT: only for rclone. WHY: a folder needs none of this.
        let out = Command::new(exe)
            .arg("--config")
            .arg(config)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("running {}", exe.display()))?; // WHAT: run rclone with this job's settings. WHY: the service account has no rclone settings of its own.
        if !out.status.success() {
            bail!(
                "rclone {} failed: {}",
                args.first().unwrap_or(&""),
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .rev()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
        } // WHAT: its last lines on failure. WHY: the reason, not just a code.
        Ok(String::from_utf8_lossy(&out.stdout).into_owned()) // WHAT: its output. WHY: listings are JSON.
    }

    fn remote(&self, key: &str) -> Result<String> {
        if !key_ok(key) {
            bail!("refusing an object name: {key}");
        } // WHAT: check the name. WHY: never a path outside our folders.
        match self {
            Target::Rclone { remote, .. } => Ok(format!("{remote}/{key}")),
            Target::Folder(_) => bail!("not an rclone target"),
        } // WHAT: the full remote path. WHY: rclone's form.
    }

    fn folder_path(&self, key: &str) -> Result<PathBuf> {
        if !key_ok(key) {
            bail!("refusing an object name: {key}");
        } // WHAT: check the name. WHY: never a path outside the folder.
        match self {
            Target::Folder(root) => Ok(key.split('/').fold(root.clone(), |p, s| p.join(s))),
            Target::Rclone { .. } => bail!("not a folder target"),
        } // WHAT: the file's path. WHY: one folder per part.
    }

    /// Sends one file, never over an object that is already there.
    ///
    /// # Errors
    ///
    /// Fails when the object is already there or the file could not be sent.
    pub fn put(&self, local: &Path, key: &str) -> Result<()> {
        match self {
            Target::Folder(_) => {
                let dst = self.folder_path(key)?; // WHAT: where it lands. WHY: checked above.
                if dst.exists() {
                    bail!("{key} is already there; a backup is never written over");
                } // WHAT: refuse an overwrite. WHY: what was sent stays as it was sent.
                #[cfg(test)]
                if let Some(n) = FAIL_PUT_AFTER.with(std::cell::Cell::get) { // WHAT: the uploads left before a planted failure. WHY: read from its cell
                    if n == 0 {
                        bail!("a planted upload failure");
                    }
                    FAIL_PUT_AFTER.with(|f| f.set(Some(n - 1)));
                } // WHAT: the tests' connection that drops part way. WHY: the next night must still run.
                #[cfg(test)]
                if LOSE_UPLOADS
                    .with(std::cell::Cell::get) // WHAT: the folder whose uploads a test loses. WHY: read from its cell
                    .is_some_and(|f| key.starts_with(f))
                {
                    return Ok(());
                } // WHAT: the tests' lossy bucket says yes and keeps nothing. WHY: the read-back must catch it.
                if let Some(p) = dst.parent() {
                    std::fs::create_dir_all(p)?;
                } // WHAT: its folder. WHY: the first one makes it.
                let part = dst.with_extension("part"); // WHAT: a temporary name. WHY: a half copy is never taken for a whole one.
                std::fs::copy(local, &part)
                    .with_context(|| format!("copying to {}", part.display()))?; // WHAT: copy it. WHY: the upload.
                std::fs::OpenOptions::new().write(true).open(&part)?.sync_all()?; // WHAT: onto the disk, the file opened for writing. WHY: a power cut must not leave a short file under a good name; Windows refuses to flush a file opened read-only (Access is denied), which failed every night to a folder target there.
                std::fs::rename(&part, &dst)?; // WHAT: the real name. WHY: only now is it whole.
                Ok(())
            }
            Target::Rclone { .. } => {
                let r = self.remote(key)?;
                self.rclone(&[
                    "copyto",
                    &local.to_string_lossy(),
                    &r,
                    "--s3-no-check-bucket",
                    "--retries",
                    "5",
                    "--low-level-retries",
                    "20",
                ])
                .map(|_| ())
            } // WHAT: rclone sends it. WHY: it checks the bytes arrived and retries a dropped connection.
        }
    }

    /// Sends every file in a local folder into a folder of the target.
    ///
    /// # Errors
    ///
    /// Fails when the folder can't be read or any file could not be sent.
    pub fn put_all(&self, local_dir: &Path, folder: &str) -> Result<()> {
        match self {
            Target::Folder(_) => {
                for e in std::fs::read_dir(local_dir)? {
                    let e = e?;
                    self.put(
                        &e.path(),
                        &format!("{folder}/{}", e.file_name().to_string_lossy()),
                    )?;
                }
                Ok(())
            } // WHAT: one by one. WHY: a folder is quick.
            Target::Rclone { .. } => {
                let r = self.remote(folder)?;
                self.rclone(&[
                    "copy",
                    &local_dir.to_string_lossy(),
                    &r,
                    "--s3-no-check-bucket",
                    "--ignore-existing",
                    "--retries",
                    "5",
                    "--low-level-retries",
                    "20",
                    "--transfers",
                    "4",
                ])
                .map(|_| ())
            } // WHAT: one rclone run for the lot. WHY: thousands of drawings would be thousands of starts; never over an existing object.
        }
    }

    /// The objects in a folder: name and size.
    ///
    /// # Errors
    ///
    /// Fails when the target can't be listed.
    pub fn list(&self, folder: &str) -> Result<Vec<(String, u64)>> {
        match self {
            Target::Folder(_) => {
                let dir = self.folder_path(folder)?; // WHAT: the folder. WHY: checked.
                if !dir.exists() {
                    return Ok(Vec::new());
                } // WHAT: none yet. WHY: the first night.
                let mut out = Vec::new(); // WHAT: the list. WHY: filled below.
                for e in std::fs::read_dir(dir)? {
                    let e = e?;
                    let m = e.metadata()?;
                    if m.is_file() {
                        out.push((e.file_name().to_string_lossy().into_owned(), m.len()));
                    }
                } // WHAT: each file. WHY: folders inside are not ours.
                Ok(out)
            }
            Target::Rclone { .. } => {
                #[derive(Deserialize)]
                struct Item {
                    #[serde(rename = "Name")]
                    name: String,
                    #[serde(rename = "Size")]
                    size: i64,
                } // WHAT: rclone's listing. WHY: only the two fields used.
                let r = self.remote(folder)?; // WHAT: the remote folder. WHY: checked.
                let text = self.rclone(&["lsjson", &r, "--files-only"])?; // WHAT: list it. WHY: what is really there.
                let items: Vec<Item> =
                    serde_json::from_str(if text.trim().is_empty() { "[]" } else { &text })?; // WHAT: read the JSON. WHY: an empty answer is no objects.
                Ok(items
                    .into_iter()
                    .map(|i| (i.name, u64::try_from(i.size).unwrap_or(0))) // WHAT: name and size, a negative size read as nought. WHY: exactly what `.max(0) as u64` gave
                    .collect()) // WHAT: name and size. WHY: the size confirms an upload.
            }
        }
    }

    /// A copy inside the target (weekly/, monthly/). Nothing is sent again.
    ///
    /// # Errors
    ///
    /// Fails when the copy could not be made.
    pub fn copy(&self, from: &str, to: &str) -> Result<()> {
        match self {
            Target::Folder(_) => {
                let src = self.folder_path(from)?;
                self.put(&src, to)
            } // WHAT: a file copy. WHY: the same rules as a put.
            Target::Rclone { .. } => {
                let (a, b) = (self.remote(from)?, self.remote(to)?);
                self.rclone(&["copyto", &a, &b, "--s3-no-check-bucket"])
                    .map(|_| ())
            } // WHAT: a copy on the server side. WHY: nothing leaves the box again.
        }
    }

    /// Deletes one object. Only the keep rule calls this, only for our own names.
    ///
    /// # Errors
    ///
    /// Fails when the object could not be deleted.
    pub fn delete(&self, key: &str) -> Result<()> {
        match self {
            Target::Folder(_) => {
                let p = self.folder_path(key)?;
                std::fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))
            } // WHAT: remove the file. WHY: the keep rule.
            Target::Rclone { .. } => {
                let r = self.remote(key)?;
                self.rclone(&["deletefile", &r]).map(|_| ())
            } // WHAT: delete it. WHY: a bucket lock may refuse it, and that is the lock working.
        }
    }

    /// Fetches one object to a local file.
    ///
    /// # Errors
    ///
    /// Fails when the object could not be fetched.
    pub fn get(&self, key: &str, local: &Path) -> Result<()> {
        match self {
            Target::Folder(_) => {
                let p = self.folder_path(key)?;
                std::fs::copy(&p, local)
                    .with_context(|| format!("reading {}", p.display()))
                    .map(|_| ())
            } // WHAT: copy it here. WHY: the drill reads a local copy.
            Target::Rclone { .. } => {
                let r = self.remote(key)?;
                self.rclone(&["copyto", &r, &local.to_string_lossy()])
                    .map(|_| ())
            } // WHAT: download it. WHY: the same.
        }
    }

    /// Fetches many objects of one folder into a local folder.
    ///
    /// # Errors
    ///
    /// Fails when any of them could not be fetched.
    pub fn get_many(&self, folder: &str, names: &[String], local_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(local_dir)?; // WHAT: the folder. WHY: they land there.
        match self {
            Target::Folder(_) => {
                for n in names {
                    self.get(&format!("{folder}/{n}"), &local_dir.join(n))?;
                }
                Ok(())
            } // WHAT: one by one. WHY: a folder is quick.
            Target::Rclone { .. } => {
                if names.iter().any(|n| !key_ok(n) || n.contains('/')) {
                    bail!("refusing an object name in the list");
                } // WHAT: check every name. WHY: the list is a file rclone reads.
                let list = local_dir.join(".wanted.txt"); // WHAT: the names, one per line. WHY: rclone's --files-from.
                std::fs::write(&list, names.join("\n"))?; // WHAT: write it. WHY: one rclone run for the lot.
                let r = self.remote(folder)?; // WHAT: the remote folder. WHY: checked.
                let res = self.rclone(&[
                    "copy",
                    &r,
                    &local_dir.to_string_lossy(),
                    "--files-from",
                    &list.to_string_lossy(),
                    "--retries",
                    "5",
                    "--transfers",
                    "4",
                ]); // WHAT: download them. WHY: many starts would be slow.
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
    Fake, // WHAT: the tests' stand-in. WHY: unit tests run on machines without age; real age is tested too where it is installed.
}

impl Cipher {
    /// The program this runs, if it runs one.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn program(&self) -> Option<&Path> {
        match self {
            Cipher::Age(exe) => Some(exe),
            #[cfg(test)]
            Cipher::Fake => None,
        } // WHAT: age's path; the tests' stand-in runs nothing. WHY: checked before a run starts.
    }

    /// Encrypts what `fill` writes, to the recipients file, into `out`. Nothing unencrypted touches the disk.
    ///
    /// # Errors
    ///
    /// Fails when age could not be run, `fill` fails, or age does not finish cleanly.
    pub fn encrypt_stream(
        &self,
        recipients: &Path,
        out: &Path,
        fill: impl FnOnce(&mut dyn Write) -> Result<()>,
    ) -> Result<()> {
        match self {
            Cipher::Age(exe) => {
                let _ = std::fs::remove_file(out);
                let encrypted = // WHAT: the encrypted file, named apart from `fill`. WHY: the two read alike
                    create_private(out).with_context(|| format!("making {}", out.display()))?; // WHAT: the encrypted file, owner-only before a byte is in it. WHY: everything in the work folder is this account's alone.
                let mut child = Command::new(exe)
                    .arg("-R")
                    .arg(recipients)
                    .stdin(Stdio::piped())
                    .stdout(encrypted) // WHAT: age writes into the encrypted file. WHY: owner-only from its first byte
                    .stderr(Stdio::piped())
                    .spawn()
                    .with_context(|| format!("starting {}", exe.display()))?; // WHAT: age reads the stream and writes the encrypted file into it. WHY: the server locks with the public key only.
                let mut sin = child
                    .stdin
                    .take()
                    .ok_or_else(|| anyhow!("age has no input"))?; // WHAT: age's input. WHY: the archive is written into it.
                let filled = fill(&mut sin); // WHAT: write everything. WHY: one stream from the files to age.
                drop(sin); // WHAT: close age's input. WHY: age writes the end of the file when its input ends.
                let done = child.wait_with_output()?; // WHAT: wait for age. WHY: its result decides.
                filled?; // WHAT: a failure while writing. WHY: a half archive is never kept.
                if !done.status.success() {
                    bail!(
                        "age failed: {}",
                        String::from_utf8_lossy(&done.stderr).trim()
                    );
                } // WHAT: age's own failure. WHY: say why.
                Ok(())
            }
            #[cfg(test)]
            Cipher::Fake => {
                let keys = std::fs::read_to_string(recipients)?;
                let mut buf = Vec::new();
                fill(&mut buf)?;
                let _ = std::fs::remove_file(out);
                create_private(out)?
                    .write_all(&fake::seal(&keys, &buf))
                    .map_err(Into::into)
            } // WHAT: the stand-in. WHY: the same shape as age's output.
        }
    }

    /// Encrypts one file into another. Returns the SHA-256 of the bytes that went in, as they went.
    ///
    /// # Errors
    ///
    /// Fails when the file can't be read or the encryption fails.
    pub fn encrypt_file(&self, recipients: &Path, input: &Path, out: &Path) -> Result<String> {
        let mut h = Sha256::new(); // WHAT: the fingerprint of what is sent. WHY: a file changed between its check and its sending is caught.
        self.encrypt_stream(recipients, out, |w| {
            let mut f = std::fs::File::open(input)?;
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
                w.write_all(&buf[..n])?;
            }
            Ok(())
        })?; // WHAT: stream the file through, fingerprinting it. WHY: a big drawing is never held in memory.
        Ok(hex(&h.finalize())) // WHAT: the fingerprint. WHY: compared with the file's name.
    }

    /// Opens a passphrase-locked file (Creede's locked key): age asks for the passphrase on the console itself.
    ///
    /// # Errors
    ///
    /// Fails when age could not be run or the passphrase is wrong.
    pub fn unlock(&self, input: &Path, out: &Path) -> Result<()> {
        match self {
            Cipher::Age(exe) => {
                let file =
                    create_private(out).with_context(|| format!("making {}", out.display()))?; // WHAT: the opened key's file, owner-only before a byte is in it. WHY: it opens every backup.
                let done = Command::new(exe)
                    .arg("-d")
                    .arg(input)
                    .stdin(Stdio::inherit())
                    .stdout(file)
                    .stderr(Stdio::inherit())
                    .status(); // WHAT: age prompts for the passphrase and writes into that file. WHY: this program never sees the passphrase, and age never makes the file itself.
                let done = match done {
                    Ok(d) => d,
                    Err(e) => {
                        let _ = std::fs::remove_file(out);
                        return Err(anyhow!("starting {}: {e}", exe.display()));
                    }
                }; // WHAT: age would not start. WHY: leave nothing behind.
                if !done.success() {
                    let _ = std::fs::remove_file(out);
                    bail!("age could not open the key file: wrong passphrase?");
                } // WHAT: refuse, leaving nothing. WHY: nothing else can run.
                Ok(())
            }
            #[cfg(test)]
            Cipher::Fake => {
                let b = std::fs::read(input)?;
                let inner = b
                    .strip_prefix(b"FAKEPASS1\n")
                    .filter(|i| i.starts_with(b"# public key:"))
                    .ok_or_else(|| anyhow!("age could not open the key file: wrong passphrase?"))?;
                create_private(out)?.write_all(inner).map_err(Into::into)
            } // WHAT: the stand-in's locked file. WHY: tests the one-prompt path.
        }
    }

    /// Decrypts with Creede's private key (an unlocked key: see `Drill::run`, which opens a locked one once).
    ///
    /// # Errors
    ///
    /// Fails when age could not be run or the file does not open with that key.
    pub fn decrypt_file(&self, identity: &Path, input: &Path, out: &Path) -> Result<()> {
        match self {
            Cipher::Age(exe) => {
                let file =
                    create_private(out).with_context(|| format!("making {}", out.display()))?; // WHAT: the output, owner-only before a byte is in it. WHY: a restored file is as private as the original.
                let done = Command::new(exe)
                    .arg("-d")
                    .arg("-i")
                    .arg(identity)
                    .arg(input)
                    .stdin(Stdio::inherit())
                    .stdout(file)
                    .stderr(Stdio::piped())
                    .output(); // WHAT: age decrypts into that file. WHY: age never makes the file itself, so it is never open to others.
                let done = match done {
                    Ok(d) => d,
                    Err(e) => {
                        let _ = std::fs::remove_file(out);
                        return Err(anyhow!("starting {}: {e}", exe.display()));
                    }
                }; // WHAT: age would not start. WHY: leave nothing behind.
                if !done.status.success() {
                    let _ = std::fs::remove_file(out);
                    bail!(
                        "could not decrypt (is this the right key?): {}",
                        String::from_utf8_lossy(&done.stderr).trim()
                    );
                } // WHAT: refuse, leaving nothing. WHY: the wrong key opens nothing.
                Ok(())
            }
            #[cfg(test)]
            Cipher::Fake => {
                let id = std::fs::read_to_string(identity)?;
                let data = std::fs::read(input)?;
                let plain = fake::open(&id, &data)
                    .ok_or_else(|| anyhow!("could not decrypt (is this the right key?)"))?;
                create_private(out)?.write_all(&plain).map_err(Into::into)
            } // WHAT: the stand-in. WHY: the wrong key must fail here too.
        }
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! The tests' stand-in for age: "FAKEAGE1", the recipients, then the bytes scrambled with a key from them. Not
    //! encryption; it only lets the tests check what this module does with an encrypted stream on machines without age.
    use sha2::{Digest, Sha256}; // WHAT: a key stream. WHY: scrambles the bytes so the tests can see nothing plain leaks.
    fn stream(keys: &str, n: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(n + 32);
        let mut c = 0u64;
        while out.len() < n {
            out.extend_from_slice(
                &Sha256::new()
                    .chain_update(keys.as_bytes())
                    .chain_update(c.to_le_bytes())
                    .finalize(),
            );
            c += 1;
        }
        out.truncate(n);
        out
    } // WHAT: bytes from the keys. WHY: the same keys undo it.
    fn recips(text: &str) -> String {
        text.lines()
            .map(str::trim)
            .filter(|l| l.starts_with("age1"))
            .collect::<Vec<_>>()
            .join(",")
    } // WHAT: the public keys, in order. WHY: the "lock".
    pub fn seal(keys: &str, data: &[u8]) -> Vec<u8> {
        let k = recips(keys);
        let mut out = format!("FAKEAGE1\n{k}\n").into_bytes();
        out.extend(data.iter().zip(stream(&k, data.len())).map(|(a, b)| a ^ b));
        out
    } // WHAT: the stand-in's output. WHY: the header says who may open it.
    pub fn open(identity: &str, data: &[u8]) -> Option<Vec<u8>> {
        let pubkey = identity
            .lines()
            .find_map(|l| l.trim().strip_prefix("# public key: "))?
            .trim()
            .to_string();
        let rest = data.strip_prefix(b"FAKEAGE1\n")?;
        let nl = rest.iter().position(|&b| b == b'\n')?;
        let k = std::str::from_utf8(&rest[..nl]).ok()?;
        if !k.split(',').any(|x| x == pubkey) {
            return None;
        }
        Some(
            rest[nl + 1..]
                .iter()
                .zip(stream(k, rest.len() - nl - 1))
                .map(|(a, b)| a ^ b)
                .collect(),
        )
    } // WHAT: undo it, only for a listed key. WHY: the wrong key must fail.
}

// ---------------------------------------------------------------- the archive: plain tar, written and read by hand

/// Writes a plain ustar archive: regular files only, short plain names.
struct TarOut<'a> {
    w: &'a mut dyn Write,
} // WHAT: the stream it writes into. WHY: gzip, then age.

impl TarOut<'_> {
    fn file(&mut self, name: &str, size: u64, body: &mut dyn Read) -> Result<()> {
        if name.len() > 99 || !key_ok(name) {
            bail!("an archive name must be short and plain: {name}");
        } // WHAT: the names we write. WHY: the reader refuses anything else.
        if size >= 0o77_777_777_777 { // WHAT: the largest size ustar's eleven octal digits hold. WHY: the same number, its digits grouped
            bail!("{name} is too big for the archive ({size} bytes)");
        } // WHAT: ustar's size limit (8 GiB). WHY: refuse rather than write a broken archive.
        let mut h = [0u8; 512]; // WHAT: the header block. WHY: ustar's fixed layout.
        h[..name.len()].copy_from_slice(name.as_bytes()); // WHAT: the name. WHY: field 0.
        h[100..108].copy_from_slice(b"0000600\0"); // WHAT: owner-only file mode. WHY: a restored copy is not handed to everyone.
        h[108..116].copy_from_slice(b"0000000\0");
        h[116..124].copy_from_slice(b"0000000\0"); // WHAT: owner and group 0. WHY: meaningless across machines.
        h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes()); // WHAT: the size in octal. WHY: ustar's form.
        h[136..148].copy_from_slice(b"00000000000\0"); // WHAT: no date. WHY: the manifest carries the one date that matters.
        h[156] = b'0'; // WHAT: a regular file. WHY: nothing else is ever written.
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00"); // WHAT: the format marker. WHY: any tar reads it.
        h[148..156].copy_from_slice(b"        "); // WHAT: blanks while summing. WHY: the checksum's rule.
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum(); // WHAT: the checksum. WHY: readers check it.
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes()); // WHAT: write it. WHY: ustar's form.
        self.w.write_all(&h)?; // WHAT: the header. WHY: before its bytes.
        let copied = std::io::copy(&mut body.take(size), self.w)?; // WHAT: the file's bytes. WHY: streamed.
        if copied != size {
            bail!("{name} changed size while it was packed");
        } // WHAT: refuse a short file. WHY: the header promised its size.
        let pad = (512 - (size % 512) as usize) % 512;
        self.w.write_all(&vec![0u8; pad])?; // WHAT: pad to a block. WHY: ustar's rule.
        Ok(())
    }
    fn end(&mut self) -> Result<()> {
        self.w.write_all(&[0u8; 1024])?;
        Ok(())
    } // WHAT: two empty blocks. WHY: the end marker.
}

/// Reads an archive into a folder. Refuses anything that isn't a short plain file name: a damaged or planted
/// archive can never write outside `into`.
fn untar(r: &mut dyn Read, into: &Path, limit: u64, expect: &[(&str, u64)]) -> Result<Vec<String>> {
    let mut names = Vec::new(); // WHAT: what came out. WHY: the caller checks it.
    let mut total = 0u64; // WHAT: bytes so far. WHY: a planted archive can't fill the disk.
    loop {
        let mut h = [0u8; 512]; // WHAT: one header. WHY: read block by block.
        r.read_exact(&mut h).context("the archive is cut short")?; // WHAT: read it. WHY: an archive must end with its end marker.
        if h.iter().all(|&b| b == 0) {
            return Ok(names);
        } // WHAT: the end. WHY: everything was read.
        let field = |a: usize, z: usize| {
            std::str::from_utf8(&h[a..z])
                .ok()
                .map(|t| t.trim_matches(|c: char| c == '\0' || c == ' ').to_string())
        }; // WHAT: a header field as text. WHY: numbers in ustar are octal text.
        let stored = field(148, 156)
            .and_then(|t| u32::from_str_radix(&t, 8).ok())
            .ok_or_else(|| anyhow!("a damaged archive header"))?; // WHAT: the header's own checksum. WHY: a damaged header is refused.
        let mut blank = h;
        blank[148..156].copy_from_slice(b"        "); // WHAT: blanks where the sum goes. WHY: the rule it was made with.
        if blank.iter().map(|&b| u32::from(b)).sum::<u32>() != stored { // WHAT: the header summed, each byte widened. WHY: compared with the checksum it carries
            bail!("a damaged archive header");
        } // WHAT: compare. WHY: never act on a damaged header.
        if h[156] != b'0' && h[156] != 0 {
            bail!("the archive holds something that isn't a plain file; refusing it");
        } // WHAT: files only. WHY: links could point outside the folder.
        let end = h[..100].iter().position(|&b| b == 0).unwrap_or(100); // WHAT: the name's end. WHY: names are NUL-padded.
        let name = std::str::from_utf8(&h[..end])?.to_string(); // WHAT: the name. WHY: checked next.
        if !key_ok(&name) || h[345] != 0 {
            bail!("the archive holds a name this program never writes: {name:?}; refusing it");
        } // WHAT: only our plain names, no prefix field. WHY: no ../, no absolute path, no drive letter.
        let size = field(124, 136)
            .and_then(|t| u64::from_str_radix(&t, 8).ok())
            .ok_or_else(|| anyhow!("a damaged size in the archive"))?; // WHAT: its size. WHY: how much to read.
        let Some(&(_, cap)) = expect.iter().find(|(n, _)| *n == name) else {
            bail!("the archive holds {name:?}, which this program never restores; refusing it");
        }; // WHAT: only the names expected. WHY: nothing else is ever written.
        if size > cap {
            bail!("{name} in the archive is {size} bytes, over the {cap} allowed; refusing it");
        } // WHAT: each name's own cap, before writing. WHY: a planted giant never reaches the disk.
        total += size;
        if total > limit {
            bail!("the archive is bigger than it said it would be; refusing it");
        } // WHAT: the cap. WHY: never fill the disk on a planted archive.
        let path = name.split('/').fold(into.to_path_buf(), |p, s| p.join(s)); // WHAT: where it goes. WHY: inside `into`, by construction.
        if !path.starts_with(into)
            || path.components().count() != into.components().count() + name.split('/').count()
        {
            bail!("an archive name would land outside its folder: {name:?}; refusing it");
        } // WHAT: checked again, by path. WHY: never written outside, whatever the name check missed.
        if let Some(p) = path.parent() {
            create_private_dir(p)?;
        } // WHAT: its folder, owner-only. WHY: made as needed.
        let mut f = create_private(&path)
            .with_context(|| format!("{name} appears twice, or can't be written"))?; // WHAT: a new owner-only file, never over one. WHY: a name twice is a planted archive, and a restored database is never readable by others.
        let n = std::io::copy(&mut r.take(size), &mut f)?; // WHAT: its bytes. WHY: streamed.
        if n != size {
            bail!("the archive is cut short");
        } // WHAT: a short body. WHY: refused.
        let pad = (512 - (size % 512)) % 512;
        std::io::copy(&mut r.take(pad), &mut std::io::sink())?; // WHAT: skip the padding. WHY: to the next header.
        names.push(name); // WHAT: note it. WHY: returned.
    }
}

// ---------------------------------------------------------------- the manifest

/// Inside the encrypted catalog: everything the drill needs, and the counts it checks.
#[derive(Serialize, Deserialize)]
pub struct Manifest {
    pub format: String, // WHAT: "excalibur-offsite-1". WHY: a later format is refused, not misread.
    pub product: String, // WHAT: "view". WHY: the drill checks it restored the right product.
    pub name: String,   // WHAT: the catalog's own name. WHY: a renamed object is noticed.
    #[serde(rename = "createdAt")]
    pub created_at: String, // WHAT: when the data was copied. WHY: the backup's age.
    #[serde(rename = "appVersion")]
    pub app_version: String, // WHAT: the server's version. WHY: restore with the same or newer.
    #[serde(rename = "databaseSha256")]
    pub database_sha256: String, // WHAT: the database copy's fingerprint. WHY: the drill checks the bytes.
    pub counts: BTreeMap<String, i64>, // WHAT: rows per table. WHY: the drill compares.
    pub salt: String, // WHAT: the salt in the drawings' object names. WHY: the drill finds each one.
    pub blobs: Vec<(String, u64)>, // WHAT: every drawing file: digest and size. WHY: the drill checks each is in the bucket.
    #[serde(rename = "missingOnDisk")]
    pub missing_on_disk: Vec<String>, // WHAT: files the database names that weren't on this disk. WHY: damage found by the backup is said, never hidden.
    #[serde(default)]
    pub installation: Option<String>, // WHAT: the office whose data this is (its installation id). WHY: a drill says whose backup it restored; catalogs made before 2026-11-11 have none (audit M2)
    #[serde(default)]
    pub company: Option<String>, // WHAT: the company name the office was set up with, if any. WHY: inside the encrypted catalog only, so a person restoring sees whose it is in words
}

/// Beside the catalog, unencrypted: sizes and fingerprints only, never a name or a record.
#[derive(Serialize, Deserialize)]
pub struct Sidecar {
    pub format: String, // WHAT: the format. WHY: as above.
    pub name: String,   // WHAT: the catalog's name. WHY: matched on download.
    #[serde(rename = "createdAt")]
    pub created_at: String, // WHAT: when. WHY: shown without the key.
    #[serde(rename = "appVersion")]
    pub app_version: String, // WHAT: which version. WHY: the same.
    #[serde(rename = "ageSha256")]
    pub age_sha256: String, // WHAT: the encrypted file's fingerprint. WHY: a damaged download is caught before decrypting.
    #[serde(rename = "ageBytes")]
    pub age_bytes: u64, // WHAT: its size. WHY: the upload is confirmed by it.
    #[serde(rename = "blobCount")]
    pub blob_count: usize, // WHAT: how many drawing files. WHY: a quick sense of size.
    #[serde(rename = "blobBytes")]
    pub blob_bytes: u64, // WHAT: their total. WHY: the same.
}

/// The two markers in the data folder the ops report reads.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Marker {
    #[serde(rename = "lastGood", default)]
    pub last_good: Option<String>, // WHAT: when the last confirmed backup's data was copied. WHY: backupDays.
    #[serde(default)]
    pub r#where: Option<String>, // WHAT: where it went, in words. WHY: Fleet shows it.
    #[serde(default)]
    pub object: Option<String>, // WHAT: which catalog. WHY: the drill and a person can find it.
    #[serde(default)]
    pub ok: Option<bool>, // WHAT: a drill's result. WHY: only a pass counts.
    #[serde(default)]
    pub at: Option<String>, // WHAT: when the drill ran, or the night failed. WHY: restoreTested, lastFailed.
    #[serde(default)]
    pub why: Option<String>, // WHAT: why the night failed. WHY: lastFailedWhy.
}

pub const BACKUP_MARKER: &str = "offsite-backup.json"; // WHAT: written after a confirmed backup. WHY: the ops report's backupDays.
pub const FAILED_MARKER: &str = "offsite-failed.json"; // WHAT: written after a failed night, removed after a good one. WHY: the ops report's lastFailed.
const BUSY: &str = "another off-site backup is still running"; // WHAT: the lock's refusal. WHY: a run already going is not a failed night.
pub const DRILL_MARKER: &str = "offsite-drill.json"; // WHAT: written after a passed drill. WHY: the ops report's restoreTested.

// ---------------------------------------------------------------- whose target it is

const OWNER_FORMAT: &str = "excalibur-offsite-owner-1"; // WHAT: the owner marker's format. WHY: a later format is refused, not misread.
const OWNER_CAP: u64 = 4 * 1024; // WHAT: the largest owner marker read. WHY: a real one is about 150 bytes.

/// At the top of a target, one per product (`owner-view.json`): which office's backups the target holds. Unencrypted,
/// and holds no name or record, only the office's installation id. Two offices pointed at one NAS share or bucket
/// folder would each prune the other's catalogs as their own; this marker makes the second one refuse (audit M2).
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Owner {
    pub format: String, // WHAT: OWNER_FORMAT. WHY: as above.
    pub product: String, // WHAT: "view". WHY: one bucket may hold several products' folders, each with its own owner.
    pub installation: String, // WHAT: the office's installation id. WHY: the one thing compared.
    pub since: String, // WHAT: when the target was first used. WHY: a person reading it knows how long it has been this office's.
}

/// The owner marker's object name for a product.
fn owner_key(product: &str) -> String {
    format!("owner-{product}.json") // WHAT: at the top of the target. WHY: beside daily/, weekly/, monthly/ and blobs/, never inside them.
}

/// An installation id as the server makes them: a short word, `_`, hex. Anything else is refused.
fn is_installation(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') // WHAT: plain characters, short. WHY: it is printed in the log and compared; a planted marker can't carry anything else.
}

/// The office's installation id from a database (the backup's consistent copy). The server makes it as it first starts.
///
/// # Errors
///
/// Fails when the database can't be opened or has no installation id.
pub fn installation_of(db: &Path) -> Result<String> {
    use rusqlite::OptionalExtension; // WHAT: a row that may be missing. WHY: a database the server never ran on has none.
    let c = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?; // WHAT: read-only. WHY: the copy is never changed.
    let id: Option<String> = c
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [crate::api::settings::INSTALLATION],
            |r| r.get(0),
        )
        .optional()?; // WHAT: the server's own name for itself. WHY: the same id the office answers to on the network.
    match id.map(|s| s.trim().to_string()) {
        Some(s) if is_installation(&s) => Ok(s),
        Some(_) => bail!("this office's installation id isn't one the server makes"),
        None => bail!("this data has no installation id yet: start the server once, then back up"),
    } // WHAT: the id, or why not. WHY: a backup with no office to name would adopt any target.
}

/// The company name the office was set up with, from a database copy, or `None`. Never fails a backup.
fn company_of(db: &Path) -> Option<String> {
    use rusqlite::OptionalExtension; // WHAT: a row that may be missing. WHY: an office not set up yet has none.
    let c = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?; // WHAT: read-only. WHY: the copy is never changed.
    c.query_row("SELECT value FROM settings WHERE key = ?1", [crate::api::settings::COMPANY], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .map(|s| s.chars().filter(|c| !c.is_control()).take(200).collect()) // WHAT: printable, at most 200 characters. WHY: shown in the drill's log line
}

impl Target {
    /// The objects at the top of the target: name and size. Folders are left out.
    fn top(&self) -> Result<Vec<(String, u64)>> {
        match self {
            Target::Folder(root) => {
                if !root.exists() {
                    return Ok(Vec::new());
                } // WHAT: none yet. WHY: the first night makes the folder.
                let mut out = Vec::new();
                for e in std::fs::read_dir(root)? {
                    let e = e?;
                    let m = e.metadata()?;
                    if m.is_file() {
                        out.push((e.file_name().to_string_lossy().into_owned(), m.len()));
                    }
                } // WHAT: each file. WHY: the owner marker is one.
                Ok(out)
            }
            Target::Rclone { remote, .. } => {
                #[derive(Deserialize)]
                struct Item {
                    #[serde(rename = "Name")]
                    name: String,
                    #[serde(rename = "Size")]
                    size: i64,
                } // WHAT: rclone's listing. WHY: the two fields used.
                let text = self.rclone(&["lsjson", remote, "--files-only"])?; // WHAT: the top level only (lsjson doesn't recurse by default). WHY: the marker sits there.
                let items: Vec<Item> =
                    serde_json::from_str(if text.trim().is_empty() { "[]" } else { &text })?;
                Ok(items.into_iter().map(|i| (i.name, u64::try_from(i.size).unwrap_or(0))).collect()) // WHAT: name and size, a negative size read as nought. WHY: exactly what `.max(0) as u64` gave
            }
        }
    }

    /// The product's owner marker, read and checked, or `None` when the target has none. A listing that fails is an
    /// error, never "none": a target that can't be read must not be adopted. The error carries the exit code: 3 the
    /// target couldn't be reached, 2 its marker is refused.
    ///
    /// # Errors
    ///
    /// The exit code: 3 when the target couldn't be reached, 2 when its marker is refused.
    pub fn owner(&self, product: &str, scratch: &Path) -> std::result::Result<Option<Owner>, Exit> {
        let key = owner_key(product); // WHAT: its name. WHY: looked for at the top.
        let top = self.top().map_err(|e| refuse(3, format!("listing {} failed: {e:#}", self.words())))?; // WHAT: the top level. WHY: an unreadable target is a failed night, not a new target
        let Some((_, size)) = top.into_iter().find(|(n, _)| *n == key) else {
            return Ok(None);
        }; // WHAT: not there. WHY: a new target, adopted by the caller.
        if size > OWNER_CAP {
            return Err(refuse(2, format!("{key} is listed at {size} bytes, over the {OWNER_CAP} an owner marker can be; not fetched")));
        } // WHAT: refuse a giant unfetched. WHY: it would fill the disk.
        let local = scratch.join(".owner.json"); // WHAT: where it lands. WHY: in the job's own folder.
        let _ = std::fs::remove_file(&local); // WHAT: no leftover. WHY: a copy fails on some systems over a file.
        self.get(&key, &local).map_err(|e| refuse(3, format!("fetching {key} failed: {e:#}")))?; // WHAT: fetch it. WHY: read below.
        let bytes = read_capped(&local, OWNER_CAP); // WHAT: read it, within the cap. WHY: the listing could have lied.
        let _ = std::fs::remove_file(&local); // WHAT: tidy. WHY: kept nowhere.
        let bytes = bytes.map_err(|e| refuse(2, format!("{key}: {e:#}")))?;
        let o: Owner = serde_json::from_slice(&bytes).map_err(|e| refuse(2, format!("{key} doesn't read: {e}")))?; // WHAT: parse. WHY: a broken marker is a refusal, never a new target.
        if o.format != OWNER_FORMAT || o.product != product || !is_installation(&o.installation) {
            return Err(refuse(2, format!("{key} is not one this program writes")));
        } // WHAT: exactly as written. WHY: a planted marker can't say something else.
        Ok(Some(o))
    }

    /// Makes sure the target is this office's: adopts a target with no owner, refuses one owned by another office.
    /// Returns what to log.
    ///
    /// # Errors
    ///
    /// The exit code: 3 when the target couldn't be reached, 2 when another office owns it.
    pub fn claim(&self, product: &str, installation: &str, scratch: &Path) -> std::result::Result<String, Exit> {
        if let Some(o) = self.owner(product, scratch)? {
            if o.installation == installation {
                return Ok(format!("target belongs to this office ({installation})"));
            } // WHAT: ours. WHY: the usual night.
            return Err(refuse(2, format!("{} belongs to another office (installation {}, since {}), not this one ({installation}). Two offices must never share a backup folder: each prunes the other's backups as its own. Give this office its own folder or bucket path (README.md, Off-site backups)", self.words(), o.installation, o.since)));
        } // WHAT: another office's. WHY: refused before anything is sent or pruned.
        let o = Owner {
            format: OWNER_FORMAT.into(),
            product: product.into(),
            installation: installation.into(),
            since: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        }; // WHAT: the marker. WHY: this office takes the new target.
        let local = scratch.join(".owner-new.json"); // WHAT: written here first. WHY: sent like any object.
        let bytes = serde_json::to_vec_pretty(&o).map_err(|e| refuse(2, e))?;
        write_atomic(&local, &bytes).map_err(|e| refuse(2, format!("{e:#}")))?;
        let sent = self.put(&local, &owner_key(product)); // WHAT: send it. WHY: never over one that's there (a folder target refuses).
        let _ = std::fs::remove_file(&local);
        sent.map_err(|e| refuse(3, format!("writing the owner marker failed: {e:#}")))?;
        match self.owner(product, scratch)? {
            Some(back) if back.installation == installation => {
                Ok(format!("target was new: it now belongs to this office ({installation})"))
            }
            _ => Err(refuse(2, format!("{} did not keep this office's owner marker; not backing up", self.words()))),
        } // WHAT: read it back. WHY: two offices starting the same night: only the one whose marker stuck goes on.
    }
}

/// What the ops report says about off-site backups: each field `None` when unknown, never a guess.
#[derive(Serialize, Default, Clone, Debug, PartialEq)]
pub struct Report {
    #[serde(rename = "lastGood")]
    pub last_good: Option<String>, // WHAT: when the last confirmed backup was taken. WHY: how old.
    pub r#where: Option<String>, // WHAT: where it went. WHY: "backed up" means nothing without where.
    #[serde(rename = "restoreTested")]
    pub restore_tested: Option<String>, // WHAT: when a drill last passed. WHY: a backup never restored is unproven.
    #[serde(rename = "lastFailed")]
    pub last_failed: Option<String>, // WHAT: when the last night failed, until a good one. WHY: a failure reaches Fleet, not only the log.
    #[serde(rename = "lastFailedWhy")]
    pub last_failed_why: Option<String>, // WHAT: why it failed. WHY: what to look at first.
}

fn past_time(s: Option<&str>) -> Option<time::OffsetDateTime> { // WHAT: the time as text, if there is one. WHY: borrowed as it is read
    let t = time::OffsetDateTime::parse(
        s?, // WHAT: the text, or no time. WHY: as before
        &time::format_description::well_known::Rfc3339,
    )
    .ok()?; // WHAT: read the time. WHY: a garbled one is unknown.
    (t <= time::OffsetDateTime::now_utc() + time::Duration::hours(1)).then_some(t)
    // WHAT: refuse the future. WHY: a backup from tomorrow isn't believed.
}

fn read_marker(path: &Path) -> Option<Marker> {
    let m = std::fs::symlink_metadata(path).ok()?; // WHAT: the file itself. WHY: a link is not ours.
    if !m.is_file() || m.len() > 64 * 1024 {
        return None;
    } // WHAT: a small plain file only. WHY: anything else is unknown, not believed.
    serde_json::from_slice(&std::fs::read(path).ok()?).ok() // WHAT: parse it. WHY: a damaged file is unknown.
}

/// The ops report's off-site section, read from the data folder.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn report(data: &Path) -> Report {
    let b = read_marker(&data.join(BACKUP_MARKER)).unwrap_or_default(); // WHAT: the backup marker. WHY: unknown when absent.
    let d = read_marker(&data.join(DRILL_MARKER)).unwrap_or_default(); // WHAT: the drill marker. WHY: the same.
    let f = read_marker(&data.join(FAILED_MARKER)).unwrap_or_default(); // WHAT: the failure marker. WHY: the same.
    let iso = |t: time::OffsetDateTime| {
        t.format(&time::format_description::well_known::Rfc3339)
            .ok()
    }; // WHAT: back to text. WHY: one form in the report.
    Report {
        last_good: past_time(b.last_good.as_deref()).and_then(iso), // WHAT: a real past time, or None. WHY: never a guess.
        r#where: b
            .r#where
            .filter(|_| past_time(b.last_good.as_deref()).is_some()) // WHAT: only with a real last good time. WHY: never a guess
            .map(|w| w.chars().take(200).collect()), // WHAT: where, with a good time only. WHY: half a marker is no marker.
        restore_tested: if d.ok == Some(true) {
            past_time(d.at.as_deref()).and_then(iso) // WHAT: the drill's time, if real. WHY: never a guess
        } else {
            None
        }, // WHAT: a passed drill's time only. WHY: a failed drill proves nothing.
        last_failed: past_time(f.at.as_deref()).and_then(iso), // WHAT: a real past time, or None. WHY: never a guess.
        last_failed_why: f
            .why
            .filter(|_| past_time(f.at.as_deref()).is_some()) // WHAT: only with a real failure time. WHY: never a guess
            .map(|w| w.chars().take(300).collect()), // WHAT: why, with a good time only. WHY: half a marker is no marker.
    }
}

/// Days since the last confirmed off-site backup, or `None` for never.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn backup_days(data: &Path) -> Option<f64> {
    let t = past_time(read_marker(&data.join(BACKUP_MARKER))?.last_good.as_deref())?; // WHAT: its time. WHY: unknown when absent or damaged.
    Some(((time::OffsetDateTime::now_utc() - t).as_seconds_f64() / 86_400.0).max(0.0))
    // WHAT: in days. WHY: Fleet's backupDays.
}

// ---------------------------------------------------------------- the job

/// Everything one run needs.
pub struct Job<'a> {
    pub product: &'a Product, // WHAT: which product. WHY: its files and checks.
    pub data: PathBuf, // WHAT: the live data folder. WHY: what is backed up, and where the markers go.
    pub work: PathBuf, // WHAT: the job's own folder (outside the data). WHY: programs, public key, spool and log.
    pub target: Target, // WHAT: where it goes. WHY: a folder or a bucket.
    pub cipher: Cipher, // WHAT: age. WHY: the lock.
    pub version: String, // WHAT: this server's version. WHY: in the manifest.
    pub mark: bool, // WHAT: whether a good run and a passed drill are marked. WHY: only for the server's own target; a hand run elsewhere moves nothing.
}

/// A run's end: an exit code and its reason. 0 good · 5 good, with a weekly/monthly or prune warning · 2 refused
/// before anything was marked · 3 the upload failed or did not read back · 4 bad usage.
#[derive(Debug)]
pub struct Exit {
    pub code: i32,
    pub why: String,
} // WHAT: code and reason. WHY: the scheduled task shows the code; the log shows the reason.

fn refuse(code: i32, why: impl std::fmt::Display) -> Exit {
    Exit {
        code,
        why: why.to_string(),
    }
} // WHAT: make one. WHY: short.

impl Job<'_> {
    fn own(&self) -> PathBuf {
        self.work.join(self.product.id)
    } // WHAT: this product's corner of the work folder. WHY: several products can share one work folder.

    /// One line to the console and the job's log.
    pub fn log(&self, line: &str) {
        let line = format!(
            "{}  {line}",
            time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()
        ); // WHAT: stamped. WHY: read the next morning.
        println!("{line}"); // WHAT: to the console. WHY: a person running it sees it.
        let path = self.own().join("offsite.log"); // WHAT: the log file. WHY: beside the job.
        if std::fs::metadata(&path)
            .is_ok_and(|m| m.len() > 4 * 1024 * 1024) // WHAT: whether the log is past 4 MB. WHY: false when it can't be read
        {
            let _ = std::fs::rename(&path, self.own().join("offsite.log.1"));
        } // WHAT: one old log kept past 4 MB. WHY: a log must never fill the disk.
        let mut o = std::fs::OpenOptions::new();
        o.create(true).append(true); // WHAT: append, making it if new. WHY: one log.
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600); // WHAT: owner-only when made. WHY: everything in the work folder is this account's alone.
        if let Ok(mut f) = o.open(&path) {
            let _ = writeln!(f, "{line}");
        } // WHAT: append. WHY: best effort; the console still has it.
    }

    /// The nightly backup. Returns the exit code to end with.
    pub fn backup(&self) -> Exit {
        let e = self.backup_run(); // WHAT: the run. WHY: its end is recorded below.
        if self.mark {
            // WHAT: only for the server's own target. WHY: a hand run elsewhere says nothing about the nightly copy.
            let failed = self.data.join(FAILED_MARKER); // WHAT: the failure marker. WHY: the ops report's lastFailed.
            if e.code == 0 || e.code == 5 {
                let _ = std::fs::remove_file(&failed);
            }
            // WHAT: a good night clears it. WHY: the failure is over.
            else if !e.why.starts_with(BUSY) {
                let m = Marker {
                    at: time::OffsetDateTime::now_utc()
                        .format(&time::format_description::well_known::Rfc3339)
                        .ok(),
                    why: Some(e.why.chars().take(300).collect()),
                    ..Marker::default()
                };
                if let Err(w) = serde_json::to_vec_pretty(&m)
                    .map_err(anyhow::Error::from)
                    .and_then(|b| write_atomic(&failed, &b))
                {
                    self.log(&format!("WARNING: could not write {FAILED_MARKER}: {w:#}"));
                }
            } // WHAT: when and why it failed. WHY: Fleet sees a failed night, not only an old one; a run already going is not a failure.
        }
        e
    }

    fn backup_run(&self) -> Exit {
        let own = self.own(); // WHAT: the job's folder. WHY: lock, stage and spool live there.
        if inside(&own, &self.data) || inside(&self.data, &own) {
            return refuse(4, "the work folder and the data folder must be apart: a backup inside what it backs up grows forever");
        } // WHAT: refuse overlap. WHY: the spool would be backed up into itself.
        if let Err(e) =
            create_private_dir(&own).and_then(|()| create_private_dir(&own.join("spool"))) // WHAT: the folder, then its spool. WHY: the spool only when its folder is made
        {
            return refuse(2, format!("could not make {}: {e}", own.display()));
        } // WHAT: make it, owner-only. WHY: the first run; the stage inside holds the database unencrypted.
        let lock = match hold_lock(&own.join("offsite.lock")) {
            Ok(l) => l,
            Err(e) => return e,
        }; // WHAT: one run at a time, held by the system. WHY: a crashed run's lock goes with it; a first upload may run past any age a stale-lock rule could pick.
        let stage = own.join("stage"); // WHAT: the plaintext stage. WHY: deleted at the start and the end of every run.
        let _ = std::fs::remove_dir_all(&stage); // WHAT: clear what a killed run left. WHY: the database copy is unencrypted.
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.backup_locked(&stage)))
                .unwrap_or_else(|p| {
                    Err(refuse(
                        3,
                        format!("the backup stopped part way: {}", panic_words(p.as_ref())),
                    ))
                }); // WHAT: the run itself; a fault inside is a failed run. WHY: a failure is always reported and cleaned up, never escapes.
        let _ = std::fs::remove_dir_all(&stage); // WHAT: delete the plaintext stage. WHY: it holds the database unencrypted.
        let _ = std::fs::remove_dir_all(own.join("outgoing")); // WHAT: encrypted drawings not yet sent. WHY: tomorrow lists the bucket again.
        drop(lock); // WHAT: free the lock; the file stays. WHY: deleting it could let two runs each hold a lock on a different file.
        match &result { Ok(0) => self.log("DONE. Tonight's backup is off this machine and marked."), Ok(c) => self.log(&format!("DONE, with housekeeping warnings above (exit {c}). Tonight's backup is safe and marked.")), Err(e) => self.log(&format!("FAILED: {}", e.why)) } // WHAT: the last line. WHY: the one to read in the morning.
        match result {
            Ok(c) => Exit {
                code: c,
                why: String::new(),
            },
            Err(e) => e,
        } // WHAT: the result. WHY: the exit code.
    }

    #[allow(clippy::too_many_lines)] // WHAT: the night's numbered steps, in one function. WHY: each step leans on what the one before checked and holds; the backup is security code, and splitting it is a change of its own to review apart
    fn backup_locked(&self, stage: &Path) -> std::result::Result<i32, Exit> {
        #[cfg(test)]
        assert!(!FAULT.with(std::cell::Cell::get), "a fault planted by a test"); // WHAT: the tests' fault, when one is planted. WHY: proves a fault becomes a failed run.
        let own = self.own(); // WHAT: the job's folder. WHY: spool and salt.
        let recipients = self.work.join("age-recipients.txt"); // WHAT: the public key file. WHY: Creede puts it there.
        let keys = check_recipients(&std::fs::read_to_string(&recipients).unwrap_or_default())
            .map_err(|e| refuse(2, format!("{e} ({})", recipients.display())))?; // WHAT: check it first. WHY: refuse before anything is copied.
        if let Some(why) = others_can_change(&self.work) {
            return Err(refuse(2, format!("{why}; only the system, administrators and this server's own account may change the work folder, its bin and rclone.conf (README.md, Off-site backups)")));
        } // WHAT: refuse a work folder others could change. WHY: whoever can swap bin/age or rclone.conf can read or redirect every backup.
        if let Some(found) = private_key_in(&self.work) {
            return Err(refuse(2, format!("{} holds a PRIVATE key. Only the public line (age1...) belongs on this server: move that file off this machine, to Creede", found.display())));
        } // WHAT: a private key anywhere in the work folder. WHY: the key that opens every backup must never sit beside them.
        if let Target::Rclone { config, exe, .. } = &self.target {
            if !config.exists() || !exe.exists() {
                return Err(refuse(2, format!("rclone or its settings are missing ({} / {}). Creede sets these up once (README.md, Off-site backups).", exe.display(), config.display())));
            }
        } // WHAT: the bucket's settings. WHY: say what's missing, not an rclone error at night.
        if let Some(exe) = self.cipher.program() {
            if !exe.exists() {
                return Err(refuse(2, format!("age is missing ({})", exe.display())));
            }
        } // WHAT: the lock program. WHY: the same.
        self.log(&format!("off-site backup of {} starting -> {} ({keys} key{} can open it; none of them is on this machine)", self.product.id, self.target.words(), if keys > 1 { "s" } else { "" })); // WHAT: say what is about to happen. WHY: the log reads like a story.

        // 1. a consistent copy of the database, while the server goes on working
        let started = time::OffsetDateTime::now_utc(); // WHAT: when the data is frozen. WHY: the backup's age counts from here.
        create_private_dir(stage).map_err(|e| refuse(2, e))?; // WHAT: the stage, owner-only. WHY: the database copy goes there unencrypted.
        let live_db = self.data.join(self.product.database); // WHAT: the live database. WHY: copied.
        if !live_db.is_file() {
            return Err(refuse(
                2,
                format!(
                    "there is no database at {}; is this the right data folder?",
                    live_db.display()
                ),
            ));
        } // WHAT: refuse a wrong folder. WHY: an empty backup must never be marked good.
        let copy = stage.join(self.product.database); // WHAT: the stage copy. WHY: packed into the catalog.
        snapshot(&live_db, &copy)
            .map_err(|e| refuse(2, format!("could not copy the database: {e:#}")))?; // WHAT: SQLite copies itself consistently. WHY: a file copy of a database in use can be one that won't open.
        let (counts, refs) = check_database(self.product, &copy)
            .map_err(|e| refuse(2, format!("the database copy did not check out: {e:#}")))?; // WHAT: open it and count. WHY: a copy that won't open is never sent.
        self.log(&format!(
            "database copied and checked: {}",
            counts
                .iter()
                .map(|(k, v)| format!("{k} {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        )); // WHAT: the counts. WHY: the log shows what was backed up.
        let installation = installation_of(&copy).map_err(|e| refuse(2, format!("{e:#}")))?; // WHAT: whose data this is. WHY: the target is checked against it next.
        let claimed = self
            .target
            .claim(self.product.id, &installation, &own)?; // WHAT: the target is this office's, or is made so. WHY: before anything is sent or pruned (audit M2).
        self.log(&claimed); // WHAT: say whose it is. WHY: the log shows the check ran.

        // 2. the drawing files: the ones on disk, each sent once
        let mut on_disk = list_blobs(&self.data.join(self.product.blobs))
            .map_err(|e| refuse(2, format!("could not list the files: {e:#}")))?; // WHAT: every file by digest. WHY: what must be in the bucket.
        let mut missing_on_disk: Vec<String> = refs
            .iter()
            .filter(|d| !on_disk.contains_key(*d))
            .cloned()
            .collect(); // WHAT: files the database names that aren't here. WHY: damage the backup must report.
        if !missing_on_disk.is_empty() {
            self.log(&format!("WARNING: {} file(s) the database names are not on this disk (first: {:.12}). They can't be backed up; the rest is.", missing_on_disk.len(), missing_on_disk[0]));
        } // WHAT: say so. WHY: never hidden.
        let salt = salt(&own)
            .map_err(|e| refuse(2, format!("could not read or make the name salt: {e:#}")))?; // WHAT: the salt for object names. WHY: the bucket doesn't say which files it holds.
        let there: BTreeMap<String, u64> = self
            .target
            .list("blobs")
            .map_err(|e| refuse(3, format!("could not list the bucket's files: {e:#}")))?
            .into_iter()
            .collect(); // WHAT: what the bucket already holds. WHY: send only what is new.
        let index_path = own.join("blob-sizes.json"); // WHAT: each sent file's encrypted size, by object name. WHY: the read-back compares exact sizes, also for files sent on earlier nights.
        let mut sizes: BTreeMap<String, u64> =
            read_sizes(&index_path).map_err(|e| refuse(2, format!("{e:#}")))?; // WHAT: the sizes so far. WHY: as above.
        let outgoing = own.join("outgoing"); // WHAT: encrypted drawings waiting to go. WHY: sent in batches.
        let _ = std::fs::remove_dir_all(&outgoing); // WHAT: start empty. WHY: a killed run's leftovers are made again.
        let (mut sent, mut sent_bytes, mut batch_bytes) = (0usize, 0u64, 0u64); // WHAT: tallies. WHY: the log and the batch size.
        let mut damaged = Vec::new(); // WHAT: files whose bytes don't match their name. WHY: never sent, and reported.
        let mut warned = 0usize; // WHAT: warnings that leave tonight's backup good but not clean. WHY: exit 5, for someone to look at.
        let send_batch = |sizes: &mut BTreeMap<String, u64>| -> std::result::Result<(), Exit> { // WHAT: named apart from `sent`, the count. WHY: the two read alike
            // WHAT: send one batch and note each file's encrypted size. WHY: the sizes the read-back expects.
            let mut batch = BTreeMap::new(); // WHAT: this batch's sizes. WHY: kept before sending, so a batch cut short is checked, not unknown.
            for e in std::fs::read_dir(&outgoing)
                .map_err(|e| refuse(2, e))?
                .flatten()
            {
                batch.insert(
                    e.file_name().to_string_lossy().into_owned(),
                    e.metadata().map_err(|e| refuse(2, e))?.len(),
                );
            } // WHAT: each encrypted file's size. WHY: as sent.
            sizes.extend(batch);
            write_atomic(
                &index_path,
                &serde_json::to_vec(&*sizes).map_err(|e| refuse(2, e))?,
            )
            .map_err(|e| {
                refuse(
                    2,
                    format!("could not keep the sizes of the files being sent: {e:#}"),
                )
            })?; // WHAT: keep the sizes first. WHY: whatever the send does, tomorrow knows what each object should be.
            self.target
                .put_all(&outgoing, "blobs")
                .map_err(|e| refuse(3, format!("sending files failed: {e:#}")))?; // WHAT: send them. WHY: off the box.
            let _ = std::fs::remove_dir_all(&outgoing);
            Ok(()) // WHAT: empty the spool. WHY: never holds the whole data.
        };
        for (digest, (path, size)) in &on_disk {
            // WHAT: each file. WHY: the new ones are sent.
            let obj = object_for(&salt, digest); // WHAT: its object name. WHY: salted.
            if there.contains_key(&obj) {
                continue;
            } // WHAT: already there. WHY: files never change, so once is enough; the manifest keeps the fingerprint already in the bucket.
            if !matches_digest(path, digest).unwrap_or(false) {
                self.log(&format!(
                    "WARNING: {digest:.12} on disk doesn't match its own fingerprint; not sent" // WHAT: the warning, naming the file. WHY: the log says which
                ));
                damaged.push(digest.clone());
                continue;
            } // WHAT: check before sending. WHY: never send damaged bytes as good ones.
            #[cfg(test)]
            if CHANGE_WHILE_SENT.with(std::cell::Cell::get) { // WHAT: whether a test changes a file while it is sent. WHY: read from its cell
                std::fs::write(path, b"changed while it was being sent")
                    .map_err(|e| refuse(2, e))?;
            } // WHAT: the tests' change between the check and the send. WHY: the fingerprint taken while sending must catch it.
            std::fs::create_dir_all(&outgoing).map_err(|e| refuse(2, e))?; // WHAT: the batch folder. WHY: made when needed.
            let out = outgoing.join(&obj); // WHAT: the encrypted file. WHY: sent with the batch.
            let as_sent = self
                .cipher
                .encrypt_file(&recipients, path, &out)
                .map_err(|e| refuse(2, format!("encrypting a file failed: {e:#}")))?; // WHAT: lock it, fingerprinting the bytes as they go. WHY: nothing leaves unencrypted, and what left is known.
            if as_sent != *digest {
                let _ = std::fs::remove_file(&out);
                self.log(&format!(
                    "WARNING: {digest:.12} changed while it was being sent; not sent tonight" // WHAT: the warning, naming the file. WHY: likewise
                ));
                damaged.push(digest.clone());
                continue;
            } // WHAT: the bytes sent aren't the ones named. WHY: an object must hold exactly what its name says.
            sent += 1;
            sent_bytes += *size;
            batch_bytes += *size; // WHAT: count it. WHY: the log and the batch.
            if batch_bytes > 1 << 30 {
                send_batch(&mut sizes)?; // WHAT: a batch sent. WHY: at each gigabyte
                batch_bytes = 0;
            } // WHAT: send each gigabyte. WHY: the spool never holds the whole data.
        }
        if outgoing.exists() {
            send_batch(&mut sizes)?; // WHAT: the last batch sent. WHY: whatever is left
        } // WHAT: the last batch. WHY: everything new is sent.
        if !damaged.is_empty() || !missing_on_disk.is_empty() {
            warned += 1;
        } // WHAT: something the database names isn't in tonight's backup. WHY: good with a warning, never plain good.
        for d in &damaged {
            on_disk.remove(d);
            missing_on_disk.push(d.clone());
        } // WHAT: damaged files are not in this backup. WHY: the manifest lists only what the bucket holds.
        let after: BTreeMap<String, u64> = self
            .target
            .list("blobs")
            .map_err(|e| refuse(3, format!("could not read the bucket back: {e:#}")))?
            .into_iter()
            .collect(); // WHAT: read the bucket back. WHY: only what is really there counts.
        let unknown = on_disk
            .keys()
            .filter(|d| !sizes.contains_key(&object_for(&salt, d)))
            .count(); // WHAT: files in the bucket whose sent size this job never noted. WHY: their size can't be checked.
        if unknown > 0 {
            return Err(refuse(3, format!("{unknown} file(s) in the target have no record of the size they were sent at ({}); not marking. If that file was lost, move it and name-salt.txt aside: the next run sends every file again under new names", index_path.display())));
        } // WHAT: refuse. WHY: only an exact size proves the object whole.
        let absent = on_disk
            .keys()
            .filter(|d| !after.contains_key(&object_for(&salt, d)))
            .count(); // WHAT: files not in the target at all. WHY: a catalog must never name one.
        if absent > 0 {
            return Err(refuse(
                3,
                format!("{absent} file(s) are not in the target after sending; not marking"),
            ));
        } // WHAT: refuse. WHY: the next night sends them.
        let wrong: Vec<String> = on_disk
            .keys()
            .filter_map(|d| {
                let o = object_for(&salt, d);
                match (after.get(&o), sizes.get(&o)) {
                    (Some(a), Some(w)) if a != w => {
                        Some(format!("blobs/{o} is {a} bytes, sent as {w}"))
                    }
                    _ => None,
                }
            })
            .collect(); // WHAT: objects of another size than sent, by name. WHY: someone can find and delete each one.
        if !wrong.is_empty() {
            return Err(refuse(3, format!("{} file(s) are not in the target at the size they were sent; not marking. Delete each from the target and the next night sends it again: {}", wrong.len(), wrong.iter().take(5).cloned().collect::<Vec<_>>().join("; "))));
        } // WHAT: refuse, naming up to five. WHY: a short or padded object is not the one sent, and is never written over.
        self.log(&format!(
            "files: {} in all, {sent} new sent ({:.1} MB), every one read back",
            on_disk.len(),
            megabytes(sent_bytes)
        )); // WHAT: the tally. WHY: upload time and cost follow from it.

        // 3. the catalog: database and manifest, packed, gzipped and locked
        let name = object_name(self.product.id, started); // WHAT: tonight's name. WHY: dated to the frozen data.
        let manifest = Manifest {
            format: "excalibur-offsite-1".into(),
            product: self.product.id.into(),
            name: name.clone(),
            created_at: started
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            app_version: self.version.clone(),
            database_sha256: sha256_file(&copy).map_err(|e| refuse(2, e))?,
            counts: counts.clone(),
            salt: salt.clone(),
            blobs: on_disk.iter().map(|(d, (_, s))| (d.clone(), *s)).collect(),
            missing_on_disk,
            installation: Some(installation.clone()),
            company: company_of(&copy),
        }; // WHAT: the manifest. WHY: what the drill checks against.
        let mj = serde_json::to_vec_pretty(&manifest).map_err(|e| refuse(2, e))?; // WHAT: as JSON. WHY: packed first.
        let spool = own.join("spool");
        let part = spool.join(format!("{name}.part"));
        let out = spool.join(&name); // WHAT: where the encrypted catalog is made. WHY: .part until it is whole.
        let db_len = std::fs::metadata(&copy).map_err(|e| refuse(2, e))?.len(); // WHAT: the copy's size. WHY: the tar header.
        let packed = self.cipher.encrypt_stream(&recipients, &part, |w| {
            let mut gz = flate2::write::GzEncoder::new(w, flate2::Compression::new(6));
            {
                let mut t = TarOut { w: &mut gz };
                t.file("manifest.json", mj.len() as u64, &mut &mj[..])?;
                t.file(
                    self.product.database,
                    db_len,
                    &mut std::fs::File::open(&copy)?,
                )?;
                t.end()?;
            }
            gz.finish()?;
            Ok(())
        }); // WHAT: manifest and database, tarred, gzipped, into age. WHY: never unencrypted on disk.
        if let Err(e) = packed {
            let _ = std::fs::remove_file(&part);
            return Err(refuse(2, format!("packing or encrypting failed: {e:#}")));
        } // WHAT: remove a half file. WHY: never sent.
        std::fs::rename(&part, &out).map_err(|e| refuse(2, e))?; // WHAT: its real name. WHY: only now whole.
        let age_sha = sha256_file(&out).map_err(|e| refuse(2, e))?;
        let age_bytes = std::fs::metadata(&out).map_err(|e| refuse(2, e))?.len(); // WHAT: fingerprint and size. WHY: the sidecar and the read-back.
        let side = Sidecar {
            format: manifest.format.clone(),
            name: name.clone(),
            created_at: manifest.created_at.clone(),
            app_version: self.version.clone(),
            age_sha256: age_sha.clone(),
            age_bytes,
            blob_count: on_disk.len(),
            blob_bytes: on_disk.values().map(|(_, s)| s).sum(),
        }; // WHAT: the unencrypted facts. WHY: no names, no records.
        let side_path = spool.join(format!("{name}.json"));
        write_atomic(
            &side_path,
            &serde_json::to_vec_pretty(&side).map_err(|e| refuse(2, e))?,
        )
        .map_err(|e| refuse(2, e))?; // WHAT: write it, owner-only. WHY: sent beside the catalog.

        // 4. send the catalog and read it back
        self.target
            .put(&out, &format!("daily/{name}"))
            .map_err(|e| refuse(3, format!("the upload failed: {e:#}")))?; // WHAT: send it. WHY: off the box.
        self.target
            .put(&side_path, &format!("daily/{name}.json"))
            .map_err(|e| refuse(3, format!("the sidecar upload failed: {e:#}")))?; // WHAT: and its sidecar. WHY: the drill checks the fingerprint.
        let daily = self
            .target
            .list("daily")
            .map_err(|e| refuse(3, format!("could not read the bucket back: {e:#}")))?; // WHAT: read back. WHY: the proof.
        if !daily.iter().any(|(n, s)| *n == name && *s == age_bytes) {
            return Err(refuse(3, format!("the bucket does not hold tonight's catalog at full size ({age_bytes} bytes); not marking")));
        } // WHAT: refuse a short or missing one. WHY: only a confirmed copy counts.
        self.log(&format!(
            "catalog sent and read back: daily/{name} ({age_bytes} bytes)"
        )); // WHAT: note it. WHY: the important line.

        // 5. mark it: only now does Fleet hear of it, and only for the target the server is set to
        if self.mark { // WHAT: this server's own target. WHY: only then does Fleet hear of it
            let marker = Marker {
                last_good: Some(manifest.created_at.clone()),
                r#where: Some(self.target.words()),
                object: Some(format!("daily/{name}")),
                ..Marker::default()
            }; // WHAT: the marker. WHY: backupDays and where.
            write_atomic(
                &self.data.join(BACKUP_MARKER),
                &serde_json::to_vec_pretty(&marker).map_err(|e| refuse(3, e))?,
            )
            .map_err(|e| refuse(3, format!("sent, but could not write the marker: {e:#}")))?;
            // WHAT: write it whole. WHY: Fleet never reads half a file.
        } else { // WHAT: another target. WHY: not marked
            // WHAT: a hand run elsewhere. WHY: a USB disk tried once must not stand for the nightly off-site copy.
            self.log(&format!("sent to {}, which isn't this server's own off-site target: not marked, backupDays unchanged", self.target.words())); // WHAT: said in the log. WHY: Fleet hears nothing of it
        } // WHAT: the end of the marking. WHY: skipped for another target.

        // 6. weekly / monthly copies and the keep rule
        let mut house = warned; // WHAT: housekeeping failures, and the warnings above. WHY: exit 5, after the backup is already safe.
        let mut listing = BTreeMap::new(); // WHAT: each folder's names. WHY: the keep rule.
        for f in ["daily", "weekly", "monthly"] {
            match self.target.list(f) {
                Ok(v) => {
                    listing.insert(
                        f.to_string(),
                        v.into_iter().map(|(n, _)| n).collect::<Vec<_>>(),
                    );
                }
                Err(e) => {
                    house += 1;
                    self.log(&format!("WARNING: could not list {f}: {e:#}"));
                    listing.insert(f.to_string(), Vec::new());
                }
            }
        } // WHAT: list each. WHY: a folder that won't list is skipped, not guessed.
        let p = plan(self.product.id, &listing, &name).map_err(|e| refuse(3, e))?; // WHAT: the plan. WHY: the keep rule.
        for (g, want) in [("weekly", p.weekly), ("monthly", p.monthly)] {
            if want {
                // WHAT: the first of its week or month. WHY: it stands for that week or month.
                match self
                    .target
                    .copy(&format!("daily/{name}"), &format!("{g}/{name}"))
                    .and_then(|()| { // WHAT: then its sidecar. WHY: only when the catalog copied
                        self.target
                            .copy(&format!("daily/{name}.json"), &format!("{g}/{name}.json"))
                    }) {
                    Ok(()) => self.log(&format!("{g} copy: {g}/{name}")),
                    Err(e) => {
                        house += 1;
                        self.log(&format!(
                            "WARNING: the {g} copy failed (tomorrow tries again): {e:#}"
                        ));
                    }
                } // WHAT: copy inside the target. WHY: nothing is sent again.
            }
        }
        for (folder, n) in &p.deletes {
            // WHAT: past the keep. WHY: 7 / 4 / 12.
            match self.target.delete(&format!("{folder}/{n}")) {
                Ok(()) => {
                    let _ = self.target.delete(&format!("{folder}/{n}.json"));
                    self.log(&format!("pruned {folder}/{n}"));
                }
                Err(e) => {
                    house += 1;
                    self.log(&format!("WARNING: could not delete {folder}/{n} (a bucket lock may still hold it; tomorrow tries again): {e:#}"));
                }
            } // WHAT: delete it and its sidecar. WHY: a lock refusing is the lock working.
        }

        // 7. the spool keeps tonight's encrypted catalog only, for a quick restore on this box
        if let Ok(rd) = std::fs::read_dir(&spool) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n != name && n != format!("{name}.json") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        } // WHAT: older ones go. WHY: they are in the bucket already.
        Ok(if house > 0 { 5 } else { 0 }) // WHAT: 0, or 5 for housekeeping. WHY: the scheduled task shows a non-zero result to look at.
    }
}

/// Takes the run's lock: the file opened (Windows: with no sharing) and locked by the system, so it is released when the
/// run ends or the process dies. `Err` is the exit to end with.
fn hold_lock(path: &Path) -> std::result::Result<std::fs::File, Exit> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true).write(true).create(true).truncate(false); // WHAT: open or make it, never emptied. WHY: its contents don't matter, only the hold.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600); // WHAT: owner-only when made. WHY: everything in the work folder is this account's alone.
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::share_mode(&mut o, 0); // WHAT: no sharing. WHY: a second run can't even open it.
    let f = match o.open(path) {
        Ok(f) => f,
        Err(e) if e.raw_os_error() == Some(32) => {
            return Err(refuse(2, format!("{BUSY} ({})", path.display())))
        }
        Err(e) => {
            return Err(refuse(
                2,
                format!("could not open the lock {}: {e}", path.display()),
            ))
        }
    }; // WHAT: open it; Windows' sharing violation is a run going. WHY: say which.
    match f.try_lock() {
        Ok(()) => Ok(f),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err(refuse(2, format!("{BUSY} ({})", path.display())))
        }
        Err(std::fs::TryLockError::Error(e)) => {
            Err(refuse(2, format!("could not lock {}: {e}", path.display())))
        }
    } // WHAT: the system's lock. WHY: never two at once.
}

/// A fault's message, for the log.
fn panic_words(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(ToString::to_string) // WHAT: a panic's &str, as text. WHY: the log says what stopped it
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "an unexpected fault".into()) // WHAT: the text a panic carries. WHY: the log says what stopped it.
}

#[cfg(test)]
thread_local! {
    /// The tests' fault inside a run, and the tests' file changed while it is sent.
    pub(crate) static FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; // WHAT: panic inside the run. WHY: proves it becomes exit 3.
    pub(crate) static PRETEND_UID: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) }; // WHAT: a different account for this server. WHY: makes every file look like a stranger's.
    pub(crate) static LOOSEN_SCRATCH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; // WHAT: open the scratch folder up just after it is made. WHY: proves the check after making it.
    pub(crate) static FAIL_PUT_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) }; // WHAT: uploads that succeed before one fails. WHY: proves a batch cut short doesn't block later nights.
    pub(crate) static CHANGE_WHILE_SENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; // WHAT: rewrite a file between its check and its encryption. WHY: proves the fingerprint taken while sending decides.
}

/// SQLite's own consistent copy of a database in use (VACUUM INTO), then made a single file.
///
/// # Errors
///
/// Fails when the database can't be copied.
pub fn snapshot(live: &Path, to: &Path) -> Result<()> {
    let db = Connection::open_with_flags(
        live,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?; // WHAT: read-only. WHY: the backup can never change the live data.
    db.busy_timeout(std::time::Duration::from_secs(30))?; // WHAT: wait for a writer. WHY: the server may be mid-save.
    db.execute("VACUUM INTO ?1", [to.to_string_lossy()])?; // WHAT: copy it. WHY: one transaction, so the copy is one moment, whole.
    Ok(())
}

/// Opens a copy, checks it, counts it, and lists the files it names.
///
/// # Errors
///
/// Fails when the copy can't be opened or does not pass its check.
pub fn check_database(
    product: &Product,
    path: &Path,
) -> Result<(BTreeMap<String, i64>, BTreeSet<String>)> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?; // WHAT: read-only. WHY: checking never changes it.
    let ok: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?; // WHAT: SQLite's own check. WHY: a damaged copy is caught here.
    if ok != "ok" {
        bail!("integrity check: {ok}");
    } // WHAT: refuse it. WHY: the same.
    let mut counts = BTreeMap::new(); // WHAT: rows per table. WHY: the manifest.
    for t in product.counts {
        counts.insert(
            t.to_string(),
            db.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| {
                r.get::<_, i64>(0)
            })?,
        );
    } // WHAT: count each. WHY: table names are constants.
    let refs = (product.references)(&db)?; // WHAT: the files it names. WHY: each must be backed up.
    if let Some(bad) = refs.iter().find(|d| !is_digest(d)) {
        bail!("the database names a file that isn't a digest: {bad:.16}");
    } // WHAT: refuse a strange one. WHY: digests become paths.
    Ok((counts, refs))
}

fn is_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
} // WHAT: 64 lowercase hex letters. WHY: the store's names.

/// Every file in the store, by digest: its path and size. `aa/bb/digest`, nothing else.
///
/// # Errors
///
/// Fails when the store can't be read or holds something that isn't a stored file.
pub fn list_blobs(root: &Path) -> Result<BTreeMap<String, (PathBuf, u64)>> {
    let mut out = BTreeMap::new(); // WHAT: the files. WHY: sorted, so runs are repeatable.
    if !root.exists() {
        return Ok(out);
    } // WHAT: none yet. WHY: a new server.
    for a in std::fs::read_dir(root)? {
        let a = a?;
        if !a.file_type()?.is_dir() {
            continue;
        } // WHAT: the first level. WHY: two letters each.
        for b in std::fs::read_dir(a.path())? {
            let b = b?;
            if !b.file_type()?.is_dir() {
                continue;
            } // WHAT: the second level. WHY: the same.
            for f in std::fs::read_dir(b.path())? {
                let f = f?;
                let n = f.file_name().to_string_lossy().into_owned();
                let ft = f.file_type()?; // WHAT: each file. WHY: named by its digest.
                if ft.is_file() && is_digest(&n) {
                    out.insert(n, (f.path(), f.metadata()?.len()));
                } // WHAT: a stored file. WHY: .part files and strangers are left out.
            }
        }
    }
    Ok(out)
}

fn matches_digest(path: &Path, digest: &str) -> Result<bool> {
    Ok(sha256_file(path)? == digest)
} // WHAT: the file is what its name says. WHY: never send damaged bytes as good ones.

/// The object name of a drawing file: a salted digest, so the bucket doesn't say which files it holds.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn object_for(salt: &str, digest: &str) -> String {
    format!(
        "{}.age",
        hex(&Sha256::new()
            .chain_update(salt.as_bytes())
            .chain_update(b":")
            .chain_update(digest.as_bytes())
            .finalize())
    )
} // WHAT: sha256(salt:digest).age. WHY: the same file gets the same name every night.

fn salt(own: &Path) -> Result<String> {
    let path = own.join("name-salt.txt"); // WHAT: kept in the job's folder. WHY: the same names every night; it also travels inside each catalog.
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim().to_string();
        if s.len() == 64 && is_digest(&s) {
            return Ok(s);
        }
        bail!("{} is damaged; move it aside and the next run makes a new one (every file is then sent again)", path.display());
    } // WHAT: read it. WHY: never silently replaced.
    let s = hex(&rand::random::<[u8; 32]>()); // WHAT: 32 random bytes. WHY: unguessable.
    write_atomic(&path, s.as_bytes())?;
    Ok(s) // WHAT: keep it. WHY: tomorrow uses it.
}

/// The encrypted sizes of the files sent, by object name. A missing file is an empty record; a damaged one is refused.
fn read_sizes(path: &Path) -> Result<BTreeMap<String, u64>> {
    match std::fs::read(path) { Ok(b) => serde_json::from_slice(&b).with_context(|| format!("{} is damaged; move it and name-salt.txt aside and the next run sends every file again", path.display())), Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()), Err(e) => Err(anyhow!("reading {}: {e}", path.display())) }
    // WHAT: read it. WHY: never silently replaced.
}

/// A size in megabytes, for a log line.
#[allow(clippy::cast_precision_loss)] // WHAT: a byte count widened to a float. WHY: shown to a tenth of a megabyte; a float holds every size below 8 PB exactly, and no conversion without a cast exists from a 64-bit number
fn megabytes(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0 // WHAT: bytes to megabytes. WHY: the same tally the log has always shown
}

fn hex(b: &[u8]) -> String {
    crate::store::hex(b) // WHAT: the store's hex writer. WHY: one way of writing hex in the server
} // WHAT: bytes as hex. WHY: names and fingerprints.

/// The SHA-256 of a file, as lowercase hex, read in pieces.
///
/// # Errors
///
/// Fails when the file can't be read.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?; // WHAT: open it. WHY: streamed.
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20]; // WHAT: the hash and a buffer. WHY: a big file is never held whole.
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    } // WHAT: read through. WHY: the fingerprint.
    Ok(hex(&h.finalize()))
}

/// A new file only its owner can read, owner-only from the moment it exists (never made open and then closed up).
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true); // WHAT: a new file, never over one. WHY: a name twice is refused, not overwritten.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600); // WHAT: owner read and write only, set as it is made. WHY: a restored database or drawing is as private as the original, even for a moment.
    o.open(path) // WHAT: make it. WHY: the caller writes into it.
}

/// A folder (and any parents it needs) only its owner can open.
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true); // WHAT: make it and its parents. WHY: the first run.
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700); // WHAT: owner only, set as it is made. WHY: the stage and the restore hold plaintext.
    b.create(path) // WHAT: make it. WHY: an existing folder is fine.
}

/// A new folder, made owner-only as it is made and refused if anything is already there. Unix: 0700. Windows: inheritance
/// removed, full control for the person running it and the system only.
fn new_private_dir(path: &Path) -> Result<()> {
    if let Some(up) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(up)?;
    } // WHAT: its parent. WHY: an ordinary folder above it.
    #[cfg(not(windows))]
    {
        let mut b = std::fs::DirBuilder::new(); // WHAT: not recursive. WHY: an existing folder is an error, not reused.
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700); // WHAT: owner only, set as it is made. WHY: never open to others, even for a moment.
        b.create(path)
            .with_context(|| format!("making {}", path.display()))?;
    } // WHAT: make it. WHY: fails if anything is already there.
    #[cfg(windows)]
    {
        let me = winacl::me()?;
        winacl::create_dir(path, &me, &[&me, winacl::SYSTEM])?;
    } // WHAT: born with only this account and the system, nothing inherited. WHY: no moment when another account could plant a junction in it.
    #[cfg(all(test, unix))]
    if LOOSEN_SCRATCH.with(std::cell::Cell::get) { // WHAT: whether a test loosens the scratch folder. WHY: read from its cell
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    } // WHAT: the tests' folder opened up after it is made. WHY: proves the check below catches it.
    #[cfg(all(test, windows))]
    if LOOSEN_SCRATCH.with(|l| l.get()) {
        let ok = std::process::Command::new("icacls").arg(path).args(["/grant", "*S-1-5-32-545:(OI)(CI)M"]).output()?; // WHAT: let every account (BUILTIN\Users) change it, with Windows' own tool. WHY: the same proof on Windows.
        anyhow::ensure!(ok.status.success(), "icacls: {ok:?}"); // WHAT: the loosening itself worked. WHY: else the test would prove nothing.
    } // WHAT: the tests' folder opened up after it is made (Windows). WHY: proves the check below catches it there too.
    check_private_dir(path) // WHAT: look at what was made. WHY: trust it only once checked.
}

/// A just-made folder is ours alone: a real folder (no link or reparse point), empty, and owner-only (Unix 0700; Windows:
/// protected, owned by this account, only it and the system allowed).
fn check_private_dir(path: &Path) -> Result<()> {
    let m = std::fs::symlink_metadata(path)
        .with_context(|| format!("looking at {}", path.display()))?; // WHAT: the folder itself. WHY: never what a link points at.
    if !m.is_dir() || m.file_type().is_symlink() {
        bail!("{} is not a plain folder", path.display());
    } // WHAT: a real folder. WHY: a link sends the restore elsewhere.
    #[cfg(windows)]
    if std::os::windows::fs::MetadataExt::file_attributes(&m) & 0x400 != 0 {
        bail!("{} is a reparse point", path.display());
    } // WHAT: no junction. WHY: the same.
    if std::fs::read_dir(path)
        .with_context(|| format!("reading {}", path.display()))?
        .next()
        .is_some()
    {
        bail!("{} is not empty", path.display());
    } // WHAT: nothing in it yet. WHY: anything there was planted.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = m.permissions().mode() & 0o777;
        if mode != 0o700 {
            bail!("{} is mode {mode:o}, not 0700", path.display());
        }
    } // WHAT: owner only. WHY: never open to others.
    #[cfg(windows)]
    {
        let me = winacl::me()?;
        winacl::is_only(path, &me, &[&me, winacl::SYSTEM])?;
    } // WHAT: protected, ours, only us and the system. WHY: the restore inherits it.
    Ok(())
}

/// The first link or reparse point under `dir`, if any.
fn link_inside(dir: &Path) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()]; // WHAT: folders to look in. WHY: the whole restore.
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            return Some(d);
        }; // WHAT: a folder that can't be read is a refusal. WHY: what it hides can't be checked.
        for e in rd {
            let Ok(e) = e else { return Some(d) }; // WHAT: an entry that can't be read. WHY: the same.
            let Ok(m) = std::fs::symlink_metadata(e.path()) else {
                return Some(e.path());
            }; // WHAT: the entry itself. WHY: one that can't be looked at isn't trusted.
            #[cfg(windows)]
            if std::os::windows::fs::MetadataExt::file_attributes(&m) & 0x400 != 0 {
                return Some(e.path());
            } // WHAT: FILE_ATTRIBUTE_REPARSE_POINT. WHY: a junction points elsewhere like a link.
            if m.file_type().is_symlink() {
                return Some(e.path());
            } // WHAT: a link. WHY: it would point outside the restore.
            if m.is_dir() {
                stack.push(e.path());
            } // WHAT: look inside. WHY: every level.
        }
    }
    None
}

const SIDECAR_CAP: u64 = 64 * 1024; // WHAT: the largest sidecar read. WHY: a real one is a few hundred bytes.
const MANIFEST_CAP: u64 = 64 * 1024 * 1024; // WHAT: the largest manifest read. WHY: a million files' digests fit; a planted giant is refused unread.

/// Reads a file only if it is a plain file within `cap` bytes.
fn read_capped(path: &Path, cap: u64) -> Result<Vec<u8>> {
    let m =
        std::fs::symlink_metadata(path).with_context(|| format!("reading {}", path.display()))?; // WHAT: its size, before reading. WHY: refused unread.
    if !m.is_file() || m.len() > cap {
        bail!(
            "{} is {} bytes, over the {cap} allowed; refusing it",
            path.display(),
            m.len()
        );
    } // WHAT: the cap. WHY: never read a planted giant into memory.
    Ok(std::fs::read(path)?)
}

/// The manifest as this program writes it: the product's own tables counted, every fingerprint 64 lowercase hex.
fn valid_manifest(product: &Product, man: &Manifest) -> Result<()> {
    let tables: BTreeSet<&str> = man.counts.keys().map(String::as_str).collect(); // WHAT: the tables it counts. WHY: compared below.
    if tables != product.counts.iter().copied().collect::<BTreeSet<_>>() {
        bail!("its counts are for other tables");
    } // WHAT: exactly the product's tables. WHY: a count missing is a check skipped.
    if man.counts.values().any(|n| *n < 0) {
        bail!("a count is below zero");
    } // WHAT: real counts. WHY: written by SQLite's count.
    if !is_digest(&man.database_sha256) || !is_digest(&man.salt) {
        bail!("the database fingerprint or the salt is not 64 hex digits");
    } // WHAT: the two fingerprints. WHY: they name and check files.
    if let Some((d, _)) = man.blobs.iter().find(|(d, _)| !is_digest(d)) {
        bail!("a file fingerprint is not 64 hex digits: {d:.16}"); // WHAT: refused, naming the start of it. WHY: a bad digest must never become a path
    } // WHAT: every file. WHY: digests become paths and object names.
    if let Some(d) = man.missing_on_disk.iter().find(|d| !is_digest(d)) {
        bail!(
            "a missing file's fingerprint is not 64 hex digits: {d:.16}" // WHAT: the refusal, naming the start of it. WHY: a bad digest must never become a path
        );
    } // WHAT: the same. WHY: compared with the database.
    if man.installation.as_deref().is_some_and(|i| !is_installation(i)) {
        bail!("its office's installation id isn't one the server makes");
    } // WHAT: plain characters. WHY: printed in the drill's summary.
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp"); // WHAT: beside it. WHY: one rename puts it in place.
    let _ = std::fs::remove_file(&tmp);
    create_private(&tmp)?.write_all(bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(()) // WHAT: write owner-only, then rename. WHY: a reader never sees half a file, and no other account can read or change it.
}

fn real(p: &Path) -> PathBuf {
    let mut base = p.to_path_buf();
    let mut rest = Vec::new(); // WHAT: the nearest folder that exists, and what is below it. WHY: a folder not made yet still has a real place.
    while !base.exists() {
        match (
            base.file_name().map(std::ffi::OsStr::to_os_string), // WHAT: this folder's own name. WHY: walked back up below
            base.parent().map(Path::to_path_buf),
        ) {
            (Some(n), Some(up)) => {
                rest.push(n);
                base = up;
            }
            _ => break,
        }
    } // WHAT: climb to one that exists. WHY: only that can be resolved.
    let mut out = std::fs::canonicalize(&base).unwrap_or(base); // WHAT: its real path. WHY: links and .. can't hide overlap.
    for n in rest.into_iter().rev() {
        out.push(n);
    }
    out // WHAT: put the rest back. WHY: the full path.
}

fn inside(a: &Path, b: &Path) -> bool {
    real(a).starts_with(real(b))
} // WHAT: a is b or under it, by real paths. WHY: overlap check that links can't fool.

// ---------------------------------------------------------------- the drill

/// What a drill is asked to do.
pub struct Drill<'a> {
    pub job: Job<'a>, // WHAT: the product, target and cipher. WHY: the same as the backup's.
    pub identity: PathBuf, // WHAT: Creede's private key (on a USB stick). WHY: never kept on the server.
    pub scratch: PathBuf,  // WHAT: an empty folder for the restore. WHY: never the live data.
    pub object: Option<String>, // WHAT: a catalog to drill, or the newest. WHY: any copy can be proven.
    pub full: bool, // WHAT: every file, or a sample. WHY: a full restore is the disaster case; a sample is the monthly check.
    pub sample: usize, // WHAT: how many files to check when not full. WHY: proves the files open without fetching them all.
    pub keep: bool,    // WHAT: keep the restored copy. WHY: the real restore uses --full --keep.
}

impl Drill<'_> {
    /// Runs the drill. RESTORE DRILL PASSED, or the reason it didn't.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn run(&self) -> Exit {
        let identity = match std::path::absolute(&self.identity) {
            Ok(p) => p,
            Err(e) => return refuse(4, format!("the key file {}: {e}", self.identity.display())),
        }; // WHAT: the key's full path. WHY: the opened key goes beside it, never in whatever folder this was started from.
        if let Some(why) = others_can_change(&self.job.work) {
            return refuse(2, format!("{why}; the drill runs bin/age with the private key, so only the system, administrators and this account may change the work folder"));
        } // WHAT: refuse a work folder others could change. WHY: a swapped age would be handed the key.
        clear_leftover_keys(&identity, &self.job); // WHAT: opened keys an interrupted drill left. WHY: they open every backup.
        guard_on_interrupt(); // WHAT: Ctrl+C or a stop signal deletes the opened key and the restore. WHY: they must not outlive the drill.
        if std::fs::symlink_metadata(&self.scratch).is_ok() {
            return refuse(
                4,
                format!(
                    "the scratch folder {} must not exist yet: the drill makes it, owner-only",
                    self.scratch.display()
                ),
            );
        } // WHAT: refuse any folder, file or link already there. WHY: a folder made earlier may be open to others, or hold anything.
        if inside(&self.scratch, &self.job.data) || inside(&self.job.data, &self.scratch) {
            return refuse(
                4,
                "the scratch folder must be apart from the live data folder",
            );
        } // WHAT: refuse overlap. WHY: a restore must never land on the real data.
        if let Err(e) = new_private_dir(&self.scratch) {
            let _ = std::fs::remove_dir_all(&self.scratch);
            return refuse(
                4,
                format!(
                    "could not make {} owner-only: {e:#}",
                    self.scratch.display()
                ),
            );
        }
        hold_secret(&self.scratch); // WHAT: the restore is deleted on Ctrl+C. WHY: it is plaintext. // WHAT: make it, owner-only from the start. WHY: the restore goes there in plaintext, and inherits it.
        let key = match OpenedKey::open(&self.job.cipher, &identity) {
            Ok(k) => k,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&self.scratch);
                let_go(&self.scratch);
                return refuse(2, format!("{e:#}"));
            }
        }; // WHAT: a locked key opened once. WHY: a drill decrypts many files; one passphrase, not one per file.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_in(&key.path)))
            .unwrap_or_else(|p| {
                Err(refuse(
                    3,
                    format!("the drill stopped part way: {}", panic_words(p.as_ref())),
                ))
            }); // WHAT: the drill; a fault inside is a failed drill. WHY: the clean-up below runs whatever happened.
        drop(key); // WHAT: delete the opened key now. WHY: it opens every backup.
        if !self.keep || r.is_err() {
            let _ = std::fs::remove_dir_all(&self.scratch);
        } // WHAT: delete the restored copy. WHY: it is plaintext.
        let_go(&self.scratch); // WHAT: the drill is over. WHY: a kept restore is the person's now.
        match r {
            Ok(summary) if !self.job.mark => {
                self.job.log(&format!("RESTORE DRILL PASSED (from {}, not this server's own target: not marked): {summary}", self.job.target.words()));
                Exit {
                    code: 0,
                    why: String::new(),
                }
            } // WHAT: a drill of another target. WHY: restoreTested speaks for the server's own off-site copy only.
            Ok(summary) => {
                let m = Marker {
                    ok: Some(true),
                    at: time::OffsetDateTime::now_utc()
                        .format(&time::format_description::well_known::Rfc3339)
                        .ok(),
                    object: Some(summary.clone()),
                    ..Marker::default()
                }; // WHAT: the drill marker. WHY: restoreTested.
                if let Err(e) = write_atomic(
                    &self.job.data.join(DRILL_MARKER),
                    &serde_json::to_vec_pretty(&m).unwrap_or_default(),
                ) {
                    self.job.log(&format!(
                        "WARNING: passed, but could not write {DRILL_MARKER}: {e:#}" // WHAT: the warning, naming the marker. WHY: the log says what wasn't written
                    ));
                } // WHAT: write it. WHY: Fleet shows when a restore was last proven.
                self.job.log(&format!("RESTORE DRILL PASSED: {summary}"));
                Exit {
                    code: 0,
                    why: String::new(),
                } // WHAT: the result. WHY: the line to look for.
            }
            Err(e) => {
                self.job.log(&format!("RESTORE DRILL FAILED: {}", e.why));
                e
            } // WHAT: the reason. WHY: a failed drill marks nothing.
        }
    }

    #[allow(clippy::too_many_lines)] // WHAT: the drill's steps, in one function. WHY: as for the backup: each check leans on the one before, and splitting it is a change of its own to review apart
    fn run_in(&self, identity: &Path) -> std::result::Result<String, Exit> {
        let j = &self.job;
        let dl = self.scratch.join("download");
        std::fs::create_dir_all(&dl).map_err(|e| refuse(4, e))?; // WHAT: the download folder. WHY: inside the scratch.
        let name = if let Some(o) = &self.object { // WHAT: the catalog asked for. WHY: drilled as named
            o.clone() // WHAT: the name asked for. WHY: drilled as named
        } else { // WHAT: none named. WHY: the newest anywhere is drilled
            let mut all = Vec::new();
            for f in ["daily", "weekly", "monthly"] {
                for (n, _) in j
                    .target
                    .list(f)
                    .map_err(|e| refuse(3, format!("could not list the bucket: {e:#}")))?
                {
                    if believable(j.product.id, &n, time::OffsetDateTime::now_utc()) {
                        all.push(format!("{f}/{n}"));
                    }
                }
            }
            all.sort_by_key(|k| k.rsplit('/').next().unwrap_or("").to_string());
            all.pop()
                .ok_or_else(|| refuse(3, "there is no backup in the target to drill"))?
        }; // WHAT: the catalog, or the newest anywhere. WHY: --latest by default.
        let base = name.rsplit('/').next().unwrap_or(&name).to_string(); // WHAT: its own name. WHY: checked against the manifest.
        if parse_name(j.product.id, &base).is_none() || !key_ok(&name) {
            return Err(refuse(
                4,
                format!("{name} isn't one of this product's catalogs"),
            ));
        } // WHAT: refuse a stranger. WHY: only our names are fetched.
        let (folder, _) = name
            .rsplit_once('/')
            .filter(|(f, _)| ["daily", "weekly", "monthly"].contains(f))
            .ok_or_else(|| refuse(4, format!("{name} isn't in daily/, weekly/ or monthly/")))?; // WHAT: its folder. WHY: listed first.
        let listed: BTreeMap<String, u64> = j
            .target
            .list(folder)
            .map_err(|e| refuse(3, format!("could not list {folder}: {e:#}")))?
            .into_iter()
            .collect(); // WHAT: what is there and how big, before fetching anything. WHY: sizes are checked before a byte comes down.
        let side_size = *listed
            .get(&format!("{base}.json"))
            .ok_or_else(|| refuse(3, format!("{name}'s sidecar is missing")))?; // WHAT: the sidecar's listed size. WHY: checked next.
        if side_size > SIDECAR_CAP {
            return Err(refuse(2, format!("the sidecar is listed at {side_size} bytes, over the {SIDECAR_CAP} a sidecar can be; not fetched")));
        } // WHAT: refuse a giant sidecar unfetched. WHY: it would fill the scratch drive.
        j.target
            .get(&format!("{name}.json"), &dl.join("sidecar.json"))
            .map_err(|e| refuse(3, format!("its sidecar is missing: {e:#}")))?; // WHAT: the sidecar first. WHY: it says how big the catalog must be.
        let side: Sidecar = serde_json::from_slice(
            &read_capped(&dl.join("sidecar.json"), SIDECAR_CAP)
                .map_err(|e| refuse(2, format!("the sidecar: {e:#}")))?,
        )
        .map_err(|e| refuse(2, format!("the sidecar doesn't read: {e}")))?; // WHAT: read it, if small. WHY: the fingerprint and size to compare.
        let cat_size = listed.get(&base).copied(); // WHAT: the catalog's listed size. WHY: compared with the sidecar before fetching.
        if cat_size != Some(side.age_bytes) {
            return Err(refuse(
                2,
                format!(
                    "the catalog is listed at {} bytes and its sidecar says {}; not fetched",
                    cat_size.map_or_else(|| "no".into(), |n| n.to_string()), // WHAT: the catalog's listed size, or "no". WHY: said in the refusal
                    side.age_bytes
                ),
            ));
        } // WHAT: refuse a catalog of another size. WHY: a hostile one can't fill the scratch drive.
        let enc = dl.join(&base);
        j.target
            .get(&name, &enc)
            .map_err(|e| refuse(3, format!("download failed: {e:#}")))?; // WHAT: now the catalog. WHY: its size is known good.
        if side.name != base || sha256_file(&enc).map_err(|e| refuse(2, e))? != side.age_sha256 {
            return Err(refuse(
                2,
                "the downloaded catalog is not the one that was uploaded (its fingerprint differs)",
            ));
        } // WHAT: compare. WHY: a damaged or swapped download is caught before decrypting.
        j.log(&format!("downloaded {name}; its fingerprint matches")); // WHAT: note it. WHY: step one passed.
        let tgz = dl.join("catalog.tar.gz");
        j.cipher
            .decrypt_file(identity, &enc, &tgz)
            .map_err(|e| refuse(2, format!("{e:#}")))?; // WHAT: decrypt with Creede's key. WHY: only that key opens it.
        let restored = self.scratch.join("restored"); // WHAT: the restored data folder. WHY: what a real restore copies into place.
        let mut gz =
            flate2::read::GzDecoder::new(std::fs::File::open(&tgz).map_err(|e| refuse(2, e))?); // WHAT: gunzip. WHY: the catalog is gzipped.
        let names = untar(
            &mut gz,
            &restored,
            64 << 30,
            &[
                ("manifest.json", MANIFEST_CAP),
                (j.product.database, 64 << 30),
            ],
        )
        .map_err(|e| refuse(2, format!("unpacking failed: {e:#}")))?;
        let _ = std::fs::remove_file(&tgz); // WHAT: unpack safely, then drop the plaintext tarball. WHY: only our names, never outside.
        if names.len() != 2
            || !names.iter().any(|n| n == "manifest.json")
            || !names.iter().any(|n| n == j.product.database)
        {
            return Err(refuse(
                2,
                format!("the catalog holds {names:?}, not a manifest and a database"),
            ));
        } // WHAT: exactly the two files. WHY: anything else is not ours.
        let man: Manifest = serde_json::from_slice(
            &read_capped(&restored.join("manifest.json"), MANIFEST_CAP)
                .map_err(|e| refuse(2, format!("the manifest: {e:#}")))?,
        )
        .map_err(|e| refuse(2, format!("the manifest doesn't read: {e}")))?;
        let _ = std::fs::remove_file(restored.join("manifest.json")); // WHAT: read it if within the cap, then leave a clean data folder. WHY: what the drill checks against.
        valid_manifest(j.product, &man).map_err(|e| {
            refuse(
                2,
                format!("the manifest is not one this program writes: {e:#}"),
            )
        })?; // WHAT: every field as written. WHY: a planted manifest can't steer what is fetched or where it goes.
        if man.format != "excalibur-offsite-1" || man.product != j.product.id || man.name != base {
            return Err(refuse(
                2,
                "the manifest is for another product, format or catalog",
            ));
        } // WHAT: the right one. WHY: never check against the wrong manifest.
        let owner = j
            .target
            .owner(j.product.id, &dl)?; // WHAT: whose target this is. WHY: compared with whose catalog it is.
        if let (Some(o), Some(i)) = (&owner, &man.installation) {
            if o.installation != *i {
                return Err(refuse(2, format!("{name} was made by office {i}, but this target belongs to office {}; it was put here by someone else, and is not restored", o.installation)));
            }
        } // WHAT: a catalog from another office in this office's target. WHY: never restored as this office's data (audit M2).
        let office = match (&man.installation, &man.company) {
            (Some(i), Some(c)) if !c.is_empty() => format!("{c}, office {i}"),
            (Some(i), _) => format!("office {i}"),
            (None, _) => "an office not named (made before catalogs named their office)".to_string(),
        }; // WHAT: whose backup this is, in words. WHY: the summary says what was restored and whose it was.
        let db = restored.join(j.product.database); // WHAT: the restored database. WHY: checked next.
        if sha256_file(&db).map_err(|e| refuse(2, e))? != man.database_sha256 {
            return Err(refuse(
                2,
                "the restored database's fingerprint differs from the manifest",
            ));
        } // WHAT: compare. WHY: the bytes are the ones backed up.
        let (counts, refs) = check_database(j.product, &db)
            .map_err(|e| refuse(2, format!("the restored database doesn't check out: {e:#}")))?; // WHAT: open, check, count. WHY: it must open.
        if counts != man.counts {
            return Err(refuse(
                2,
                format!(
                    "the restored counts differ from the manifest: {counts:?} vs {:?}",
                    man.counts
                ),
            ));
        } // WHAT: compare. WHY: nothing lost.
        let listed: BTreeSet<String> = man.blobs.iter().map(|(d, _)| d.clone()).collect(); // WHAT: the files the manifest lists. WHY: checked against the database and the bucket.
        if let Some(d) = refs
            .iter()
            .find(|d| !listed.contains(*d) && !man.missing_on_disk.contains(*d))
        {
            return Err(refuse(
                2,
                format!(
                    "the database names a file the manifest doesn't list ({d:.12})" // WHAT: the refusal, naming the file. WHY: a hole is found now
                ),
            ));
        } // WHAT: every named file accounted for. WHY: a hole is found now, not on the bad day.
        let there: BTreeMap<String, u64> = j
            .target
            .list("blobs")
            .map_err(|e| refuse(3, format!("could not list the bucket's files: {e:#}")))?
            .into_iter()
            .collect(); // WHAT: the bucket's files. WHY: each listed one must be there.
        if let Some((d, _)) = man.blobs.iter().find(|(d, s)| {
            there
                .get(&object_for(&man.salt, d))
                .is_none_or(|&b| b <= *s) // WHAT: missing, or short of its size. WHY: either is a hole
        }) {
            return Err(refuse(
                2,
                format!(
                    "a file the backup lists is not in the bucket at full size ({d:.12})" // WHAT: the refusal, naming the file. WHY: likewise
                ),
            ));
        } // WHAT: all present. WHY: a missing file is a hole in the backup.
        let record = (j.product.record)(
            &Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| refuse(2, e))?,
        )
        .map_err(|e| refuse(2, e))?; // WHAT: one real record and its file. WHY: opened below.
        let mut want: Vec<String> = if self.full {
            man.blobs.iter().map(|(d, _)| d.clone()).collect()
        } else {
            let mut v: Vec<String> = man.blobs.iter().map(|(d, _)| d.clone()).collect();
            let mut r = rand::thread_rng();
            rand::seq::SliceRandom::shuffle(&mut v[..], &mut r);
            v.truncate(self.sample);
            v
        }; // WHAT: every file, or a random sample. WHY: monthly checks needn't fetch it all.
        if let Some((_, d)) = &record {
            if listed.contains(d) && !want.contains(d) {
                want.push(d.clone());
            }
        } // WHAT: the record's own file too. WHY: the record is opened whole.
        let enc_dir = dl.join("blobs");
        let objs: Vec<String> = want.iter().map(|d| object_for(&man.salt, d)).collect();
        j.target
            .get_many("blobs", &objs, &enc_dir)
            .map_err(|e| refuse(3, format!("downloading files failed: {e:#}")))?; // WHAT: fetch them. WHY: decrypted and checked next.
        let files = restored.join(j.product.blobs); // WHAT: the restored file store. WHY: the same layout as the live one.
        for d in &want {
            // WHAT: each fetched file. WHY: it must decrypt and match its name.
            let to = files.join(&d[0..2]).join(&d[2..4]);
            let out = to.join(d); // WHAT: its place. WHY: aa/bb/digest.
            if !is_digest(d) || !out.starts_with(&self.scratch) {
                return Err(refuse(
                    2,
                    "a restored file would land outside the scratch folder; refusing it",
                ));
            } // WHAT: stays inside. WHY: a restore never writes anywhere else.
            create_private_dir(&to).map_err(|e| refuse(2, e))?; // WHAT: its folder, owner-only. WHY: plaintext inside.
            j.cipher
                .decrypt_file(identity, &enc_dir.join(object_for(&man.salt, d)), &out)
                .map_err(|e| refuse(2, format!("a file wouldn't decrypt: {e:#}")))?; // WHAT: decrypt it. WHY: with the same key.
            if sha256_file(&out).map_err(|e| refuse(2, e))? != *d {
                return Err(refuse(
                    2,
                    format!("a restored file doesn't match its fingerprint ({d:.12})"), // WHAT: the refusal, naming the start of it. WHY: a restore that differs is not a restore
                ));
            } // WHAT: compare. WHY: every byte the same.
        }
        let _ = std::fs::remove_dir_all(&dl); // WHAT: drop the downloads. WHY: encrypted copies no longer needed.
        if let Some(l) = link_inside(&self.scratch) {
            return Err(refuse(2, format!("{} is a link or reparse point inside the scratch folder; refusing to open the restore", l.display())));
        } // WHAT: no links anywhere in the restore. WHY: nothing it opens may point elsewhere.
        let opened = if let Some((id, d)) = &record { // WHAT: the newest set, if the backup has one. WHY: its drawing is read below
            // WHAT: open one real record through the server's own store. WHY: the app reads it, not just this check.
            let store = crate::Store::open(&db, &files).map_err(|e| {
                refuse(
                    2,
                    format!("the server could not open the restored data: {e:#}"),
                )
            })?;
            let bytes = store.get_blob(d).map_err(|e| {
                refuse(
                    2,
                    format!("the server could not read record {id}'s file: {e:#}"),
                )
            })?;
            format!(
                "record {id} opened ({} bytes, {})",
                bytes.len(),
                if bytes.starts_with(b"%PDF") {
                    "a PDF"
                } else {
                    "a file"
                }
            )
        } else { // WHAT: the newest set, its drawing read, above; an empty server here. WHY: proof the restore is usable, or still that it opens
            crate::Store::open(&db, &files).map_err(|e| {
                refuse(
                    2,
                    format!("the server could not open the restored data: {e:#}"),
                )
            })?;
            "no records yet; the server opened the data".to_string()
        }; // WHAT: what was opened. WHY: said in the summary
        Ok(format!("{name} ({office}): counts match ({}), {} of {} files fetched and fingerprint-checked{}, every file present in the target, {opened}", counts.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "), want.len(), man.blobs.len(), if self.full { " (full restore)" } else { "" }))
        // WHAT: the summary. WHY: the log and the marker say what was proven.
    }
}

/// Creede's key for one drill: a locked key (age's own format) is opened once, beside itself on his USB stick, never on
/// the server's disk, and deleted when this is dropped, pass or fail. An unlocked key is used as it is.
struct OpenedKey {
    path: PathBuf,
    delete: bool,
} // WHAT: the key file to decrypt with, and whether it is ours to delete. WHY: an opened key is deleted, the original never.

impl OpenedKey {
    fn open(cipher: &Cipher, identity: &Path) -> Result<OpenedKey> {
        let mut head = [0u8; 21];
        let n = std::fs::File::open(identity)
            .with_context(|| format!("reading the key file {}", identity.display()))?
            .read(&mut head)?; // WHAT: the file's first bytes. WHY: a locked key starts with age's header.
        if !(head[..n].starts_with(b"age-encryption.org/v1") || head[..n].starts_with(b"FAKEPASS1"))
        {
            return Ok(OpenedKey {
                path: identity.to_path_buf(),
                delete: false,
            });
        } // WHAT: not locked. WHY: used as it is.
        let out = key_beside(identity)?; // WHAT: beside the key, on the stick. WHY: the opened key never touches the server's disk.
        hold_secret(&out); // WHAT: deleted on Ctrl+C, even mid-prompt. WHY: it opens every backup.
        if let Err(e) = cipher.unlock(identity, &out) {
            let _ = std::fs::remove_file(&out);
            let_go(&out);
            return Err(e);
        }
        Ok(OpenedKey {
            path: out,
            delete: true,
        }) // WHAT: age asks for the passphrase once. WHY: one prompt for the whole drill.
    }
}

impl Drop for OpenedKey {
    fn drop(&mut self) {
        if self.delete {
            let _ = std::fs::remove_file(&self.path);
            let_go(&self.path);
        }
    }
} // WHAT: delete the opened key. WHY: it opens every backup; even a panic removes it.

// ---------------------------------------------------------------- the settings, the command line and the night

/// The job the settings describe: the data folder, the work folder and the target. `None` when either setting is empty.
///
/// # Errors
///
/// Fails when the work folder or the target can't be read.
pub fn job_from(
    product: &'static Product,
    data: PathBuf,
    work: Option<&str>,
    target: Option<&str>,
    version: &str,
) -> Result<Option<Job<'static>>> {
    let (Some(work), Some(spec)) = (
        work.filter(|w| !w.trim().is_empty()),
        target.filter(|t| !t.trim().is_empty()),
    ) else {
        return Ok(None);
    }; // WHAT: both set, or off. WHY: off until someone sets it up.
    let work = std::path::absolute(work).with_context(|| format!("the work folder {work}"))?; // WHAT: the work folder as a full path. WHY: age and rclone are run by their full path, never looked up on the PATH.
    let target = Target::parse(spec, &work)?; // WHAT: where it goes. WHY: a malformed target is refused before anything runs.
    let age = work
        .join("bin")
        .join(if cfg!(windows) { "age.exe" } else { "age" }); // WHAT: age beside the job. WHY: never whatever is on the PATH.
    Ok(Some(Job {
        product,
        data,
        work,
        target,
        cipher: Cipher::Age(age),
        version: version.to_string(),
        mark: true,
    })) // WHAT: the job, marking. WHY: the same for the night, the command and the drill; the command clears `mark` for another target.
}

/// The time of night, "1:30" or "01:30", 24-hour, the server's own clock.
///
/// # Errors
///
/// Fails, saying what is wanted, when it isn't a time of day.
pub fn night_time(at: &str) -> Result<chrono::NaiveTime> {
    let t = at.trim(); // WHAT: without spaces. WHY: a setting typed with a space still reads.
    let ok = t.len() >= 4
        && t.len() <= 5
        && t.as_bytes()[t.len() - 3] == b':'
        && t.chars().filter(|c| *c != ':').all(|c| c.is_ascii_digit()); // WHAT: H:MM or HH:MM only. WHY: "1.30" or "1:30am" is refused, not guessed.
    let parsed = if ok {
        chrono::NaiveTime::parse_from_str(t, "%H:%M").ok()
    } else {
        None
    }; // WHAT: read it. WHY: 25:00 or 1:61 is refused.
    parsed.ok_or_else(|| anyhow!("HYPERVIEW_OFFSITE_AT is \"{at}\", which isn't a time of day as hours and minutes, 24-hour (\"01:30\")"))
    // WHAT: say what is wanted. WHY: found as the server starts, not at night.
}

/// Checks the off-site settings as the server starts: a typo is found now, not at night.
///
/// # Errors
///
/// Fails, saying which setting is wrong, when they can't work together.
pub fn check_settings(
    data: &Path,
    work: Option<&str>,
    target: Option<&str>,
    at: &str,
) -> Result<()> {
    night_time(at)?; // WHAT: the time reads. WHY: the night must come.
    let target = target.filter(|t| !t.trim().is_empty()); // WHAT: the target, if set. WHY: without one the backup is off.
    let Some(spec) = target else { return Ok(()) }; // WHAT: off. WHY: nothing else to check.
    let Some(work) = work.filter(|w| !w.trim().is_empty()) else {
        bail!("HYPERVIEW_OFFSITE_TARGET is set, so HYPERVIEW_OFFSITE_WORK must be too: a folder apart from the data folder for the backup's programs, key and log")
    }; // WHAT: refuse a half setting. WHY: the backup can't run without its folder.
    let work = std::path::absolute(work)?; // WHAT: the full path. WHY: compared below.
    if inside(&work, data) || inside(data, &work) {
        bail!("HYPERVIEW_OFFSITE_WORK ({}) must be apart from the data folder ({}): a backup inside what it backs up grows forever", work.display(), data.display());
    } // WHAT: refuse overlap. WHY: the spool would back itself up.
    Target::parse(spec, &work)?; // WHAT: the target reads. WHY: a malformed one is refused now.
    Ok(())
}

/// Whether tonight's backup is due: past the time, none confirmed since it, and no failed try in the last hour.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn due(
    now: chrono::NaiveDateTime,
    at: chrono::NaiveTime,
    last_good: Option<chrono::NaiveDateTime>,
    last_try: Option<chrono::NaiveDateTime>,
) -> bool {
    let from = now.date().and_time(at); // WHAT: today at the time. WHY: once a day, from then on.
    if now < from {
        return false;
    } // WHAT: not yet. WHY: last night's covers until then.
    if last_good.is_some_and(|g| g >= from) {
        return false;
    } // WHAT: done today. WHY: one a day.
    last_try.is_none_or(|t| now - t >= chrono::Duration::hours(1)) // WHAT: no failed try, or the last an hour or more ago; one under an hour ago waits. WHY: tried again gently, not every look.
}

/// How often the server looks whether tonight's backup is due.
pub const LOOK_EVERY: std::time::Duration = std::time::Duration::from_secs(5 * 60); // WHAT: five minutes. WHY: close enough to the time, and nothing to a server.

/// Tonight's backup, run by the server itself: once a day, the first look after the time, retried an hour after a failure.
pub async fn nightly(server: crate::api::Shared) {
    let Ok(at) = night_time(&server.config.offsite_at) else {
        return;
    }; // WHAT: the time. WHY: checked as the server started; never guessed.
    let mut last_try: Option<chrono::NaiveDateTime> = None; // WHAT: the last failed run. WHY: retried an hour later.
    loop {
        tokio::time::sleep(LOOK_EVERY).await; // WHAT: wait between looks. WHY: never a busy loop.
        let now = chrono::Local::now().naive_local(); // WHAT: the office's clock. WHY: "01:30" means the office's half past one.
        let good = report(&server.config.data)
            .last_good
            .and_then(|g| chrono::DateTime::parse_from_rfc3339(&g).ok())
            .map(|g| g.with_timezone(&chrono::Local).naive_local()); // WHAT: the last confirmed backup, on the office's clock. WHY: compared with tonight's time.
        if !due(now, at, good, last_try) {
            continue;
        } // WHAT: not due. WHY: once a night.
        let job = match job_from(
            &VIEW,
            server.config.data.clone(),
            server.config.offsite_work.as_deref(),
            server.config.offsite_target.as_deref(),
            crate::VERSION,
        ) {
            Ok(Some(j)) => j,
            Ok(None) => return,
            Err(e) => {
                tracing::error!("the off-site backup's settings don't read: {e:#}");
                return;
            }
        }; // WHAT: the job. WHY: the settings were checked as the server started.
        last_try = Some(now); // WHAT: note the try. WHY: a failure waits an hour.
        match tokio::task::spawn_blocking(move || job.backup()).await { // WHAT: run it off the request threads. WHY: it reads and sends for minutes or hours.
            Ok(e) if e.code == 0 || e.code == 5 => tracing::info!("tonight's off-site backup ended with exit {} (0 good, 5 good with a warning)", e.code), // WHAT: say so in the server's log. WHY: beside the job's own log.
            Ok(e) => tracing::error!("tonight's off-site backup did not finish (exit {}): {}. Nothing is marked; it tries again in an hour", e.code, e.why), // WHAT: say why. WHY: Fleet's backupDays also stays old until one passes.
            Err(e) => tracing::error!("tonight's off-site backup stopped part way: {e}"), // WHAT: a panic in the job. WHY: the server keeps serving.
        }
    }
}

/// Whether a run stands for the server's own off-site copy: the target and the work folder given (`None`: as set) are
/// the configured ones, compared as real places.
pub fn marks(
    set_target: Option<&str>,
    set_work: Option<&str>,
    target: Option<&str>,
    work: Option<&str>,
) -> bool {
    let set_target = set_target.map(str::trim).filter(|t| !t.is_empty()); // WHAT: the setting. WHY: none set means nothing marks.
    let same_target = match (set_target, target.map(str::trim)) {
        (Some(_), None) => true,
        (Some(a), Some(b)) => same_place(a, b),
        _ => false,
    }; // WHAT: the same target. WHY: a USB disk tried once is not the off-site copy.
    let same_work = match (set_work, work) {
        (_, None) => true,
        (Some(a), Some(b)) => real_of(a) == real_of(b),
        (None, Some(_)) => false,
    }; // WHAT: the same work folder. WHY: another one has its own salt and sizes.
    same_target && same_work
}

fn real_of(p: &str) -> Option<PathBuf> {
    std::path::absolute(p.trim()).ok().map(|a| real(&a))
} // WHAT: a folder's real place. WHY: two spellings of one folder are the same.

fn same_place(a: &str, b: &str) -> bool {
    match (a.strip_prefix("rclone:"), b.strip_prefix("rclone:")) {
        (Some(x), Some(y)) => x.trim_end_matches('/') == y.trim_end_matches('/'),
        (None, None) => real_of(a).is_some() && real_of(a) == real_of(b),
        _ => false,
    } // WHAT: the same remote, or the same real folder. WHY: compared as places, not text.
}

/// Where an opened key goes: a new name beside the key file, by its full path.
fn key_beside(identity: &Path) -> Result<PathBuf> {
    let full = std::path::absolute(identity)?; // WHAT: the key's full path. WHY: a relative one has no folder of its own.
    Ok(full.parent().unwrap_or(Path::new("/")).join(format!(
        ".drill-key-{}.txt",
        hex(&rand::random::<[u8; 6]>())
    ))) // WHAT: beside it. WHY: on the stick, never the server's disk.
}

/// Deletes opened keys an interrupted drill left beside the key file, and says so.
fn clear_leftover_keys(identity: &Path, job: &Job) {
    let Some(dir) = identity.parent() else { return }; // WHAT: the key's folder. WHY: where opened keys go.
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned(); // WHAT: its name. WHY: only ours are touched.
        if n.starts_with(".drill-key-")
            && std::path::Path::new(&n).extension().is_some_and(|x| x == "txt") // WHAT: ends in .txt, exactly as written. WHY: only the drill's own opened keys
            && std::fs::remove_file(e.path()).is_ok()
        {
            job.log(&format!(
                "removed {n}, an opened key an earlier drill left behind beside {}",
                identity.display()
            ));
        } // WHAT: delete it and say so. WHY: it opens every backup.
    }
}

/// The files and folders a drill must not leave behind if it is stopped: the opened key and the restore.
static SECRETS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new()); // WHAT: what to delete on Ctrl+C. WHY: a stopped drill leaves no plaintext or key.

fn hold_secret(p: &Path) {
    if let Ok(mut v) = SECRETS.lock() {
        v.push(p.to_path_buf());
    }
} // WHAT: note it. WHY: deleted if stopped.
fn let_go(p: &Path) {
    if let Ok(mut v) = SECRETS.lock() {
        v.retain(|x| x != p);
    }
} // WHAT: forget it. WHY: deleted already, or kept on purpose.

/// Deletes every noted secret (or, for a test, those under one folder): what Ctrl+C or a stop signal runs before the drill ends.
pub(crate) fn delete_secrets(under: Option<&Path>) {
    let held: Vec<PathBuf> = SECRETS
        .lock()
        .map(|mut v| {
            let (go, stay): (Vec<PathBuf>, Vec<PathBuf>) = v
                .drain(..)
                .partition(|p| under.is_none_or(|u| p.starts_with(u)));
            *v = stay;
            go
        })
        .unwrap_or_default(); // WHAT: take the ones to delete. WHY: each is deleted once.
    for p in held {
        let _ = if p.is_dir() {
            std::fs::remove_dir_all(&p)
        } else {
            std::fs::remove_file(&p)
        };
    } // WHAT: delete each. WHY: the key and the restore must not outlive the drill.
}

/// Sets, once, what Ctrl+C or a stop signal does during a drill: delete the secrets, then end.
fn guard_on_interrupt() {
    static ONCE: std::sync::Once = std::sync::Once::new(); // WHAT: once per process. WHY: the handler can be set only once.
    ONCE.call_once(|| {
        let _ = ctrlc::set_handler(|| {
            delete_secrets(None);
            std::process::exit(130);
        });
    }); // WHAT: delete, then end as interrupted. WHY: age's passphrase prompt can be stopped with Ctrl+C.
}

/// `--offsite-backup [TARGET]` and `--offsite-drill [TARGET] --identity KEY --scratch DIR [...]`, with the server's settings.
pub fn main_from_args(
    product: &'static Product,
    config: &crate::Config,
    version: &str,
    args: &[String],
) -> i32 {
    let usage = "usage: hyperview-server --offsite-backup [TARGET] [--work DIR] [--data DIR]\n       hyperview-server --offsite-drill [TARGET] --identity KEYFILE --scratch EMPTYDIR [--work DIR] [--data DIR] [--object daily/NAME] [--full] [--sample N] [--keep]"; // WHAT: the two forms. WHY: printed on a mistake.
    let Some(mode) = args
        .first()
        .filter(|m| *m == "--offsite-backup" || *m == "--offsite-drill")
    else {
        eprintln!("{usage}");
        return 4;
    }; // WHAT: which command. WHY: refuse anything else.
    let mut opt: BTreeMap<String, String> = BTreeMap::new();
    let mut flags = BTreeSet::new();
    let mut spec: Option<String> = None;
    let mut i = 1; // WHAT: the options. WHY: read below.
    while i < args.len() {
        let a = &args[i];
        if ["--full", "--keep"].contains(&a.as_str()) {
            flags.insert(a.clone());
            i += 1;
        } else if [
            "--data",
            "--work",
            "--identity",
            "--scratch",
            "--object",
            "--sample",
        ]
        .contains(&a.as_str())
            && i + 1 < args.len()
        {
            opt.insert(a.clone(), args[i + 1].clone());
            i += 2;
        } else if i == 1 && !a.starts_with("--") {
            spec = Some(a.clone());
            i += 1;
        } else {
            eprintln!("unknown or incomplete option {a}\n{usage}");
            return 4;
        }
    } // WHAT: a target first, then --name value pairs and two flags. WHY: a mistyped option must not be ignored.
    let data = opt
        .get("--data")
        .map_or_else(|| config.data.clone(), PathBuf::from); // WHAT: the data folder the server uses, or the one given. WHY: the installed program finds its own drawings.
    let work = opt
        .get("--work")
        .cloned()
        .or_else(|| config.offsite_work.clone()); // WHAT: the work folder given, or the setting. WHY: the same as the night's.
    let given = spec.clone(); // WHAT: the target typed, if any. WHY: decides whether this run marks.
    let target = spec.or_else(|| config.offsite_target.clone()); // WHAT: the target given, or the setting. WHY: a person can try another, such as a USB disk.
    let job = match job_from(product, data, work.as_deref(), target.as_deref(), version) {
        Ok(Some(j)) => j,
        Ok(None) => {
            eprintln!("The off-site backup isn't set up: HYPERVIEW_OFFSITE_TARGET and HYPERVIEW_OFFSITE_WORK are empty (README.md, Off-site backups).");
            return 4;
        }
        Err(e) => {
            eprintln!("{e:#}");
            return 4;
        }
    }; // WHAT: the job, or why not. WHY: say what is missing.
    let mut job = job;
    job.mark = marks(
        config.offsite_target.as_deref(),
        config.offsite_work.as_deref(),
        given.as_deref(),
        opt.get("--work").map(String::as_str),
    ) && opt
        .get("--data")
        .is_none_or(|d| real_of(d) == Some(real(&config.data))); // WHAT: mark only the server's own target, work folder and data, as real places. WHY: a backup by hand to a USB disk must not move backupDays.
    let exit = if mode == "--offsite-backup" {
        job.backup()
    } else {
        // WHAT: the backup now, or the drill. WHY: the two commands.
        let (Some(id), Some(scratch)) = (opt.get("--identity"), opt.get("--scratch")) else {
            eprintln!("{usage}");
            return 4;
        }; // WHAT: the key and the scratch folder. WHY: both required.
        let sample = match opt.get("--sample").map(|s| s.parse::<usize>()) {
            None => 20,
            Some(Ok(n)) => n,
            Some(Err(_)) => {
                eprintln!("--sample takes a number\n{usage}");
                return 4;
            }
        }; // WHAT: how many files a sample drill checks. WHY: 20 by default; a typo is refused.
        Drill {
            job,
            identity: std::path::absolute(id).unwrap_or_else(|_| PathBuf::from(id)),
            scratch: PathBuf::from(scratch),
            object: opt.get("--object").cloned(),
            full: flags.contains("--full"),
            sample,
            keep: flags.contains("--keep"),
        }
        .run() // WHAT: run it. WHY: Creede, with the key on a USB stick.
    };
    if exit.code != 0 && exit.code != 5 && !exit.why.is_empty() {
        eprintln!("{}", exit.why);
    } // WHAT: the reason on failure. WHY: a person running it sees why.
    exit.code
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod cad_part {
    // WHAT: the cad part's drawings ride in View's backup. WHY: one server, one backup; a CAD drawing left out would be lost with the disk.
    #[test]
    fn cad_drawings_are_counted_and_their_files_carried() {
        let db = rusqlite::Connection::open_in_memory().unwrap(); // WHAT: a database like the server's. WHY: the backup reads it
        db.execute_batch(crate::store::SCHEMA_FOR_TESTS).unwrap(); // WHAT: View's tables. WHY: the references query reads them all
        db.execute_batch(crate::cad::SCHEMA).unwrap(); // WHAT: CAD's. WHY: the same database
        db.execute_batch(crate::archetype::SCHEMA).unwrap(); // WHAT: Archetype's. WHY: the same database
        db.execute("INSERT INTO cad_drawings (id, folder, name, created_by, created_at) VALUES (1, '', 'P.excad', 'sam', 1)", []).unwrap();
        db.execute("INSERT INTO cad_versions (drawing, version, sha256, size, saved_by, saved_at) VALUES (1, 1, ?1, 2, 'sam', 1)", [&"c".repeat(64)]).unwrap();
        assert!(super::view_references(&db).unwrap().contains(&"c".repeat(64)), "a CAD drawing's file is one the backup must carry");
        assert!(super::VIEW.counts.contains(&"cad_drawings") && super::VIEW.counts.contains(&"cad_versions"), "and the drill counts them");
        db.execute("INSERT INTO arch_files (digest, bytes, sent) VALUES (?1, 3, 'now')", [&"d".repeat(64)]).unwrap(); // WHAT: a file sent for an Archetype job. WHY: it rides in the backup too
        assert!(super::view_references(&db).unwrap().contains(&"d".repeat(64)), "an Archetype job's file is carried");
        assert!(super::VIEW.counts.contains(&"arch_jobs") && super::VIEW.counts.contains(&"arch_checkins"), "and the drill counts Archetype's jobs");
    }
}
