//! The Citadel cell's program, as this build carries it.
//!
//! The cell runs as the system (`--move-from`) and under a restricted token
//! (`--cell`), so a program in its place that didn't come from the signed
//! release would run with the Office Server's own rights. This build is made
//! with `packaging/citadel-cell.json` inside it: the cell's files and the
//! SHA-256 of each. A cell program is run only when its SHA-256 is that one.
//!
//! Where the files come from:
//!
//! - **installing:** the `citadel\` folder beside the server program being
//!   installed (the USB kit carries it), checked file by file against the pin
//!   and copied into `citadel.new`, then put in place;
//! - **updating:** the release's cell part (platform
//!   [`hub::feed::CITADEL_PLATFORM`]), signed with the same key and checked
//!   the same way as the server itself, unpacked into `citadel.new` by the
//!   server doing the update; the new server checks it against its own pin at
//!   its first start, before the cell starts, and puts it in place. The cell
//!   is never running while its folder is replaced.
//!
//! The cell part is a gzipped tar: each file named by its own SHA-256, and
//! `names.json` saying what each is called. Named by fingerprint, so the
//! archive's names are plain and every file is checked as it comes out.
//!
//! The folder put aside by the last swap is kept as `citadel.old`, beside
//! `Hyperview-Server.exe.old`, for a rollback (`crate::kept`).

use std::collections::BTreeMap; // WHAT: sorted maps. WHY: the pin's files, in one order
use std::io::Read; // WHAT: byte streams. WHY: files are hashed as they are read
use std::path::{Path, PathBuf}; // WHAT: file paths. WHY: Windows and Linux both

use serde::Deserialize; // WHAT: JSON in. WHY: the pin and the bundle's names
use sha2::{Digest, Sha256}; // WHAT: SHA-256. WHY: every file's fingerprint

/// The pin this build was made with.
pub const PINNED: &str = include_str!("../../../packaging/citadel-cell.json"); // WHAT: the pin, inside the program. WHY: signed with the server itself, so whoever could change it could already change everything

/// The cell's folder beside the server program.
pub const FOLDER: &str = "citadel"; // WHAT: `citadel\`. WHY: §6a
/// Where a new cell waits to be put in place.
pub const STAGED: &str = "citadel.new"; // WHAT: beside `citadel\`. WHY: swapped in before the cell starts
/// Where the cell before the last swap is kept, for a rollback.
pub const KEPT: &str = "citadel.old"; // WHAT: beside `citadel\`. WHY: put back with the server by `--roll-back`
/// The cell's program.
pub const PROGRAM: &str = "Citadel Server.exe"; // WHAT: the one-file program the cell is. WHY: the name the cell's command line uses
/// The bundle's list of names.
pub const NAMES: &str = "names.json"; // WHAT: fingerprint to file name. WHY: the archive's own names are fingerprints
/// What is said when this build carries no cell.
pub const NOT_CARRIED: &str = "this build of the Office Server carries no Citadel cell (packaging/citadel-cell.json lists no files)"; // WHAT: the reason. WHY: the cell never starts without a pin

/// The start of the error when the cell before can't be put aside, which on
/// Windows means its program is still running; the caller may wait and try
/// again.
pub const BUSY: &str = "the cell before couldn't be put aside, which on Windows means it is still running"; // WHAT: the words. WHY: told apart from a refusal

/// The largest cell file taken.
const LARGEST: u64 = 400 * 1024 * 1024; // WHAT: 400 MB. WHY: the one-file cell is about 100 MB; a planted giant never fills the disk

/// The cell's files and their fingerprints.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    #[serde(default)]
    pub version: String, // WHAT: the cell's version. WHY: said in the log
    #[serde(default)]
    pub files: BTreeMap<String, String>, // WHAT: each file's name in `citadel\` and its SHA-256. WHY: the only files run or kept there
}

impl Pin {
    /// The program's fingerprint.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn program(&self) -> Option<&str> {
        self.files.get(PROGRAM).map(String::as_str) // WHAT: the program's SHA-256. WHY: checked before every run
    }
}

/// The pin this build was made with.
///
/// # Errors
///
/// When it lists no files, or one it lists isn't plain.
pub fn pinned() -> Result<Pin, String> {
    read_pin(PINNED) // WHAT: read. WHY: the one this build carries
}

/// Reads a pin, strictly.
///
/// # Errors
///
/// When it doesn't read, lists no files, names one that isn't plain, has a
/// fingerprint that isn't one, or doesn't list the program.
pub fn read_pin(text: &str) -> Result<Pin, String> {
    let pin: Pin = serde_json::from_str(text).map_err(|e| format!("the cell's pin doesn't read: {e}"))?; // WHAT: the JSON. WHY: a pin that doesn't read carries nothing
    if pin.files.is_empty() {
        return Err(NOT_CARRIED.to_string()); // WHAT: nothing pinned. WHY: no cell without one
    }
    if let Some((name, _)) = pin.files.iter().find(|(name, sha)| !plain_name(name) || !crate::store::is_digest(sha)) {
        return Err(format!("the cell's pin lists {name:?}, which isn't a plain file name with a SHA-256")); // WHAT: refused. WHY: a name could otherwise reach outside the folder
    }
    if pin.program().is_none() {
        return Err(format!("the cell's pin doesn't list {PROGRAM}")); // WHAT: refused. WHY: the program is what is run
    }
    Ok(pin)
}

/// Whether `name` is a plain file name: no folder, no drive, no `..`.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 100 && name != "." && name != ".." && name.chars().all(|c| c.is_ascii_alphanumeric() || " -_.()".contains(c)) // WHAT: letters, digits, spaces and a few marks. WHY: never a separator, so never outside `citadel\`
}

/// The SHA-256 of a file, as lower-case hex.
///
/// # Errors
///
/// When it can't be read.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?; // WHAT: opened. WHY: read in pieces
    let mut hash = Sha256::new(); // WHAT: the fingerprint so far. WHY: never the whole file in memory
    let mut piece = vec![0u8; 1 << 16]; // WHAT: 64 KiB at a time. WHY: a 100 MB program read in pieces
    loop {
        let n = file.read(&mut piece)?; // WHAT: the next piece. WHY: hashed
        if n == 0 {
            break; // WHAT: the end. WHY: done
        }
        hash.update(&piece[..n]); // WHAT: hashed. WHY: the fingerprint
    }
    Ok(hub::update::hex(&hash.finalize())) // WHAT: lower-case hex. WHY: as the pin writes it
}

/// Whether `path` is a regular file, never a link.
fn regular(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file()) // WHAT: a plain file. WHY: a link could point anywhere
}

/// Checks the cell program about to be run against the pin.
///
/// # Errors
///
/// When it isn't there, isn't a plain file, or isn't the pinned one.
pub fn check_program(program: &Path, pin: &Pin) -> Result<(), String> {
    let want = pin.program().ok_or_else(|| format!("the cell's pin doesn't list {PROGRAM}"))?; // WHAT: the pinned fingerprint. WHY: compared below
    if !regular(program) {
        return Err(format!("the cell program {} isn't there as a plain file; it was not run", program.display())); // WHAT: refused. WHY: never a link or a folder
    }
    let found = sha256_file(program).map_err(|e| format!("the cell program {} couldn't be read ({e}); it was not run", program.display()))?; // WHAT: its fingerprint. WHY: compared with the pin
    if found != want {
        return Err(format!("the cell program {} isn't the one this Office Server was built with (SHA-256 {found}, not {want}); it was not run", program.display())); // WHAT: refused. WHY: risk 8: only the signed bundle's program runs as the system or the cell
    }
    Ok(())
}

/// Checks a cell folder against the pin: every pinned file there and right,
/// and nothing else in it.
///
/// # Errors
///
/// What is missing, wrong or extra.
pub fn check_folder(folder: &Path, pin: &Pin) -> Result<(), String> {
    for (name, want) in &pin.files {
        let path = folder.join(name); // WHAT: the file. WHY: checked
        if !regular(&path) {
            return Err(format!("{} is missing from {}", name, folder.display())); // WHAT: refused. WHY: a cell missing a file isn't the pinned one
        }
        let found = sha256_file(&path).map_err(|e| format!("{name} couldn't be read ({e})"))?; // WHAT: its fingerprint. WHY: compared
        if found != *want {
            return Err(format!("{name} in {} isn't the pinned one (SHA-256 {found}, not {want})", folder.display())); // WHAT: refused. WHY: only the signed bundle's files
        }
    }
    for entry in std::fs::read_dir(folder).map_err(|e| format!("{} couldn't be read ({e})", folder.display()))? {
        let name = entry.map_err(|e| e.to_string())?.file_name(); // WHAT: each thing in the folder. WHY: nothing else may be there
        if !pin.files.contains_key(name.to_string_lossy().as_ref()) {
            return Err(format!("{} holds \"{}\", which the pin doesn't list", folder.display(), name.to_string_lossy())); // WHAT: refused. WHY: a DLL put beside the program would load with it
        }
    }
    Ok(())
}

/// Installs the cell from a folder (the USB kit's `citadel\`, beside the
/// server program being installed) into `home`: checked, copied into
/// `citadel.new`, checked again, and put in place.
///
/// # Errors
///
/// When the folder isn't the pinned cell, or a copy or a rename fails.
pub fn place_from(source: &Path, home: &Path, pin: &Pin) -> Result<(), String> {
    check_folder(source, pin).map_err(|e| format!("the cell beside this program isn't the one it was built with: {e}"))?; // WHAT: the source, checked. WHY: never copy in what won't be run
    let staged = home.join(STAGED); // WHAT: where it waits. WHY: swapped in whole
    let _ = std::fs::remove_dir_all(&staged); // WHAT: a half copy from before gone. WHY: made fresh
    std::fs::create_dir_all(&staged).map_err(|e| format!("could not make {} ({e})", staged.display()))?; // WHAT: made. WHY: the copies go in it
    for name in pin.files.keys() {
        std::fs::copy(source.join(name), staged.join(name)).map_err(|e| format!("could not copy {name} ({e})"))?; // WHAT: each pinned file. WHY: nothing else
    }
    swap_staged(home, pin).map(|_| ()) // WHAT: checked and put in place. WHY: the same step as an update
}

/// Puts a waiting `citadel.new` in place, before the cell starts: checked
/// against this build's pin first; the cell it replaces kept as
/// `citadel.old`. Whether one was put in place.
///
/// # Errors
///
/// When the waiting cell isn't the pinned one (it is then removed), or a
/// rename fails (the cell before is then put back).
pub fn swap_staged(home: &Path, pin: &Pin) -> Result<bool, String> {
    let staged = home.join(STAGED); // WHAT: the waiting cell. WHY: put in place
    if std::fs::symlink_metadata(&staged).is_err() {
        return Ok(false); // WHAT: nothing waiting. WHY: the ordinary start
    }
    if let Err(why) = check_folder(&staged, pin) {
        let _ = std::fs::remove_dir_all(&staged); // WHAT: taken away. WHY: never run, and not tried again
        return Err(format!("the cell waiting to be put in place isn't the one this Office Server was built with, so it was removed: {why}")); // WHAT: refused. WHY: only the pinned cell
    }
    let (current, kept) = (home.join(FOLDER), home.join(KEPT)); // WHAT: the cell now, and where it is kept. WHY: for a rollback
    let had = std::fs::symlink_metadata(&current).is_ok(); // WHAT: whether there is a cell now. WHY: the first install has none
    if had {
        let _ = std::fs::remove_dir_all(&kept); // WHAT: the one kept before gone. WHY: one previous version is kept, the one before this
        std::fs::rename(&current, &kept).map_err(|e| format!("{BUSY} ({e}); nothing was changed, and the new cell waits for the next start"))?; // WHAT: the cell now, kept; nothing changed if it can't be. WHY: a rollback puts it back; review low 1: never half swapped
    }
    if let Err(e) = std::fs::rename(&staged, &current) {
        if had {
            let _ = std::fs::rename(&kept, &current); // WHAT: the cell before put back. WHY: never left without a cell
        }
        return Err(format!("could not put the new cell in place ({e})")); // WHAT: said. WHY: the cell before still runs
    }
    Ok(true)
}

/// Unpacks a signed cell part (already checked against the release
/// signature) into `citadel.new` beside the server, for the next start to
/// check and put in place. The server doing this needn't know the new pin:
/// every file is checked against the fingerprint it is named by, and the new
/// server checks the whole folder against its own pin.
///
/// # Errors
///
/// When the archive isn't a cell part this program writes.
pub fn stage_bundle(gzipped: &[u8], home: &Path) -> Result<(), String> {
    let part = home.join(format!("{STAGED}.part")); // WHAT: where it is unpacked. WHY: only a whole cell is ever `citadel.new`
    let _ = std::fs::remove_dir_all(&part); // WHAT: a half one from before gone. WHY: made fresh
    std::fs::create_dir_all(&part).map_err(|e| format!("could not make {} ({e})", part.display()))?; // WHAT: made. WHY: unpacked into
    let unpacked = unpack(gzipped, &part, home); // WHAT: unpacked and named. WHY: below
    let _ = std::fs::remove_dir_all(&part); // WHAT: the unpacking folder gone, whatever happened. WHY: never a stray half cell
    if unpacked.is_err() {
        let _ = std::fs::remove_dir_all(home.join(STAGED)); // WHAT: a half-made waiting cell gone. WHY: only a whole cell ever waits
    }
    unpacked
}

/// The work of [`stage_bundle`].
fn unpack(gzipped: &[u8], part: &Path, home: &Path) -> Result<(), String> {
    let mut gz = flate2::read::GzDecoder::new(gzipped); // WHAT: gunzipped as it is read. WHY: the part is a .tar.gz
    let names = crate::offsite::untar_with(&mut gz, part, LARGEST * 4, &|name| {
        if name == NAMES {
            Some(64 * 1024) // WHAT: the names list, small. WHY: a planted giant is refused
        } else if crate::store::is_digest(name) {
            Some(LARGEST) // WHAT: a file named by its fingerprint. WHY: the cell's files
        } else {
            None // WHAT: anything else refused. WHY: only what this program writes
        }
    })
    .map_err(|e| format!("the cell part doesn't unpack: {e:#}"))?; // WHAT: unpacked into the part folder. WHY: never outside it
    let listed: BTreeMap<String, String> = serde_json::from_slice(&std::fs::read(part.join(NAMES)).map_err(|e| format!("the cell part has no {NAMES} ({e})"))?).map_err(|e| format!("the cell part's {NAMES} doesn't read ({e})"))?; // WHAT: fingerprint to name. WHY: what each file is called
    let files: Vec<&String> = names.iter().filter(|n| n.as_str() != NAMES).collect(); // WHAT: the files unpacked. WHY: each must be listed
    if files.len() != listed.len() || files.iter().any(|f| !listed.contains_key(f.as_str())) {
        return Err("the cell part's files and its list of names don't agree".to_string()); // WHAT: refused. WHY: nothing unnamed, nothing missing
    }
    let staged = home.join(STAGED); // WHAT: where the whole cell waits. WHY: checked and swapped in at the next start
    let _ = std::fs::remove_dir_all(&staged); // WHAT: an older waiting cell gone. WHY: the newest is the one put in place
    std::fs::create_dir_all(&staged).map_err(|e| format!("could not make {} ({e})", staged.display()))?; // WHAT: made. WHY: the files go in it
    for (sha, name) in &listed {
        let from = part.join(sha); // WHAT: the file, by its fingerprint. WHY: checked against it
        if !plain_name(name) || sha256_file(&from).map_err(|e| e.to_string())? != *sha {
            let _ = std::fs::remove_dir_all(&staged); // WHAT: the half cell gone. WHY: never left to be checked later
            return Err(format!("{name:?} in the cell part isn't plain, or isn't the file its fingerprint names")); // WHAT: refused. WHY: every file checked as it comes out
        }
        std::fs::rename(&from, staged.join(name)).map_err(|e| format!("could not place {name} ({e})"))?; // WHAT: under its own name. WHY: as the cell expects
    }
    Ok(())
}

/// Packs cell files into a cell part, as the publishing script does: each
/// file named by its SHA-256, and `names.json`. For the tests here and the
/// update tests; the release's own is made by `tools/citadel_cell.py`.
///
/// # Errors
///
/// When a name isn't plain, or the archive can't be written.
pub fn pack(files: &[(&str, &[u8])]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new(); // WHAT: the gzipped tar. WHY: returned
    {
        let mut gz = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast()); // WHAT: gzipped. WHY: as published
        let mut tar = crate::offsite::TarOut { w: &mut gz }; // WHAT: the tar writer. WHY: the same plain tar the reader takes
        let names: BTreeMap<String, String> = files.iter().map(|(n, b)| (hub::update::hex(&Sha256::digest(b)), (*n).to_string())).collect(); // WHAT: fingerprint to name. WHY: names.json
        let list = serde_json::to_vec(&names).map_err(|e| e.to_string())?; // WHAT: as JSON. WHY: its entry
        tar.file(NAMES, list.len() as u64, &mut list.as_slice()).map_err(|e| e.to_string())?; // WHAT: the list. WHY: first
        for (_, body) in files {
            tar.file(&hub::update::hex(&Sha256::digest(body)), body.len() as u64, &mut &body[..]).map_err(|e| e.to_string())?; // WHAT: each file, by its fingerprint. WHY: the archive's plain names
        }
        tar.end().map_err(|e| e.to_string())?; // WHAT: the end. WHY: a whole archive
        gz.finish().map_err(|e| e.to_string())?; // WHAT: flushed. WHY: a whole gzip
    }
    Ok(out)
}

/// The cell's program in `home`.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn program_in(home: &Path) -> PathBuf {
    home.join(FOLDER).join(PROGRAM) // WHAT: `citadel\Citadel Server.exe`. WHY: §6a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(what: &str) -> PathBuf {
        let n: u64 = rand::random(); // WHAT: a fresh name. WHY: tests run side by side
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("cellbundle-{what}-{n:016x}")); // WHAT: under the build's own folder. WHY: never a shared temporary folder
        std::fs::create_dir_all(&dir).unwrap(); // WHAT: made. WHY: the test's files go in it
        dir
    }

    /// A cell of one program, `body`, and its pin.
    fn a_cell(dir: &Path, body: &[u8]) -> (PathBuf, Pin) {
        let source = dir.join("kit-citadel"); // WHAT: the USB kit's `citadel\`. WHY: installed from
        std::fs::create_dir_all(&source).unwrap(); // WHAT: made. WHY: the program goes in it
        std::fs::write(source.join(PROGRAM), body).unwrap(); // WHAT: the program. WHY: pinned below
        let pin = read_pin(&format!("{{\"version\":\"1.1.0\",\"files\":{{\"{PROGRAM}\":\"{}\"}}}}", hub::update::hex(&Sha256::digest(body)))).unwrap(); // WHAT: its pin. WHY: as packaging/citadel-cell.json writes it
        (source, pin)
    }

    /// A cell part as the publishing script makes it.
    fn bundle(files: &[(&str, &[u8])], extra: Option<(&str, &[u8])>) -> Vec<u8> {
        let mut out = Vec::new(); // WHAT: the gzipped tar. WHY: returned
        {
            let mut gz = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast()); // WHAT: gzipped. WHY: as published
            let mut tar = crate::offsite::TarOut { w: &mut gz }; // WHAT: the tar writer. WHY: the same plain tar
            let names: BTreeMap<String, String> = files.iter().map(|(n, b)| (hub::update::hex(&Sha256::digest(b)), (*n).to_string())).collect(); // WHAT: fingerprint to name. WHY: names.json
            let list = serde_json::to_vec(&names).unwrap(); // WHAT: as JSON. WHY: its entry
            tar.file(NAMES, list.len() as u64, &mut list.as_slice()).unwrap(); // WHAT: the list. WHY: first
            for (_, body) in files {
                tar.file(&hub::update::hex(&Sha256::digest(body)), body.len() as u64, &mut &body[..]).unwrap(); // WHAT: each file, by its fingerprint. WHY: the archive's plain names
            }
            if let Some((name, body)) = extra {
                tar.file(name, body.len() as u64, &mut &body[..]).unwrap(); // WHAT: something planted. WHY: the refusal tests
            }
            tar.end().unwrap(); // WHAT: the end. WHY: a whole archive
            gz.finish().unwrap(); // WHAT: flushed. WHY: a whole gzip
        }
        out
    }

    #[test]
    fn this_build_carries_no_cell_until_one_is_pinned() {
        assert_eq!(read_pin(r#"{"version":"","files":{}}"#), Err(NOT_CARRIED.to_string()), "an empty pin carries nothing"); // WHAT: no cell. WHY: never a cell nobody pinned
        assert!(read_pin(r#"{"files":{"..\\x.exe":"00"}}"#).is_err(), "a name that climbs out is refused"); // WHAT: refused. WHY: plain names only
        assert!(read_pin(&format!(r#"{{"files":{{"other.dll":"{}"}}}}"#, "a".repeat(64))).is_err(), "the program must be listed"); // WHAT: refused. WHY: the program is what runs
    }

    #[test]
    fn only_the_pinned_program_is_run() {
        let dir = folder("program"); // WHAT: a fresh folder. WHY: its own files
        let (source, pin) = a_cell(&dir, b"the real cell"); // WHAT: a pinned cell. WHY: checked
        assert!(check_program(&source.join(PROGRAM), &pin).is_ok(), "the pinned one runs"); // WHAT: passes. WHY: its fingerprint
        std::fs::write(source.join(PROGRAM), b"someone else's program").unwrap(); // WHAT: swapped. WHY: risk 8
        let why = check_program(&source.join(PROGRAM), &pin).unwrap_err(); // WHAT: refused. WHY: not the pinned one
        assert!(why.contains("was not run"), "{why}"); // WHAT: said. WHY: plain
        assert!(check_program(&dir.join("nothing.exe"), &pin).is_err(), "a missing program is refused"); // WHAT: refused. WHY: nothing to run
        let _ = std::fs::remove_dir_all(&dir); // WHAT: tidied. WHY: a test leaves nothing
    }

    #[test]
    fn install_puts_the_kit_s_cell_in_place_and_refuses_a_planted_file() {
        let dir = folder("install"); // WHAT: a fresh folder. WHY: its own files
        let home = dir.join("home"); // WHAT: the service's folder. WHY: the cell goes in it
        let (source, pin) = a_cell(&dir, b"cell 1.1.0"); // WHAT: the kit's cell. WHY: installed
        place_from(&source, &home, &pin).unwrap(); // WHAT: installed. WHY: the first install
        assert!(check_folder(&home.join(FOLDER), &pin).is_ok(), "in place and checked"); // WHAT: there. WHY: the cell can start
        std::fs::write(source.join("version.dll"), b"planted").unwrap(); // WHAT: a DLL beside the kit's program. WHY: it would load with it
        assert!(place_from(&source, &home, &pin).is_err(), "a kit with a file the pin doesn't list is refused"); // WHAT: refused. WHY: nothing unpinned is copied
        let _ = std::fs::remove_dir_all(&dir); // WHAT: tidied. WHY: a test leaves nothing
    }

    #[test]
    fn an_update_s_cell_is_unpacked_then_checked_and_swapped_in_keeping_the_one_before() {
        let dir = folder("update"); // WHAT: a fresh folder. WHY: its own files
        let home = dir.join("home"); // WHAT: the service's folder. WHY: the cell is in it
        let (source, old_pin) = a_cell(&dir, b"cell 1.1.0"); // WHAT: the cell installed now. WHY: replaced by the update
        place_from(&source, &home, &old_pin).unwrap(); // WHAT: installed. WHY: before the update
        let new_body: &[u8] = b"cell 1.2.0"; // WHAT: the next cell. WHY: the update's
        let new_pin = read_pin(&format!("{{\"files\":{{\"{PROGRAM}\":\"{}\"}}}}", hub::update::hex(&Sha256::digest(new_body)))).unwrap(); // WHAT: the new server's pin. WHY: it checks at its first start
        stage_bundle(&bundle(&[(PROGRAM, new_body)], None), &home).unwrap(); // WHAT: unpacked by the old server. WHY: the update
        assert!(check_folder(&home.join(FOLDER), &old_pin).is_ok(), "the running cell is untouched until the next start"); // WHAT: unchanged. WHY: never replaced while it runs
        assert!(swap_staged(&home, &new_pin).unwrap(), "put in place at the next start"); // WHAT: swapped. WHY: the new server's first start
        assert!(check_folder(&home.join(FOLDER), &new_pin).is_ok(), "the new cell"); // WHAT: in place. WHY: the update took
        assert!(check_folder(&home.join(KEPT), &old_pin).is_ok(), "the one before kept for a rollback"); // WHAT: kept. WHY: critical 4
        assert!(!swap_staged(&home, &new_pin).unwrap(), "nothing more waiting"); // WHAT: once. WHY: the ordinary start
        let _ = std::fs::remove_dir_all(&dir); // WHAT: tidied. WHY: a test leaves nothing
    }

    #[test]
    fn a_waiting_cell_that_isn_t_this_build_s_is_removed_never_run() {
        let dir = folder("wrong"); // WHAT: a fresh folder. WHY: its own files
        let home = dir.join("home"); // WHAT: the service's folder. WHY: the cell is in it
        let (source, pin) = a_cell(&dir, b"cell 1.1.0"); // WHAT: the cell installed. WHY: kept running
        place_from(&source, &home, &pin).unwrap(); // WHAT: installed. WHY: before
        stage_bundle(&bundle(&[(PROGRAM, b"not the pinned cell")], None), &home).unwrap(); // WHAT: a part signed but not this build's. WHY: the new server's pin is the authority
        assert!(swap_staged(&home, &pin).is_err(), "refused"); // WHAT: refused. WHY: only the pinned cell
        assert!(!home.join(STAGED).exists(), "and removed"); // WHAT: gone. WHY: never tried again
        assert!(check_folder(&home.join(FOLDER), &pin).is_ok(), "the running cell is kept"); // WHAT: untouched. WHY: never left without a cell
        let _ = std::fs::remove_dir_all(&dir); // WHAT: tidied. WHY: a test leaves nothing
    }

    #[test]
    fn a_cell_part_with_a_file_its_names_don_t_vouch_for_is_refused() {
        let dir = folder("planted"); // WHAT: a fresh folder. WHY: its own files
        let home = dir.join("home"); // WHAT: the service's folder. WHY: unpacked beside
        let body: &[u8] = b"cell"; // WHAT: a cell. WHY: listed
        let planted: &[u8] = b"planted"; // WHAT: an unlisted file. WHY: the refusal
        assert!(stage_bundle(&bundle(&[(PROGRAM, body)], Some((&hub::update::hex(&Sha256::digest(planted)), planted))), &home).is_err(), "an unlisted file is refused"); // WHAT: refused. WHY: nothing unnamed
        assert!(stage_bundle(&bundle(&[(PROGRAM, body)], Some(("evil.dll", planted))), &home).is_err(), "a name that isn't a fingerprint is refused"); // WHAT: refused. WHY: only what this program writes
        assert!(stage_bundle(&bundle(&[("..\\up.exe", body)], None), &home).is_err(), "a name that climbs out is refused"); // WHAT: refused. WHY: plain names only
        assert!(!home.join(STAGED).exists(), "nothing waits after a refusal"); // WHAT: nothing staged. WHY: only a whole cell
        let _ = std::fs::remove_dir_all(&dir); // WHAT: tidied. WHY: a test leaves nothing
    }

    #[test]
    fn a_cell_part_packed_by_the_publishing_tool_unpacks_here() {
        let dir = folder("python"); // WHAT: a fresh folder. WHY: its own files
        let home = dir.join("home"); // WHAT: the service's folder. WHY: unpacked beside
        stage_bundle(include_bytes!("cellbundle/made-by-citadel_cell.py.tar.gz"), &home).unwrap(); // WHAT: a part made by tools/citadel_cell.py's pack. WHY: the publishing side and this reader must agree on the format
        assert_eq!(std::fs::read(home.join(STAGED).join(PROGRAM)).unwrap(), b"a cell packed by tools/citadel_cell.py", "the program comes out under its name"); // WHAT: unpacked. WHY: as published
        let _ = std::fs::remove_dir_all(&dir); // WHAT: tidied. WHY: a test leaves nothing
    }
}
