//! The Office Server writes its maintenance key into the Citadel cell's
//! settings as the system, in a folder the cell itself can change. A link
//! the cell makes there must never send that write anywhere else (the
//! review of 2026-11-12, its attack test `atk_cellkey.rs`, ported here).

#![cfg(unix)]

use std::path::PathBuf;

/// A fresh cell data folder and a folder outside it, standing for anything
/// the system can write.
fn places(what: &str) -> (PathBuf, PathBuf, PathBuf) {
    let n: u64 = rand::random(); // WHAT: a fresh name. WHY: tests run side by side
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cellkey-{what}-{n:016x}")); // WHAT: under the build's own folder. WHY: never a shared temporary folder
    let (data, victim) = (base.join("celldata"), base.join("somewhere-else")); // WHAT: the cell's data, and a folder outside it. WHY: the attack's two places
    std::fs::create_dir_all(&data).unwrap(); // WHAT: made. WHY: the cell's folder
    std::fs::create_dir_all(&victim).unwrap(); // WHAT: made. WHY: what a link would point at
    std::fs::write(victim.join("settings.json"), r#"{"theirs":true}"#).unwrap(); // WHAT: a file that isn't the cell's. WHY: must stay untouched
    (base, data, victim)
}

#[test]
fn a_cell_owned_state_link_never_redirects_the_systems_write() {
    let (base, data, victim) = places("state"); // WHAT: the reviewer's setup. WHY: ported as it was
    std::os::unix::fs::symlink(&victim, data.join("state")).unwrap(); // WHAT: the cell replaces state\ with a link (a junction on Windows). WHY: the attack
    assert!(hyperview_server::cell::give_the_cell_the_office_key(&data, Some("k")).is_err(), "refused, so the cell isn't started"); // WHAT: refused. WHY: the review's medium
    assert_eq!(std::fs::read_to_string(victim.join("settings.json")).unwrap(), r#"{"theirs":true}"#, "nothing outside the cell's folder was written"); // WHAT: untouched. WHY: the guard
    let _ = std::fs::remove_dir_all(&base); // WHAT: tidied. WHY: a test leaves nothing
}

#[test]
fn a_settings_file_that_is_a_link_is_never_read_or_written_through() {
    for name in ["settings.json", "settings.json.previous"] {
        let (base, data, victim) = places("file"); // WHAT: a fresh setup each time. WHY: one link at a time
        std::fs::create_dir_all(data.join("state")).unwrap(); // WHAT: a real state\. WHY: only the file is a link
        std::os::unix::fs::symlink(victim.join("settings.json"), data.join("state").join(name)).unwrap(); // WHAT: the file made a link to somewhere else. WHY: the attack on the file itself
        assert!(hyperview_server::cell::give_the_cell_the_office_key(&data, Some("k")).is_err(), "{name}: refused"); // WHAT: refused. WHY: never through a link
        assert_eq!(std::fs::read_to_string(victim.join("settings.json")).unwrap(), r#"{"theirs":true}"#, "{name}: untouched"); // WHAT: untouched. WHY: the guard
        let _ = std::fs::remove_dir_all(&base); // WHAT: tidied. WHY: a test leaves nothing
    }
}

#[test]
fn a_fresh_cell_gets_its_state_folder_and_the_office_key() {
    let (base, data, _) = places("fresh"); // WHAT: a cell with no state\ yet. WHY: its first start
    hyperview_server::cell::give_the_cell_the_office_key(&data, Some("k")).unwrap(); // WHAT: written. WHY: the ordinary case still works
    assert!(std::fs::read_to_string(data.join("state").join("settings.json")).unwrap().contains("fleetKeySha256"), "the office's key is there"); // WHAT: written. WHY: the cell's admin routes open only to it
    assert!(!data.join("state").join("settings.json.previous").exists(), "no fallback copy is made where there was none"); // WHAT: none. WHY: only an existing one is rewritten
    let _ = std::fs::remove_dir_all(&base); // WHAT: tidied. WHY: a test leaves nothing
}
