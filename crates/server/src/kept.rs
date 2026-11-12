//! The version before the last update, kept for a rollback, and the
//! rollback itself.
//!
//! An update puts the running server aside as `Hyperview-Server.old` and the
//! running cell aside as `citadel.old` (`hub::update::swap_in`,
//! `crate::cellbundle::swap_staged`). Both are kept until the new version has
//! run for a day, or the next update arrives (which replaces them with the
//! version it puts aside). Until then `hyperview-server --roll-back` puts
//! both back, together, and holds the office on that version: the server
//! doesn't update itself again to the version it was rolled back from, only to
//! a newer one.
//!
//! A rollback to a server older than 0.7.0 is refused: once seats have moved
//! to HTTPS and pinned the server, a server without HTTPS would leave every
//! seat unable to connect (docs/OFFICE-SERVER.md).

use std::path::{Path, PathBuf}; // WHAT: file paths. WHY: Windows and Linux both

/// Which version has been running since when: `version|RFC 3339`.
pub const SINCE: &str = "server_running_since"; // WHAT: the setting. WHY: the day the previous version is kept for starts at the new version's first start
/// The version a rollback came back from: never updated to again.
pub const HELD: &str = "server_held_from"; // WHAT: the setting. WHY: the update check would otherwise put the rolled-back version straight back
/// How long the version before is kept once the new one runs.
pub const KEEP_FOR: time::Duration = time::Duration::hours(24); // WHAT: a day. WHY: critical 4
/// The oldest server a rollback may put back.
pub const OLDEST: &str = "0.7.0"; // WHAT: the first with HTTPS. WHY: an older one strands every seat that pinned the server

/// The kept program beside `program`.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn kept_program(program: &Path) -> PathBuf {
    program.with_extension("old") // WHAT: where `swap_in` put the version before. WHY: the same name it uses
}

/// After a start, and from time to time after: the version before is let go
/// once this one has run a day. Whether it was let go now.
pub fn tidy(store: &crate::Store, program: &Path, version: &str, now: time::OffsetDateTime) -> bool {
    let rfc = |t: time::OffsetDateTime| t.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(); // WHAT: a time as text. WHY: kept in the settings
    let since = store.setting(SINCE).ok().flatten().and_then(|text| {
        let (v, at) = text.split_once('|')?; // WHAT: the version and its start. WHY: as written below
        let at = time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339).ok()?; // WHAT: the start, read. WHY: compared with now
        (v == version).then_some(at) // WHAT: only this version's start. WHY: a new version starts its own day
    }); // WHAT: since when this version has run. WHY: the day is counted from its first start
    let Some(since) = since else {
        let _ = store.set_setting(SINCE, &format!("{version}|{}", rfc(now))); // WHAT: this version's first start. WHY: its day starts now
        return false; // WHAT: kept. WHY: the new version hasn't run a day yet
    };
    if now - since < KEEP_FOR {
        return false; // WHAT: kept. WHY: within the day
    }
    let home = program.parent().unwrap_or(Path::new(".")); // WHAT: the service's folder. WHY: the cell's kept folder is there
    let had = kept_program(program).exists() || home.join(crate::cellbundle::KEPT).exists(); // WHAT: whether anything was kept. WHY: said once
    let _ = std::fs::remove_file(kept_program(program)); // WHAT: the server before let go. WHY: this one has run a day
    let _ = std::fs::remove_file(program.with_extension("new")); // WHAT: a leftover of an update. WHY: as before
    let _ = std::fs::remove_dir_all(home.join(crate::cellbundle::KEPT)); // WHAT: the cell before let go. WHY: likewise
    if had {
        tracing::info!("version {version} has run a day, so the version before it is no longer kept for a rollback"); // WHAT: said. WHY: whoever looks after the server knows
    }
    had
}

/// Whether the server should not update itself to `version`: an office
/// rolled back from it is held below it.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn held_back(store: &crate::Store, version: &str) -> bool {
    store.setting(HELD).ok().flatten().is_some_and(|from| !hub::update::newer(version, &from)) // WHAT: the version rolled back from, or an older one. WHY: only a newer one is taken
}

/// Puts the version before back: the server program and the cell, together,
/// with the newer kept beside them in turn. The service must be stopped
/// first. What was done, in words.
///
/// # Errors
///
/// When nothing is kept, the kept server is older than 0.7.0 or isn't older
/// than the running one, or a rename fails (what was renamed is put back).
pub fn roll_back(store: &crate::Store, program: &Path) -> Result<String, String> {
    let kept = kept_program(program); // WHAT: the version before. WHY: put back
    if !std::fs::symlink_metadata(&kept).is_ok_and(|m| m.file_type().is_file()) {
        return Err(format!("there is no earlier version kept beside {} (it is kept for a day after an update)", program.display())); // WHAT: refused. WHY: nothing to put back
    }
    let back_to = hub::update::version_in_file(&kept).ok_or("the kept program doesn't say its version, so it was not put back")?; // WHAT: its version. WHY: checked below
    let now = hub::update::version_in_file(program).unwrap_or_else(|| crate::VERSION.to_string()); // WHAT: the version installed now. WHY: rolled back from
    if hub::update::compare(&back_to, OLDEST) == std::cmp::Ordering::Less {
        return Err(format!("the kept version is {back_to}, older than {OLDEST}; seats that pinned this server over HTTPS couldn't connect to it, so it was not put back")); // WHAT: refused. WHY: docs/OFFICE-SERVER.md: never back below 0.7.0
    }
    if !hub::update::newer(&now, &back_to) {
        return Err(format!("the kept version {back_to} isn't older than {now}; nothing was changed")); // WHAT: refused. WHY: a rollback goes back
    }
    exchange(program, &kept)?; // WHAT: the server put back, the newer kept. WHY: the two swap places
    let home = program.parent().unwrap_or(Path::new(".")); // WHAT: the service's folder. WHY: the cell is there
    let (cell, cell_kept) = (home.join(crate::cellbundle::FOLDER), home.join(crate::cellbundle::KEPT)); // WHAT: the cell and the one before. WHY: put back together
    let cell_too = cell_kept.exists(); // WHAT: whether a cell was kept. WHY: an office without Citadel has none
    if cell_too {
        if let Err(why) = exchange(&cell, &cell_kept) {
            let _ = exchange(program, &kept); // WHAT: the server put forward again. WHY: never a server and a cell of different versions
            return Err(format!("the cell couldn't be put back ({why}); nothing was changed")); // WHAT: said. WHY: both or neither
        }
    }
    let _ = store.set_setting(HELD, &now); // WHAT: held below the version rolled back from. WHY: the update check would put it straight back
    let _ = store.with(|db| Ok(db.execute("DELETE FROM settings WHERE key = ?1", rusqlite::params![SINCE])?)); // WHAT: the day forgotten. WHY: the version put back starts its own
    let said = format!("put version {back_to} back in place of {now}{}; {now} is kept beside it, and the server won't update itself to {now} again, only to a newer version", if cell_too { ", with its Citadel cell" } else { "" }); // WHAT: what was done. WHY: printed and audited
    let _ = store.audit(&crate::audit::Entry::new("rolled the Office Server back", "the server console").saying(said.clone())); // WHAT: written. WHY: every change is in the audit log
    Ok(said)
}

/// Swaps two paths' places by renaming, putting both back if a step fails.
fn exchange(a: &Path, b: &Path) -> Result<(), String> {
    let aside = a.with_extension("rolling"); // WHAT: a name for the moment. WHY: two renames can't swap without one
    let _ = std::fs::remove_file(&aside); // WHAT: a leftover gone. WHY: a rename onto it would fail
    let _ = std::fs::remove_dir_all(&aside); // WHAT: likewise, for a folder. WHY: the cell is a folder
    std::fs::rename(a, &aside).map_err(|e| format!("could not move {} aside ({e})", a.display()))?; // WHAT: the current one aside. WHY: its name is wanted
    if let Err(e) = std::fs::rename(b, a) {
        let _ = std::fs::rename(&aside, a); // WHAT: put back. WHY: never left without it
        return Err(format!("could not put {} back ({e})", b.display())); // WHAT: said. WHY: nothing changed
    }
    if let Err(e) = std::fs::rename(&aside, b) {
        let _ = std::fs::rename(a, b); // WHAT: undone. WHY: as it was
        let _ = std::fs::rename(&aside, a); // WHAT: likewise. WHY: as it was
        return Err(format!("could not keep {} ({e})", a.display())); // WHAT: said. WHY: nothing changed
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_home() -> (crate::Store, PathBuf) {
        let n: u64 = rand::random(); // WHAT: a fresh name. WHY: tests run side by side
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("kept-{n:016x}")); // WHAT: under the build's own folder. WHY: never a shared temporary folder
        std::fs::create_dir_all(&dir).unwrap(); // WHAT: made. WHY: the service's folder
        let config = crate::Config { data: dir.join("data"), ..crate::Config::default() }; // WHAT: its data. WHY: the settings
        (crate::Store::open(&config.database(), &config.blobs()).unwrap(), dir)
    }

    /// A program file saying `version`, as a build's mark does.
    fn program(path: &Path, version: &str) {
        std::fs::write(path, format!("MZ\u{1}hyperview-build-version={version}\u{2}")).unwrap(); // WHAT: a stand-in program with its version mark. WHY: what version_in_file reads
    }

    #[test]
    fn the_version_before_is_kept_a_day_and_then_let_go() {
        let (store, home) = a_home(); // WHAT: a service's folder. WHY: the programs are in it
        let exe = home.join("Hyperview-Server.exe"); // WHAT: the server. WHY: its kept copy is beside it
        program(&exe, "0.7.1"); // WHAT: the new version. WHY: running
        program(&kept_program(&exe), "0.7.0"); // WHAT: the one before. WHY: kept for a rollback
        std::fs::create_dir_all(home.join("citadel.old")).unwrap(); // WHAT: the cell before. WHY: kept with it
        let start = time::OffsetDateTime::now_utc(); // WHAT: the new version's first start. WHY: the day counts from it
        assert!(!tidy(&store, &exe, "0.7.1", start), "kept at the first start"); // WHAT: kept. WHY: critical 4: no longer deleted at every start
        assert!(!tidy(&store, &exe, "0.7.1", start + time::Duration::hours(23)), "kept within the day"); // WHAT: kept. WHY: likewise
        assert!(kept_program(&exe).exists() && home.join("citadel.old").exists(), "both still there"); // WHAT: kept. WHY: a rollback can use them
        assert!(tidy(&store, &exe, "0.7.1", start + time::Duration::hours(25)), "let go after a day"); // WHAT: let go. WHY: the new one has run a day
        assert!(!kept_program(&exe).exists() && !home.join("citadel.old").exists(), "both gone"); // WHAT: gone. WHY: tidied
        let _ = std::fs::remove_dir_all(&home); // WHAT: tidied. WHY: a test leaves nothing
    }

    #[test]
    fn a_rollback_puts_the_server_and_its_cell_back_and_holds_the_office_there() {
        let (store, home) = a_home(); // WHAT: a service's folder. WHY: the programs are in it
        let exe = home.join("Hyperview-Server.exe"); // WHAT: the server. WHY: rolled back
        program(&exe, "0.7.1"); // WHAT: the new version. WHY: rolled back from
        program(&kept_program(&exe), "0.7.0"); // WHAT: the one before. WHY: put back
        std::fs::create_dir_all(home.join("citadel")).unwrap(); // WHAT: the new cell. WHY: rolled back with it
        std::fs::write(home.join("citadel").join("Citadel Server.exe"), b"cell new").unwrap(); // WHAT: its program. WHY: told apart below
        std::fs::create_dir_all(home.join("citadel.old")).unwrap(); // WHAT: the cell before. WHY: put back
        std::fs::write(home.join("citadel.old").join("Citadel Server.exe"), b"cell old").unwrap(); // WHAT: its program. WHY: told apart below
        let said = roll_back(&store, &exe).unwrap(); // WHAT: rolled back. WHY: the documented rollback, for real
        assert!(said.contains("0.7.0") && said.contains("Citadel cell"), "{said}"); // WHAT: said. WHY: plain
        assert_eq!(hub::update::version_in_file(&exe).as_deref(), Some("0.7.0"), "the server before runs"); // WHAT: put back. WHY: the rollback
        assert_eq!(std::fs::read(home.join("citadel").join("Citadel Server.exe")).unwrap(), b"cell old", "with its cell"); // WHAT: put back together. WHY: never a server and cell of different versions
        assert_eq!(hub::update::version_in_file(&kept_program(&exe)).as_deref(), Some("0.7.1"), "the newer kept in turn"); // WHAT: kept. WHY: it can be gone forward to by hand
        assert!(held_back(&store, "0.7.1"), "the update check won't put 0.7.1 straight back"); // WHAT: held. WHY: the office asked to go back
        assert!(!held_back(&store, "0.7.2"), "a newer version is still taken"); // WHAT: not held. WHY: a fix comes as a newer version
        let _ = std::fs::remove_dir_all(&home); // WHAT: tidied. WHY: a test leaves nothing
    }

    #[test]
    fn a_rollback_below_0_7_0_or_to_nothing_is_refused() {
        let (store, home) = a_home(); // WHAT: a service's folder. WHY: the programs are in it
        let exe = home.join("Hyperview-Server.exe"); // WHAT: the server. WHY: asked to roll back
        program(&exe, "0.7.0"); // WHAT: the first HTTPS version. WHY: running
        assert!(roll_back(&store, &exe).is_err(), "nothing kept, nothing done"); // WHAT: refused. WHY: nothing to put back
        program(&kept_program(&exe), "0.6.32"); // WHAT: a server without HTTPS kept. WHY: the dangerous case
        let why = roll_back(&store, &exe).unwrap_err(); // WHAT: refused. WHY: seats pinned over HTTPS
        assert!(why.contains("older than 0.7.0"), "{why}"); // WHAT: said. WHY: plain
        assert_eq!(hub::update::version_in_file(&exe).as_deref(), Some("0.7.0"), "nothing changed"); // WHAT: untouched. WHY: refused before any rename
        let _ = std::fs::remove_dir_all(&home); // WHAT: tidied. WHY: a test leaves nothing
    }
}
