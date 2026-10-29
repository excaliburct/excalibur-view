//! Opening a file that isn't a drawing (a spreadsheet, a letter, a picture)
//! with the program the computer opens that kind of file with.
//!
//! Excalibur View doesn't edit these yet. So the Studio saves a copy in the
//! person's own cache folder and hands it to the system, and says plainly
//! that it is a copy: a change saved in it goes nowhere, and belongs in the
//! company's own files. On a tablet the copy goes to the system's share and
//! open-in sheet, where there is one; where there isn't, the person is told
//! to open it on a computer.

use std::path::{Path, PathBuf}; // WHAT: paths. WHY: the copy's.

/// What the Studio says beside **Open with its program**.
pub const A_COPY: &str = "This opens a copy: save changes in the company files."; // WHAT: the warning. WHY: a change saved in a temporary copy is lost.

/// What a tablet with no way to hand a file on says instead.
pub const ON_A_COMPUTER: &str = "This kind of file can't be opened here: open it on a computer."; // WHAT: the sentence. WHY: said plainly rather than doing nothing.

/// A name safe to save a copy under: the file's own name, with anything a
/// file system or a shell could read as more than a name taken out, and its
/// extension kept so the system knows its kind.
pub fn safe_name(name: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(""); // WHAT: the last part only. WHY: a name never says where to write.
    let kept: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '(' | ')') {
                c
            } else {
                '_'
            }
        }) // WHAT: ordinary characters only. WHY: nothing that means something to a system.
        .take(120) // WHAT: at most 120 characters. WHY: deep temporary folders on Windows.
        .collect();
    let kept = kept.trim().trim_start_matches('.').to_string(); // WHAT: no leading dots or spaces. WHY: never a hidden file or `..`.
    if kept.is_empty() {
        "file".to_string()
    } else {
        kept
    } // WHAT: something, always. WHY: a copy needs a name.
}

/// What the Studio says when a file is a program or a shortcut.
pub const RUNS_ITSELF: &str = "This is a program or a shortcut, so Excalibur View won't open it: a file that runs itself could be anything. If it's meant to be here, get it from the company files."; // WHAT: the sentence. WHY: said plainly rather than running it.

/// Kinds of file the system would run rather than open: programs, scripts,
/// installers and shortcuts on Windows, Mac and Linux.
const RUNS: &[&str] = &[
    "exe", "com", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "msc", "msi", "msp", "scr", "pif", "cpl", "hta", "lnk", "url", "reg", "inf", "jar", "appref-ms", "application", "gadget", "dll", "sys", "scf", "settingcontent-ms", "library-ms", "search-ms", // WHAT: Windows. WHY: Explorer runs each of these.
    "app", "command", "tool", "pkg", "dmg", "workflow", "terminal", "webloc", "inetloc", "fileloc", // WHAT: Mac. WHY: `open` runs or mounts each of these.
    "desktop", "sh", "bash", "run", "bin", "appimage", "deb", "rpm", "py", "pl", // WHAT: Linux. WHY: a desktop may run each of these.
]; // WHAT: the list. WHY: an office's files are spreadsheets, letters and pictures; nothing in them needs to run.

/// Whether a file of this name is one the system would run.
pub fn runs_itself(name: &str) -> bool {
    let name = safe_name(name).to_lowercase(); // WHAT: the name as it would be saved. WHY: checked exactly as written to disk.
    let name = name.trim_end_matches(['.', ' ']); // WHAT: trailing dots and spaces gone. WHY: Windows drops them, so `run.exe.` is `run.exe`.
    match name.rsplit_once('.') {
        Some((_, ext)) => RUNS.contains(&ext), // WHAT: its kind on the list. WHY: refused.
        None => false, // WHAT: no kind at all. WHY: opened as a plain file.
    }
}

/// Saves `bytes` as a copy of file `id` called `name` in this person's own
/// cache folder, beside the kept drawings, and says where. Never a program
/// or a shortcut.
pub fn save_copy(id: &str, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if runs_itself(name) {
        return Err(RUNS_ITSELF.to_string()); // WHAT: refused before anything is written. WHY: a file somebody put in a job folder must never run on a field hand's computer with a double click.
    }
    let drawings = crate::server::cache_folder(); // WHAT: where this person's kept drawings are. WHY: their own folder, not the computer's shared temporary one another account could plant a file in.
    let folder = drawings
        .parent()
        .unwrap_or(&drawings)
        .join("copies")
        .join(crate::server::safe(id)); // WHAT: a folder of its own per file, beside the drawings. WHY: two files of the same name never overwrite each other.
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?; // WHAT: made. WHY: the copy goes in it.
    let path = folder.join(safe_name(name)); // WHAT: the copy. WHY: its own name, so the system knows its kind.
    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?; // WHAT: written. WHY: handed to its program.
    from_elsewhere(&path); // WHAT: marked as downloaded. WHY: Windows then opens Office files in Protected View, as it would from a browser.
    Ok(path)
}

/// Marks a copy as having come from another computer, as a browser does, so
/// Windows treats it with the same care (Office's Protected View, the
/// warning before anything runs). Nothing elsewhere.
fn from_elsewhere(path: &Path) {
    if cfg!(windows) {
        let mut mark = path.as_os_str().to_owned(); // WHAT: the copy's path. WHY: the mark is a stream beside it.
        mark.push(":Zone.Identifier"); // WHAT: the stream Windows reads. WHY: where a browser writes the same mark.
        let _ = std::fs::write(PathBuf::from(mark), "[ZoneTransfer]\r\nZoneId=3\r\n"); // WHAT: "from the internet". WHY: the most careful zone; a drive that can't hold it just goes without.
    }
}

/// Hands the copy to the system to open with its own program. On a tablet,
/// the share and open-in sheet; where there is none, why not.
pub fn open(path: &Path) -> Result<(), String> {
    if crate::platform::can_share() {
        crate::platform::share(path); // WHAT: the system's share and open-in sheet. WHY: how a tablet opens a file in another app.
        return Ok(());
    }
    if cfg!(any(target_os = "android", target_os = "ios")) {
        return Err(ON_A_COMPUTER.to_string()); // WHAT: a tablet with no sheet. WHY: said plainly.
    }
    let mut command = if cfg!(windows) {
        std::process::Command::new("explorer.exe") // WHAT: Explorer, given the file. WHY: opens it with its own program, and no shell reads the name.
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open") // WHAT: the Mac's `open`. WHY: the same.
    } else {
        std::process::Command::new("xdg-open") // WHAT: the desktop's opener. WHY: the same on Linux.
    };
    command
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("The computer couldn't open {}: {e}", path.display()))
    // WHAT: started, not waited for. WHY: the program stays usable while the file is open.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_is_saved_under_a_name_nothing_can_read_as_more_than_a_name() {
        assert_eq!(safe_name("Estimate.xlsx"), "Estimate.xlsx"); // WHAT: an ordinary name. WHY: kept as it is.
        assert_eq!(safe_name("..\\..\\evil.bat"), "evil.bat"); // WHAT: folders in front. WHY: only the name.
        assert_eq!(safe_name("a&b|c\"d.docx"), "a_b_c_d.docx"); // WHAT: characters a shell reads. WHY: taken out.
        assert_eq!(safe_name(".."), "file"); // WHAT: no name at all. WHY: still a name.
        let path = save_copy("file_test", "Cut list.xlsx", b"PK").unwrap(); // WHAT: a copy saved. WHY: as the Studio does.
        assert_eq!(std::fs::read(&path).unwrap(), b"PK"); // WHAT: its bytes. WHY: the file whole.
        assert!(path.ends_with("Cut list.xlsx")); // WHAT: its name. WHY: so the system knows its kind.
        let _ = std::fs::remove_dir_all(path.parent().unwrap()); // WHAT: cleaned up. WHY: tests leave nothing.
    }

    #[test]
    fn a_program_or_a_shortcut_is_never_saved_or_opened() {
        for name in ["Site photos.exe", "Pictures/Photo.LNK", "run.bat", "x.ps1", "a.scr", "b.hta", "c.command", "d.desktop", "e.AppImage", "f.exe.", "g.exe ", "h.url"] {
            assert!(runs_itself(name), "{name}"); // WHAT: each refused. WHY: the system would run it.
            assert_eq!(save_copy("file_runs", name, b"MZ"), Err(RUNS_ITSELF.to_string()), "{name}"); // WHAT: nothing written. WHY: refused first.
        }
        for name in ["Estimate.xlsx", "Letter.docx", "Photo.jpg", "Plan.dwg", "notes", "exe.pdf"] {
            assert!(!runs_itself(name), "{name}"); // WHAT: ordinary office files. WHY: opened as before.
        }
        assert!(!crate::server::cache_folder().parent().unwrap().join("copies").join(crate::server::safe("file_runs")).exists()); // WHAT: no folder made for the refused ones. WHY: nothing was saved.
    }
}
