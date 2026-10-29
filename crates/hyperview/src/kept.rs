//! What a seat keeps of the office's drawings, and letting go of what its
//! person may no longer see.
//!
//! A drawing opened from the office's server is kept in the cache folder, so
//! the next morning it opens without coming down the wire again, and so it
//! can be reopened from the recent files. That copy outlives the reason it was
//! allowed: when an administrator takes somebody off a job, out of a group, or
//! gives a folder to the office only, the copy on that person's computer or
//! tablet would still open. So each copy is written down against the set it
//! came from, and every so often the seat asks the server about each one; a
//! set the server now answers "not found" for is taken off the computer.
//!
//! Nothing is taken off when the server can't be asked: a seat in a truck
//! with no signal keeps its drawings. And only "not found" counts: a refusal
//! because the sign-in ran out says nothing about the drawing.

use std::collections::BTreeMap; // WHAT: set ids to file names, in order. WHY: the index, written the same way every time.
use std::path::{Path, PathBuf}; // WHAT: paths. WHY: the cache folder and its files.

/// The index's file, in the cache folder.
pub const INDEX: &str = "kept.json"; // WHAT: which file holds the index. WHY: beside the copies it describes.

/// What the server said about one kept set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    Yes,      // WHAT: the person may still see it. WHY: the copy stays.
    No,       // WHAT: the server answered "not found". WHY: the copy goes.
    CantTell, // WHAT: no answer, or another refusal. WHY: the copy stays, and nothing more is asked this time.
}

fn read_index(folder: &Path) -> BTreeMap<String, String> {
    std::fs::read(folder.join(INDEX)) // WHAT: the index's bytes. WHY: read back.
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok()) // WHAT: as JSON. WHY: written by `write_index`.
        .unwrap_or_default() // WHAT: none, or unreadable, is empty. WHY: the copies it would have named are then treated as unknown below.
}

fn write_index(folder: &Path, index: &BTreeMap<String, String>) {
    let Ok(text) = serde_json::to_vec_pretty(index) else {
        return;
    }; // WHAT: as JSON. WHY: a person can read it.
    let temp = folder.join(format!("{INDEX}.part")); // WHAT: written beside. WHY: a half-written index is never read.
    if std::fs::write(&temp, text).is_ok() {
        let _ = std::fs::rename(&temp, folder.join(INDEX)); // WHAT: moved into place. WHY: all or nothing.
    }
}

/// Writes down that `file`, in `folder`, is the copy of drawing set `set`.
pub fn remember(folder: &Path, set: &str, file: &Path) {
    let Some(name) = file.file_name().and_then(|n| n.to_str()) else {
        return;
    }; // WHAT: the copy's name in the folder. WHY: the index names files, not paths.
    let mut index = read_index(folder); // WHAT: the index as it is. WHY: added to.
    index.insert(set.to_string(), name.to_string()); // WHAT: this set's copy. WHY: asked about later.
    write_index(folder, &index); // WHAT: kept. WHY: survives the program closing.
}

/// Takes off the computer every kept copy the server says this person can no
/// longer see, and any copy kept before copies were written down (it is
/// downloaded again when it is next opened, if it is still theirs).
/// `signed_in` is whether the seat has just reached the server as its person;
/// copies nobody wrote down go only then. Asks `asked` about each kept set;
/// stops asking at the first it can't tell, and then takes nothing off that
/// it doesn't know about. Answers the copies taken off.
pub fn forget_unseen(
    folder: &Path,
    signed_in: bool,
    mut asked: impl FnMut(&str) -> Seen,
) -> Vec<PathBuf> {
    let mut index = read_index(folder); // WHAT: what is kept, and of which set. WHY: each is asked about.
    let mut gone: Vec<PathBuf> = Vec::new(); // WHAT: the copies taken off. WHY: said to the person.
    let mut keep: BTreeMap<String, String> = BTreeMap::new(); // WHAT: the index after. WHY: written back.
    let mut stopped = false; // WHAT: whether a question couldn't be answered. WHY: then no more are asked.
    for (set, name) in index.iter() {
        let answer = if stopped { Seen::CantTell } else { asked(set) }; // WHAT: whether this person may still see the set. WHY: the server decides.
        match answer {
            Seen::No => {
                let path = folder.join(name); // WHAT: the copy. WHY: taken off.
                let _ = std::fs::remove_file(path.with_extension("part")); // WHAT: any half-download of it. WHY: nothing of it stays.
                if std::fs::remove_file(&path).is_ok() || !path.exists() {
                    gone.push(path); // WHAT: taken off. WHY: said.
                } else {
                    keep.insert(set.clone(), name.clone()); // WHAT: couldn't be removed (open on Windows, say). WHY: tried again next time.
                }
            }
            Seen::Yes => {
                keep.insert(set.clone(), name.clone()); // WHAT: still theirs. WHY: kept.
            }
            Seen::CantTell => {
                stopped = true; // WHAT: no more questions this time. WHY: a seat with no signal isn't asked a hundred times.
                keep.insert(set.clone(), name.clone()); // WHAT: kept. WHY: never taken off on a guess.
            }
        }
    }
    if signed_in && !stopped {
        // WHAT: the seat reached the server and every question was answered. WHY: only then is a copy nobody wrote down taken off; it may be a drawing this person can no longer see.
        let known: Vec<&String> = keep.values().collect(); // WHAT: the copies written down. WHY: kept.
        if let Ok(entries) = std::fs::read_dir(folder) {
            for entry in entries.flatten() {
                let path = entry.path(); // WHAT: one file in the cache. WHY: checked.
                let Some(name) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
                else {
                    continue;
                }; // WHAT: its name. WHY: compared with the index.
                let is_copy = name.to_ascii_lowercase().ends_with(".pdf"); // WHAT: a kept drawing. WHY: the index and anything else are left.
                if is_copy
                    && !known.iter().any(|k| **k == name)
                    && std::fs::remove_file(&path).is_ok()
                {
                    gone.push(path); // WHAT: an unknown copy taken off. WHY: as above.
                }
            }
        }
    }
    index = keep; // WHAT: the index after. WHY: written back.
    write_index(folder, &index); // WHAT: kept. WHY: the next look starts from it.
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_folder() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0); // WHAT: a counter. WHY: each test in this run gets its own folder.
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed); // WHAT: this test's number. WHY: as above.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0); // WHAT: the time now. WHY: another run at the same time never shares a folder.
        let scratch = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")); // WHAT: the test run's own scratch folder, or the build folder. WHY: somewhere private, as the printing tests use.
        let path = scratch.join(format!("hyperview-kept-{}-{stamp}-{n}", std::process::id())); // WHAT: the folder. WHY: a cache of its own.
        std::fs::create_dir_all(&path).unwrap(); // WHAT: made. WHY: files go in it.
        path
    }

    #[test]
    fn a_copy_the_server_no_longer_shows_this_person_is_taken_off_and_the_rest_stay() {
        let folder = a_folder(); // WHAT: a cache. WHY: under test.
        for (set, name) in [
            ("set_field", "aaaaaaaaaaaa-S-101.pdf"),
            ("set_office", "bbbbbbbbbbbb-Contract.pdf"),
        ] {
            std::fs::write(folder.join(name), b"%PDF").unwrap(); // WHAT: a kept copy. WHY: one of each.
            remember(&folder, set, &folder.join(name)); // WHAT: written down. WHY: as `fetch` does.
        }
        let gone = forget_unseen(&folder, true, |set| {
            if set == "set_office" {
                Seen::No
            } else {
                Seen::Yes
            }
        }); // WHAT: the contract's folder given to the office only. WHY: the server says not found.
        assert_eq!(gone, vec![folder.join("bbbbbbbbbbbb-Contract.pdf")]); // WHAT: the contract taken off. WHY: no longer theirs.
        assert!(folder.join("aaaaaaaaaaaa-S-101.pdf").exists()); // WHAT: the shop drawing stays. WHY: still theirs.
        assert!(!folder.join("bbbbbbbbbbbb-Contract.pdf").exists()); // WHAT: gone from the disk. WHY: it can't be reopened from the recent files.
        assert!(!read_index(&folder).contains_key("set_office")); // WHAT: off the index. WHY: never asked about again.
        let _ = std::fs::remove_dir_all(&folder); // WHAT: cleaned up. WHY: tests leave nothing.
    }

    #[test]
    fn with_no_answer_from_the_server_nothing_is_taken_off() {
        let folder = a_folder(); // WHAT: a cache. WHY: under test.
        std::fs::write(folder.join("cccccccccccc-A.pdf"), b"%PDF").unwrap(); // WHAT: a kept copy. WHY: written down.
        remember(&folder, "set_a", &folder.join("cccccccccccc-A.pdf")); // WHAT: written down. WHY: asked about.
        std::fs::write(folder.join("dddddddddddd-Old.pdf"), b"%PDF").unwrap(); // WHAT: a copy from before copies were written down. WHY: unknown.
        let mut asked = 0; // WHAT: how many questions. WHY: stops at the first unanswered.
        let gone = forget_unseen(&folder, true, |_| {
            asked += 1; // WHAT: counted. WHY: checked.
            Seen::CantTell // WHAT: no signal. WHY: a seat in a truck.
        });
        assert!(gone.is_empty()); // WHAT: nothing taken off. WHY: never on a guess.
        assert!(
            folder.join("dddddddddddd-Old.pdf").exists()
                && folder.join("cccccccccccc-A.pdf").exists()
        ); // WHAT: both kept. WHY: likewise.
        assert_eq!(asked, 1); // WHAT: one question. WHY: no more once it couldn't be answered.
        let _ = std::fs::remove_dir_all(&folder); // WHAT: cleaned up. WHY: tests leave nothing.
    }

    #[test]
    fn a_copy_nobody_wrote_down_goes_once_the_server_has_answered() {
        let folder = a_folder(); // WHAT: a cache. WHY: under test.
        std::fs::write(folder.join("eeeeeeeeeeee-B.pdf"), b"%PDF").unwrap(); // WHAT: a written-down copy. WHY: still theirs.
        remember(&folder, "set_b", &folder.join("eeeeeeeeeeee-B.pdf")); // WHAT: written down. WHY: asked about.
        std::fs::write(folder.join("ffffffffffff-Old.pdf"), b"%PDF").unwrap(); // WHAT: an unknown copy. WHY: from before 0.6.25.
        let kept = forget_unseen(&folder, false, |_| Seen::Yes); // WHAT: not signed in this time. WHY: an unknown copy stays until the seat reaches the server as its person.
        assert!(kept.is_empty() && folder.join("ffffffffffff-Old.pdf").exists()); // WHAT: nothing taken off. WHY: as just said.
        let gone = forget_unseen(&folder, true, |_| Seen::Yes); // WHAT: the server answers. WHY: so unknown copies go.
        assert_eq!(gone, vec![folder.join("ffffffffffff-Old.pdf")]); // WHAT: the unknown one taken off. WHY: it may be a drawing this person can no longer see.
        assert!(folder.join("eeeeeeeeeeee-B.pdf").exists()); // WHAT: the known one stays. WHY: theirs.
        assert!(folder.join(INDEX).exists()); // WHAT: the index stays. WHY: it is not a copy.
        let _ = std::fs::remove_dir_all(&folder); // WHAT: cleaned up. WHY: tests leave nothing.
    }
}
