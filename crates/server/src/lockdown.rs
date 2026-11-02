//! Who may read and write the service's own folder on Windows.
//!
//! The installed service keeps its program, its database and the drawings in
//! `C:\ProgramData\Excalibur Hyperview\Server`. A folder made there
//! inherits ProgramData's permissions, under which every account on the
//! computer may read it and add files to it. So anybody signed in to the
//! office server could read the database, the remote-access token in it
//! included, and could put a DLL beside the service's program, or a connector
//! in place before it was fetched, which the service would then run as the
//! system. Each time the service starts, its folder is made its own: the
//! system account and administrators, and nobody else.

use std::ffi::OsString; // WHAT: arguments that are paths. WHY: a folder's name need not be valid text.
use std::path::Path; // WHAT: a folder. WHY: the one locked down.

/// What `icacls` is asked: nothing inherited, and full control for the system
/// account the service runs as and for administrators, on the folder and on
/// everything already in it. Well-known SIDs rather than names, so it works on
/// a server in any language.
pub fn service_only_acl(folder: &Path) -> Vec<OsString> {
    let mut args = vec![folder.as_os_str().to_os_string()]; // WHAT: the folder, first and whole. WHY: what is locked down.
    for arg in [
        "/inheritance:r",          // WHAT: nothing inherited from above. WHY: ProgramData lets every account read and add files.
        "/grant:r",                // WHAT: these grants, replacing any before. WHY: the list below is the whole of who may.
        "*S-1-5-18:(OI)(CI)F",     // WHAT: the system account, full control, on everything below. WHY: the service runs as it.
        "*S-1-5-32-544:(OI)(CI)F", // WHAT: Administrators, full control. WHY: whoever looks after the server can still back it up and read its log.
        "/T",                      // WHAT: what is already there too. WHY: the database and the program were made before this ran.
        "/C",                      // WHAT: carry on past a file it can't change. WHY: one locked file must not leave the rest open.
        "/Q",                      // WHAT: quietly. WHY: nothing to show.
    ] {
        args.push(arg.into()); // WHAT: added. WHY: in order.
    }
    args
}

/// Makes `folder` the service's own (see [`service_only_acl`]). Said in the
/// log when it can't be done, and the server carries on: a server that works
/// with ProgramData's permissions is better than no server.
#[cfg(windows)]
pub fn lock_down(folder: &Path) {
    use std::os::windows::process::CommandExt; // WHAT: Windows-only process options. WHY: no console window.
    const NO_WINDOW: u32 = 0x0800_0000; // WHAT: CREATE_NO_WINDOW. WHY: a service has no desktop to show one on.
    let system = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows")); // WHAT: Windows' own folder. WHY: icacls is run by its full path, never found on a PATH somebody could change.
    let done = std::process::Command::new(system.join("System32").join("icacls.exe"))
        .args(service_only_acl(folder))
        .creation_flags(NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status(); // WHAT: run, and waited for. WHY: done before anything is served.
    match done {
        Ok(status) if status.success() => tracing::info!("{} is readable by the system and administrators only", folder.display()), // WHAT: said. WHY: the log shows it was done.
        Ok(status) => tracing::warn!("could not limit who may read {} (icacls said {status})", folder.display()), // WHAT: said. WHY: the folder keeps ProgramData's permissions.
        Err(e) => tracing::warn!("could not limit who may read {}: {e}", folder.display()), // WHAT: said. WHY: likewise.
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_service_folder_is_locked_to_the_system_and_administrators_only() {
        let args: Vec<String> = super::service_only_acl(std::path::Path::new("C:/ProgramData/Excalibur Hyperview/Server"))
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(); // WHAT: the arguments as text. WHY: compared below.
        assert_eq!(args[0], "C:/ProgramData/Excalibur Hyperview/Server", "the folder first, whole, as one argument");
        assert!(args.contains(&"/inheritance:r".to_string()), "nothing inherited from ProgramData");
        let grants: Vec<&String> = args.iter().filter(|a| a.starts_with('*')).collect(); // WHAT: who is granted anything. WHY: exactly two.
        assert_eq!(grants, ["*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F"], "only the system and administrators");
        assert!(args.contains(&"/T".to_string()), "and what is already there");
    }
}
