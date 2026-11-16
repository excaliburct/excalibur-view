//! Who may read and write the service's own folder on Windows.
//!
//! The installed service keeps its program, its database and the drawings in
//! `C:\ProgramData\Excalibur Hyperview\Server`. A folder made there
//! inherits `ProgramData`'s permissions, under which every account on the
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
//!
//! All of that is done with Windows' own calls: the folder's owner and
//! protected DACL in one `SetNamedSecurityInfoW` (which passes the entries on
//! to everything inside), then each thing inside reset to inherit only
//! (`TreeResetNamedSecurityInfoW`); each file's new permissions are written in
//! a single step. Before 0.7.1 it was
//! four `icacls` runs, and between two of them the folder had no entries at
//! all, so for a moment even an administrator couldn't read in it: an install
//! reading its own report just then said it "stopped without saying why"
//! (found by the Windows end-to-end test, 2026-11-12).

#[cfg(windows)]
use std::path::Path; // WHAT: a folder. WHY: the one locked down.

/// The service's folder's permissions, as SDDL: owned by administrators;
/// protected (nothing from `ProgramData`); the system and administrators in
/// full, passed on to everything inside; nobody else.
pub const SERVICE_ONLY: &str = "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"; // WHAT: well-known SIDs, not names. WHY: works on a server in any language; an owner can always give itself access again, so administrators own it

/// The folder above's permissions (`ProgramData\Excalibur Hyperview`, and
/// `ProgramData\Excalibur Office`): owned by administrators, protected, the
/// system and administrators in full on the folder itself only.
pub const PARENT_ONLY: &str = "O:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)"; // WHAT: the folder alone, nothing passed on. WHY: what is inside keeps its own; nobody else may rename what is in it (audit, 2026-11-11)

/// WHAT: makes the folder the service's folder sits in the system's and
/// administrators' alone, on that folder only, in one call. WHY: `ProgramData`
/// lets any account make a folder there and own it; an account that made this
/// one first could rename the locked folder below it and put its own program
/// in its place for the system to start (audit, 2026-11-11).
#[cfg(windows)]
pub fn lock_parent(parent: &Path) {
    if let Err(e) = set(parent, PARENT_ONLY, false) {
        tracing::warn!("could not limit who may change {} ({e})", parent.display()); // WHAT: said. WHY: whoever looks after the server can see it
    }
}

/// Sets `sddl`'s owner and protected DACL on `path` in one call. With `tree`,
/// everything inside is reset to inherit from it alone, each file's
/// permissions written in one step, the entries an account wrote on its own
/// files taken away; a file that can't be changed is passed over, counted and
/// said. The folder never stands without entries, so an administrator can
/// always read in it.
///
/// # Errors
///
/// When the SDDL doesn't read, or Windows refuses the folder itself.
#[cfg(windows)]
pub fn set(path: &Path, sddl: &str, tree: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt as _; // WHAT: UTF-16. WHY: Windows' W calls
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW, TreeResetNamedSecurityInfoW, ProgressInvokeOnError, PROG_INVOKE_SETTING, SDDL_REVISION_1, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, ACL, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSID, UNPROTECTED_DACL_SECURITY_INFORMATION};
    /// Counts each object Windows couldn't change, and carries on.
    unsafe extern "system" fn failed(_name: windows_sys::core::PCWSTR, status: u32, _setting: *mut PROG_INVOKE_SETTING, args: *const core::ffi::c_void, _set: windows_sys::core::BOOL) {
        if status != 0 && !args.is_null() {
            // SAFETY: `args` is the counter below, alive for the whole call that invokes this.
            unsafe { (*args.cast::<std::sync::atomic::AtomicUsize>()).fetch_add(1, std::sync::atomic::Ordering::Relaxed) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: counting a file that couldn't be changed
        }
    }
    let text: Vec<u16> = std::ffi::OsStr::new(sddl).encode_wide().chain(std::iter::once(0)).collect(); // WHAT: the SDDL. WHY: as Windows takes it
    let mut name: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect(); // WHAT: the folder. WHY: likewise
    let missed = std::sync::atomic::AtomicUsize::new(0); // WHAT: files passed over. WHY: said
    // SAFETY: every pointer is to a live local; the owner and DACL point into the descriptor Windows allocates, which is freed
    // once after the call that copies them; the callback gets only the counter, alive for the whole call.
    let code = unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: a folder's owner and DACL set in one call
        let mut descriptor: *mut core::ffi::c_void = std::ptr::null_mut(); // WHAT: filled by Windows. WHY: the parsed SDDL
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(text.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) == 0 {
            return Err(format!("the permissions don't read ({})", std::io::Error::last_os_error())); // WHAT: refused. WHY: never set half
        }
        let (mut owner, mut defaulted): (PSID, i32) = (std::ptr::null_mut(), 0); // WHAT: filled by Windows. WHY: the owner
        let (mut present, mut dacl) = (0i32, std::ptr::null_mut::<ACL>()); // WHAT: filled by Windows. WHY: the DACL
        GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted);
        GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted);
        let code = if owner.is_null() || present == 0 || dacl.is_null() {
            87 // WHAT: ERROR_INVALID_PARAMETER. WHY: never an empty grant or no owner
        } else {
            match SetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, std::ptr::null_mut(), std::ptr::null_mut(), dacl, std::ptr::null()) { // WHAT: the folder's protected DACL first, in one write; Windows passes the inheritable entries on to everything inside, each in one write. WHY: never a moment without entries; the owner's own right to change the DACL is enough for this, where changing the owner first needs a privilege (found by the Windows end-to-end test, 2026-11-13)
                0 => SetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, owner, std::ptr::null_mut(), std::ptr::null(), std::ptr::null()), // WHAT: then the owner. WHY: administrators now have full control, which includes taking ownership
                failed => failed, // WHAT: Windows' reason. WHY: said by the caller
            }
        };
        if code == 0 && tree {
            let mut empty: *mut core::ffi::c_void = std::ptr::null_mut(); // WHAT: filled by Windows. WHY: a DACL with no entries of its own
            let none: Vec<u16> = std::ffi::OsStr::new("D:").encode_wide().chain(std::iter::once(0)).collect(); // WHAT: an empty, present DACL. WHY: each thing inside keeps only what it inherits
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(none.as_ptr(), SDDL_REVISION_1, &mut empty, std::ptr::null_mut()) != 0 {
                let (mut empty_present, mut empty_dacl, mut empty_defaulted) = (0i32, std::ptr::null_mut::<ACL>(), 0i32); // WHAT: filled by Windows. WHY: the empty DACL
                GetSecurityDescriptorDacl(empty, &mut empty_present, &mut empty_dacl, &mut empty_defaulted);
                for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
                    let mut inside: Vec<u16> = entry.path().as_os_str().encode_wide().chain(std::iter::once(0)).collect(); // WHAT: one thing inside. WHY: reset with all below it
                    let reset = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION; // WHAT: owner, and a DACL that inherits. WHY: only what the folder passes on
                    if TreeResetNamedSecurityInfoW(inside.as_mut_ptr(), SE_FILE_OBJECT, reset, owner, std::ptr::null_mut(), empty_dacl, std::ptr::null(), 0, Some(failed), ProgressInvokeOnError, (&raw const missed).cast()) != 0 {
                        missed.fetch_add(1, std::sync::atomic::Ordering::Relaxed); // WHAT: counted. WHY: said below
                    } // WHAT: its own entries and those below it taken away, each written once with what it inherits. WHY: an entry an account wrote on its own file goes
                }
                LocalFree(empty); // WHAT: let go. WHY: Windows copied it
            }
        }
        LocalFree(descriptor); // WHAT: let go. WHY: Windows copied what it needed
        code
    };
    let passed_over = missed.load(std::sync::atomic::Ordering::Relaxed); // WHAT: how many weren't changed. WHY: said
    if passed_over > 0 {
        tracing::warn!("{passed_over} files in {} could not be made the system's and administrators' only", path.display()); // WHAT: said. WHY: someone should look
    }
    if code == 0 { Ok(()) } else { Err(format!("Windows said {}", std::io::Error::from_raw_os_error(i32::try_from(code).unwrap_or(i32::MAX)))) } // WHAT: done, or why not. WHY: said by the caller
}

/// Whether `name`, a file directly in the service's folder, is one the
/// service would load as a library and does not ship: any DLL, and the
/// `.local` redirection folder Windows looks in beside a program.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
#[allow(clippy::case_sensitive_file_extension_comparisons)] // WHAT: `.dll` compared as written. WHY: the name is lower-cased on the line before, so the comparison already ignores case; the lint can't see that
pub fn planted(name: &str, program: &str) -> bool {
    let lower = name.to_ascii_lowercase(); // WHAT: the name as Windows compares it. WHY: `VERSION.DLL` is `version.dll`.
    lower.ends_with(".dll") || lower == format!("{}.local", program.to_ascii_lowercase())
    // WHAT: a library, or the redirection folder. WHY: the service is one program and ships neither, so either is somebody else's.
}

/// Makes `folder` the service's own (see [`SERVICE_ONLY`] and [`set`]) and takes
/// away any DLL beside its program (see [`planted`]). Said in the log when it
/// can't be done, and the server carries on: a server that works with
/// ProgramData's permissions is better than no server.
#[cfg(windows)]
pub fn lock_down(folder: &Path) {
    match set(folder, SERVICE_ONLY, true) {
        Ok(()) => tracing::info!("{} is the system's and administrators' only", folder.display()), // WHAT: said. WHY: the log shows it was done.
        Err(e) => tracing::warn!("could not limit who may use {} ({e})", folder.display()), // WHAT: said. WHY: the folder keeps whatever permissions it had; whoever looks after the server can see it.
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

    #[test]
    fn the_service_folder_is_the_system_s_and_administrators_only_and_its_parent_alone() {
        assert_eq!(SERVICE_ONLY, "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)", "owned by administrators, protected, the system and administrators passed on to everything"); // WHAT: exactly this. WHY: nobody else, nothing from ProgramData
        assert_eq!(PARENT_ONLY, "O:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)", "the folder above, itself only"); // WHAT: exactly this. WHY: what is inside keeps its own
        for sddl in [SERVICE_ONLY, PARENT_ONLY] {
            assert!(sddl.contains("D:P("), "protected: {sddl}"); // WHAT: nothing inherited. WHY: ProgramData lets every account read and add files
            assert_eq!(sddl.matches("(A;").count(), 2, "two entries: {sddl}"); // WHAT: the two. WHY: nobody else
        }
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
