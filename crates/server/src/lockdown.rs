//! Who may read and write the service's own folder on Windows.
//!
//! The installed service keeps its program, its database and the drawings in
//! `C:\ProgramData\Excalibur Hyperview\Server`. A folder made there
//! inherits ProgramData's permissions, under which every account on the
//! computer may read it and add files to it. So anybody signed in to the
//! office server could read the database, the remote-access token in it
//! included, and could put a DLL beside the service's program, or a connector
//! in place before it was fetched, which the service would then run as the
//! system. Each time the service starts, and when it is installed, its folder
//! is made its own: the system account and administrators, and nobody else.
//!
//! Granting the two was not enough on its own. An account that put a file
//! there before (or made the folder itself) owns what it made, and an owner
//! may always change who else may use it; a permission it wrote on its own
//! file is not inherited and so is not taken away by stopping inheritance;
//! and a DLL it left beside the program is loaded by the service before any
//! of this runs. So the folder's own entries are cleared, everything under it
//! is made to inherit from it alone, administrators are made the owners of
//! all of it, and any DLL beside the program is taken away: the service is
//! one program and ships none.

use std::ffi::OsString; // WHAT: arguments that are paths. WHY: a folder's name need not be valid text.
use std::path::Path; // WHAT: a folder. WHY: the one locked down.

/// The system account. Full control, on the folder and everything below.
const SYSTEM: &str = "*S-1-5-18:(OI)(CI)F"; // WHAT: well-known SID, not a name. WHY: works on a server in any language.
/// Administrators. Full control, on the folder and everything below.
const ADMINISTRATORS: &str = "*S-1-5-32-544:(OI)(CI)F"; // WHAT: the same, for administrators. WHY: whoever looks after the server can still back it up and read its log.
/// Administrators, as an owner.
const ADMINISTRATORS_OWN: &str = "*S-1-5-32-544"; // WHAT: the administrators group's SID. WHY: the owner of everything in the folder.

/// The `icacls` runs that make `folder` the service's own, in order. Each is
/// one argument list.
///
/// 1. The folder's own entries cleared (`/reset`): whatever an account wrote
///    on it goes.
/// 2. Nothing inherited from ProgramData, and full control for the system and
///    administrators only (`/inheritance:r /grant:r`).
/// 3. Everything below reset to inherit from the folder alone
///    (`folder\* /reset /T`): an entry an account wrote on its own file goes.
/// 4. Administrators made the owner of the folder and everything in it
///    (`/setowner /T`): nobody keeps an owner's right to change a file's
///    permissions back, including on a file made in the moment between 1 and 2.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn service_only_steps(folder: &Path) -> Vec<Vec<OsString>> {
    let whole = || folder.as_os_str().to_os_string(); // WHAT: the folder, as one argument. WHY: used by most steps.
    let below = folder.join("*").into_os_string(); // WHAT: everything inside the folder. WHY: icacls reads the `*` itself, so the folder's own entry is not touched by step 3.
    let quiet = ["/C", "/Q"]; // WHAT: carry on past a file it can't change, and quietly. WHY: one locked file must not leave the rest open, and a service has nobody to show it to.
    let step = |head: Vec<OsString>, rest: &[&str]| -> Vec<OsString> {
        head.into_iter()
            .chain(rest.iter().chain(quiet.iter()).map(|a| OsString::from(*a)))
            .collect() // WHAT: one run's arguments. WHY: the path first, then the switches.
    };
    vec![
        step(vec![whole()], &["/reset"]), // WHAT: the folder's own entries cleared. WHY: an entry an account wrote on the folder would otherwise stay, and be inherited by everything below it.
        step(
            vec![whole()],
            &["/inheritance:r", "/grant:r", SYSTEM, ADMINISTRATORS],
        ), // WHAT: nothing from ProgramData; the system and administrators only. WHY: ProgramData lets every account read and add files.
        step(vec![below], &["/reset", "/T"]), // WHAT: everything below inherits from the folder alone. WHY: an entry an account wrote on its own file is not inherited, so it outlived step 2.
        step(vec![whole()], &["/setowner", ADMINISTRATORS_OWN, "/T"]), // WHAT: administrators own all of it. WHY: an owner can always give itself access again.
    ]
}

/// WHAT: the steps that make the folder the service's folder sits in
/// (`ProgramData\Excalibur Hyperview`) the system's and administrators' alone,
/// on that folder only. WHY: ProgramData lets any account make a folder there
/// and own it; an account that made this one first could rename the locked
/// folder below it and put its own program in its place for the system to start
/// (audit, 2026-11-11). Granted to the folder itself, not passed on: what is
/// inside keeps its own permissions.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn parent_steps(parent: &Path) -> Vec<Vec<OsString>> {
    let whole = || parent.as_os_str().to_os_string(); // WHAT: the folder, as one argument. WHY: every step is on it alone
    let quiet = ["/C", "/Q"]; // WHAT: carry on past an error, quietly. WHY: as for the service's folder
    let step = |rest: &[&str]| -> Vec<OsString> {
        std::iter::once(whole()).chain(rest.iter().chain(quiet.iter()).map(|a| OsString::from(*a))).collect() // WHAT: one run's arguments. WHY: the path first, then the switches
    };
    vec![
        step(&["/reset"]), // WHAT: the folder's own entries cleared, the creator's included. WHY: whoever made it no longer holds it
        step(&["/inheritance:r", "/grant:r", SYSTEM_HERE, ADMINISTRATORS_HERE]), // WHAT: the system and administrators only, on this folder. WHY: nobody else may rename or delete what is in it
        step(&["/setowner", ADMINISTRATORS_OWN]), // WHAT: administrators own it. WHY: the account that made it first no longer does
    ]
}

/// WHAT: full control of one folder, not passed to what is inside. WHY: `parent_steps`.
const SYSTEM_HERE: &str = "*S-1-5-18:F";
/// WHAT: the same, for administrators. WHY: as above.
const ADMINISTRATORS_HERE: &str = "*S-1-5-32-544:F";

/// WHAT: runs `parent_steps` on `parent`. WHY: before the service's own folder is made in it.
#[cfg(windows)]
pub fn lock_parent(parent: &Path) {
    use std::os::windows::process::CommandExt; // WHAT: Windows-only process options. WHY: no console window.
    const NO_WINDOW: u32 = 0x0800_0000; // WHAT: CREATE_NO_WINDOW. WHY: as in lock_down
    let system = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows")); // WHAT: Windows' own folder. WHY: icacls by its full path
    let icacls = system.join("System32").join("icacls.exe"); // WHAT: the program. WHY: as above
    for args in parent_steps(parent) {
        let done = std::process::Command::new(&icacls).args(&args).creation_flags(NO_WINDOW).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status(); // WHAT: run and waited for. WHY: done before the service's folder is made
        if !matches!(done, Ok(status) if status.success()) {
            tracing::warn!("could not limit who may change {} ({:?})", parent.display(), args.get(1)); // WHAT: said. WHY: whoever looks after the server can run it by hand
        }
    }
}

/// Whether `name`, a file directly in the service's folder, is one the
/// service would load as a library and does not ship: any DLL, and the
/// `.local` redirection folder Windows looks in beside a program.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn planted(name: &str, program: &str) -> bool {
    let lower = name.to_ascii_lowercase(); // WHAT: the name as Windows compares it. WHY: `VERSION.DLL` is `version.dll`.
    lower.ends_with(".dll") || lower == format!("{}.local", program.to_ascii_lowercase())
    // WHAT: a library, or the redirection folder. WHY: the service is one program and ships neither, so either is somebody else's.
}

/// Makes `folder` the service's own (see [`service_only_steps`]) and takes
/// away any DLL beside its program (see [`planted`]). Said in the log when it
/// can't be done, and the server carries on: a server that works with
/// ProgramData's permissions is better than no server.
#[cfg(windows)]
pub fn lock_down(folder: &Path) {
    use std::os::windows::process::CommandExt; // WHAT: Windows-only process options. WHY: no console window.
    const NO_WINDOW: u32 = 0x0800_0000; // WHAT: CREATE_NO_WINDOW. WHY: a service has no desktop to show one on.
    let system = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows")); // WHAT: Windows' own folder. WHY: icacls is run by its full path, never found on a PATH somebody could change.
    let icacls = system.join("System32").join("icacls.exe"); // WHAT: the program. WHY: as above.
    let mut held = true; // WHAT: whether every step went through. WHY: said once at the end.
    for args in service_only_steps(folder) {
        let done = std::process::Command::new(&icacls)
            .args(&args)
            .creation_flags(NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(); // WHAT: run, and waited for. WHY: done before anything is served.
        match done {
            Ok(status) if status.success() => {} // WHAT: done. WHY: on to the next.
            Ok(status) => {
                held = false; // WHAT: noted. WHY: said below.
                tracing::warn!(
                    "could not limit who may use {} ({:?}: icacls said {status})",
                    folder.display(),
                    args.get(1)
                ); // WHAT: which step. WHY: whoever looks after the server can run it by hand.
            }
            Err(e) => {
                held = false; // WHAT: noted. WHY: said below.
                tracing::warn!("could not limit who may use {}: {e}", folder.display());
                // WHAT: icacls would not start. WHY: the folder keeps whatever permissions it had.
            }
        }
    }
    if held {
        tracing::info!(
            "{} is the system's and administrators' only",
            folder.display()
        ); // WHAT: said. WHY: the log shows it was done.
    }
    let program = crate::winsvc::PROGRAM; // WHAT: the service's own file name. WHY: its `.local` folder is looked for.
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned(); // WHAT: one name in the folder. WHY: judged below.
        if !planted(&name, program) {
            continue; // WHAT: the program, its data, its report. WHY: the service's own.
        }
        let path = entry.path(); // WHAT: where it is. WHY: taken away.
        let gone = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        }; // WHAT: taken away. WHY: the service would load it, as the system.
        match gone {
            Ok(()) => tracing::warn!(
                "took away {}, which this service does not ship and would have loaded",
                path.display()
            ), // WHAT: said. WHY: somebody put it there, and an administrator should know.
            Err(e) => tracing::error!(
                "{} is beside the service and could not be taken away ({e}); remove it by hand",
                path.display()
            ), // WHAT: said loudly. WHY: the service may load it on its next start.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn as_text(step: &[OsString]) -> Vec<String> {
        step.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect() // WHAT: one run's arguments as text. WHY: compared below.
    }

    #[test]
    fn the_service_folder_is_locked_to_the_system_and_administrators_only() {
        let folder = Path::new("C:/ProgramData/Excalibur Hyperview/Server"); // WHAT: the service's folder. WHY: the one locked down.
        let steps: Vec<Vec<String>> = service_only_steps(folder)
            .iter()
            .map(|s| as_text(s))
            .collect(); // WHAT: every run. WHY: checked below.
        let grants: Vec<&String> = steps
            .iter()
            .flatten()
            .filter(|a| a.starts_with("*S-1-5") && a.contains(':'))
            .collect(); // WHAT: who is granted anything, anywhere. WHY: exactly two.
        assert_eq!(
            grants,
            ["*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F"],
            "only the system and administrators"
        );
        let closing = steps
            .iter()
            .position(|s| s.contains(&"/inheritance:r".to_string()))
            .expect("inheritance from ProgramData stopped"); // WHAT: the step that closes the folder. WHY: ordered below.
        assert_eq!(
            steps[closing][0],
            folder.as_os_str().to_string_lossy(),
            "on the folder itself, as one argument"
        );
        assert!(
            steps[closing].contains(&"/grant:r".to_string()),
            "the grants replace any before"
        );
    }

    #[test]
    fn the_folder_above_is_locked_too_and_only_itself() {
        // WHAT: audit, 2026-11-11, M3. WHY: an account that made it first could swap the service's folder.
        let parent = Path::new("C:/ProgramData/Excalibur Hyperview"); // WHAT: the folder above. WHY: the one locked here
        let steps: Vec<Vec<String>> = parent_steps(parent).iter().map(|s| as_text(s)).collect(); // WHAT: every run. WHY: checked below
        assert!(steps.iter().all(|s| s[0] == parent.as_os_str().to_string_lossy()), "on the folder itself"); // WHAT: nothing below touched. WHY: what is inside keeps its own
        let grants: Vec<&String> = steps.iter().flatten().filter(|a| a.starts_with("*S-1-5") && a.contains(':')).collect(); // WHAT: who is granted. WHY: exactly two
        assert_eq!(grants, ["*S-1-5-18:F", "*S-1-5-32-544:F"], "the system and administrators, not passed on");
        assert!(steps.iter().any(|s| s.contains(&"/setowner".to_string()) && !s.contains(&"/T".to_string())), "owned by administrators, the folder alone");
        assert!(steps.iter().any(|s| s.contains(&"/inheritance:r".to_string())), "nothing from ProgramData");
    }

    #[test]
    fn what_an_account_left_in_the_folder_is_taken_from_it() {
        let folder = Path::new("C:/ProgramData/Excalibur Hyperview/Server"); // WHAT: the service's folder. WHY: as above.
        let steps: Vec<Vec<String>> = service_only_steps(folder)
            .iter()
            .map(|s| as_text(s))
            .collect(); // WHAT: every run. WHY: checked below.
        let at = |wanted: &dyn Fn(&Vec<String>) -> bool| steps.iter().position(wanted); // WHAT: where a step is. WHY: the order matters.
        let own_entries = at(&|s| {
            s[0] == folder.as_os_str().to_string_lossy() && s.contains(&"/reset".to_string())
        })
        .expect("the folder's own entries cleared"); // WHAT: an entry an account wrote on the folder. WHY: not inherited, so stopping inheritance left it, and it was inherited by everything below.
        let closing = at(&|s| s.contains(&"/inheritance:r".to_string())).unwrap(); // WHAT: the closing step. WHY: must come after the clearing.
        assert!(own_entries < closing, "cleared, then closed: {steps:?}");
        let below = at(&|s| {
            s[0].ends_with('*')
                && s.contains(&"/reset".to_string())
                && s.contains(&"/T".to_string())
        })
        .expect("everything below reset to inherit"); // WHAT: entries an account wrote on its own files. WHY: they outlived the grants.
        let owner = at(&|s| {
            s.contains(&"/setowner".to_string())
                && s.contains(&"*S-1-5-32-544".to_string())
                && s.contains(&"/T".to_string())
        })
        .expect("administrators made the owner of everything"); // WHAT: ownership taken. WHY: an owner can always give itself access again.
        assert!(
            closing < below && below < owner,
            "the owner is changed last, after anything made in between: {steps:?}"
        );
        assert!(
            steps.iter().all(|s| s.contains(&"/C".to_string())),
            "no step stops at one locked file"
        );
    }

    #[test]
    fn a_dll_beside_the_service_is_somebody_elses() {
        let program = "Hyperview-Server.exe"; // WHAT: the service's file. WHY: as installed.
        for name in [
            "version.dll",
            "USERENV.DLL",
            "bcrypt.Dll",
            "Hyperview-Server.exe.local",
            "HYPERVIEW-SERVER.EXE.LOCAL",
        ] {
            assert!(
                planted(name, program),
                "{name} would be left beside the service"
            ); // WHAT: a library or the redirection folder. WHY: the service ships neither, and loads both before anything runs.
        }
        for name in [
            "Hyperview-Server.exe",
            "hyperview-data",
            "last-install.txt",
            "Hyperview-Server.exe.old",
            "notes.dll.txt",
        ] {
            assert!(!planted(name, program), "{name} is the service's own"); // WHAT: what the service keeps there. WHY: never taken away.
        }
    }
}
