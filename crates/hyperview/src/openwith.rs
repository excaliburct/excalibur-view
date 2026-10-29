//! Opening a file that isn't a drawing (a spreadsheet, a letter, a picture)
//! with the program the computer opens that kind of file with.
//!
//! Excalibur View doesn't edit these yet. So the Studio saves a copy to the
//! computer's temporary folder and hands it to the system, and says plainly
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

/// Saves `bytes` as a copy of file `id` called `name` in the temporary
/// folder, and says where.
pub fn save_copy(id: &str, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let folder = std::env::temp_dir()
        .join("Excalibur View copies")
        .join(crate::server::safe(id)); // WHAT: a folder of its own per file. WHY: two files of the same name never overwrite each other.
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?; // WHAT: made. WHY: the copy goes in it.
    let path = folder.join(safe_name(name)); // WHAT: the copy. WHY: its own name, so the system knows its kind.
    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?; // WHAT: written. WHY: handed to its program.
    Ok(path)
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
}
