//! Where this computer keeps its device key (docs/OFFICE-ENROLLMENT.md §1).
//!
//! Made on first run and kept for good. On Windows the private half is
//! sealed with DPAPI for the signed-in Windows user (`CurrentUser`), as
//! Citadel's `DpapiDeviceKeyStore` does, so another account on the same PC,
//! or the file copied to another PC, can't open it. Elsewhere it is a file
//! only its owner may read (0600). It never leaves the computer.

use std::path::{Path, PathBuf};

use crate::device::DeviceKey;

/// The file the key is kept in, inside the app's own settings folder.
pub const FILE: &str = "device.key"; // WHAT: the name. WHY: the same name Citadel uses

/// The most a key file may be. A sealed 32-byte key is a few hundred bytes.
const LARGEST: u64 = 4096; // WHAT: 4 KiB. WHY: Citadel's limit; anything bigger is damaged

/// The extra secret DPAPI mixes in, so another program's sealed blob can't be
/// passed off as this one.
#[cfg(windows)]
const ENTROPY: &[u8] = b"excalibur-device-key/1"; // WHAT: this key's label. WHY: as Citadel's "citadel-device-key/1"

/// The key kept in `folder`, made and kept now if there is none.
///
/// # Errors
///
/// When the kept key can't be read (damaged, or sealed for another Windows
/// user), or a new one can't be kept. A damaged key is never replaced
/// quietly: the computer would become a stranger to the office.
pub fn load_or_make(folder: &Path) -> Result<DeviceKey, String> {
    let path = folder.join(FILE); // WHAT: the key's file. WHY: one per app per user
    match std::fs::metadata(&path) {
        Ok(meta) if meta.len() > LARGEST => Err("this computer's office server key file is damaged. Remove it to link this computer again.".into()), // WHAT: refused. WHY: never read something that size
        Ok(_) => {
            let sealed = std::fs::read(&path).map_err(|e| format!("this computer's office server key couldn't be read: {e}"))?; // WHAT: the file. WHY: opened below
            let mut secret = open(&sealed)?; // WHAT: the 32 bytes. WHY: the key
            let key = DeviceKey::from_secret(&secret); // WHAT: the key. WHY: signs
            secret.fill(0); // WHAT: cleared. WHY: only the key holds it now
            Ok(key)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = DeviceKey::generate()?; // WHAT: a new key. WHY: first run
            keep(&path, &key)?; // WHAT: kept, unless another first run kept one first. WHY: the same key every run after
            read_back(&path) // WHAT: whichever key is on the disk now. WHY: review: two first runs at once must agree on one key
        }
        Err(e) => Err(format!("this computer's office server key couldn't be read: {e}")), // WHAT: said. WHY: never replaced quietly
    }
}

/// The key kept at `path`, read back.
fn read_back(path: &Path) -> Result<DeviceKey, String> {
    let sealed = std::fs::read(path).map_err(|e| format!("this computer's office server key couldn't be read: {e}"))?; // WHAT: the file, as kept. WHY: opened below
    let mut secret = open(&sealed)?; // WHAT: the 32 bytes. WHY: the key
    let key = DeviceKey::from_secret(&secret); // WHAT: the key. WHY: signs
    secret.fill(0); // WHAT: cleared. WHY: only the key holds it now
    Ok(key)
}

/// Keeps `key` at `path`, written whole or not at all, and never over a key
/// already there.
fn keep(path: &Path, key: &DeviceKey) -> Result<(), String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0); // WHAT: a count of part files this process made. WHY: two threads reading the clock in the same nanosecond still get different names
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; // WHAT: the folder. WHY: first run
    }
    let mut secret = key.secret(); // WHAT: the private half. WHY: sealed
    let sealed = seal(&secret); // WHAT: sealed for this user. WHY: DPAPI, or as it is in a 0600 file
    secret.fill(0); // WHAT: cleared. WHY: as above
    let sealed = sealed?;
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()); // WHAT: a fresh number. WHY: two first runs never share a part file
    let count = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed); // WHAT: this part file's number. WHY: as above
    let part = path.with_extension(format!("saving{}-{n}-{count}", std::process::id())); // WHAT: a file beside it, this run's own. WHY: put into place whole; create_new refuses a name already taken
    write_private(&part, &sealed).map_err(|e| e.to_string())?;
    let placed = place_new(&part, path, |from, to| std::fs::hard_link(from, to)); // WHAT: into place only if no key is there. WHY: never over another first run's key, on any disk
    let _ = std::fs::remove_file(&part); // WHAT: the part file gone, whatever happened. WHY: never a stray copy of a key
    placed.map_err(|e| e.to_string()) // WHAT: placed, or why not. WHY: the caller reads the key on the disk
}

/// Puts the part file `part` at `path` only if nothing is there yet: by a
/// hard link (`link`), or on a disk without hard links (FAT, exFAT, some
/// network shares) by a move that never replaces. When neither is possible
/// it refuses rather than replace. A key already there is kept, and this
/// part file is let go. (Excalibur CAD's `place_new`, aa669de.)
fn place_new(part: &Path, path: &Path, link: impl Fn(&Path, &Path) -> std::io::Result<()>) -> std::io::Result<()> {
    let placed = match link(part, path) {
        Ok(()) => return std::fs::remove_file(part), // WHAT: named, and the part's own name taken off. WHY: the key is whole under its real name only
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e), // WHAT: another first run's key is there. WHY: kept, below
        Err(_) => rename_no_replace(part, path), // WHAT: a disk without hard links. WHY: a move that never replaces, or a refusal
    };
    match placed {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => std::fs::remove_file(part), // WHAT: theirs is kept, this one let go. WHY: the caller reads the key on disk, so two first runs agree
        other => other, // WHAT: moved, or a refusal said. WHY: never a replace
    }
}

/// Moves `from` to `to` only if nothing is at `to`: `AlreadyExists` if
/// something is. Windows: `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING`;
/// Linux: `renameat2` with `RENAME_NOREPLACE`; elsewhere refused.
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _; // WHAT: paths as UTF-16. WHY: what the wide Windows call takes
        use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH}; // WHAT: the move, flushed. WHY: no MOVEFILE_REPLACE_EXISTING, so it never replaces
        let wide = |p: &Path| p.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>(); // WHAT: a path, ended by a zero. WHY: as Windows reads it
        let (f, t) = (wide(from), wide(to)); // WHAT: both paths. WHY: kept alive for the call
        // SAFETY: both pointers are to zero-ended UTF-16 buffers that live until the call returns; Windows only reads them.
        let moved = unsafe { MoveFileExW(windows::core::PCWSTR(f.as_ptr()), windows::core::PCWSTR(t.as_ptr()), MOVEFILE_WRITE_THROUGH) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: a move that never replaces
        moved.map_err(|e| if to.exists() { std::io::Error::from(std::io::ErrorKind::AlreadyExists) } else { std::io::Error::other(e.to_string()) }) // WHAT: done, or AlreadyExists when something is there. WHY: the caller keeps the key already there
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        use std::os::unix::ffi::OsStrExt as _; // WHAT: paths as bytes. WHY: as the system call takes them
        let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput)); // WHAT: a path, ended by a zero. WHY: as the system call reads it
        let (f, t) = (c(from)?, c(to)?); // WHAT: both paths. WHY: kept alive for the call
        // SAFETY: both pointers are to zero-ended strings that live until the call returns; the kernel only reads them.
        let done = unsafe { libc::renameat2(libc::AT_FDCWD, f.as_ptr(), libc::AT_FDCWD, t.as_ptr(), libc::RENAME_NOREPLACE) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Linux system call: a rename that never replaces
        if done == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) } // WHAT: moved, or why not (EEXIST reads as AlreadyExists). WHY: never a replace
    }
    #[cfg(not(any(windows, all(target_os = "linux", target_env = "gnu"))))]
    {
        let _ = (from, to); // WHAT: unused here. WHY: no move that never replaces on this system
        Err(std::io::Error::other("this disk can't make a new file without risking replacing one, so the office server key wasn't kept; use a folder on a local disk")) // WHAT: refused, in plain words. WHY: never a replace
    }
}

/// Writes `bytes` to a new file only its owner may read (0600 on Unix).
fn write_private(path: &PathBuf, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new(); // WHAT: how it is opened. WHY: made new, with its permissions from the start
    options.write(true).create_new(true); // WHAT: a new file, never one already there. WHY: review: nothing else's file is ever written into
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600); // WHAT: owner read and write only. WHY: §1: a 0600 file
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?; // WHAT: 0600 even if the file was there. WHY: a leftover's looser mode never stays
    }
    file.write_all(bytes)?;
    file.sync_all() // WHAT: on the disk before it is named. WHY: a power cut never leaves half a key
}

/// The private half as it is kept: sealed by DPAPI for this Windows user,
/// or the 32 bytes in a 0600 file elsewhere. A key of zeros is never kept.
fn seal(secret: &[u8; 32]) -> Result<Vec<u8>, String> {
    if secret.iter().all(|b| *b == 0) {
        return Err("this computer's new office server key is empty; nothing was kept".into()); // WHAT: refused. WHY: a key of zeros means the random numbers never arrived
    }
    #[cfg(windows)]
    return dpapi(secret, true); // WHAT: sealed. WHY: `CurrentUser`
    #[cfg(not(windows))]
    Ok(secret.to_vec()) // WHAT: as it is. WHY: the file's permissions keep it
}

/// The 32 bytes back from what was kept.
fn open(sealed: &[u8]) -> Result<[u8; 32], String> {
    #[cfg(windows)]
    let plain = dpapi(sealed, false).map_err(|_| "this computer's office server key can't be opened by this Windows account. Remove it to link this computer again.".to_string())?; // WHAT: unsealed. WHY: only this Windows user can
    #[cfg(not(windows))]
    let plain = sealed.to_vec(); // WHAT: as kept. WHY: a 0600 file
    let secret: [u8; 32] = plain.as_slice().try_into().map_err(|_| "this computer's office server key file is damaged. Remove it to link this computer again.".to_string())?; // WHAT: exactly 32 bytes. WHY: an Ed25519 private key
    let mut plain = plain;
    plain.fill(0); // WHAT: the copy cleared. WHY: only the key holds it
    Ok(secret)
}

/// DPAPI, CurrentUser, with this key's entropy: sealing or opening.
#[cfg(windows)]
fn dpapi(data: &[u8], sealing: bool) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: u32::try_from(data.len()).map_err(|e| e.to_string())?, pbData: data.as_ptr().cast_mut() }; // WHAT: what goes in. WHY: DPAPI reads it, never writes it
    let entropy = CRYPT_INTEGER_BLOB { cbData: u32::try_from(ENTROPY.len()).map_err(|e| e.to_string())?, pbData: ENTROPY.as_ptr().cast_mut() }; // WHAT: the label. WHY: as above
    let mut output = CRYPT_INTEGER_BLOB::default(); // WHAT: filled by DPAPI. WHY: freed below
    // SAFETY: the input and entropy blobs point at live slices DPAPI only reads; the output buffer DPAPI allocates is
    // copied out, cleared and freed once with LocalFree, and never touched after.
    unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: DPAPI CurrentUser, as Citadel's DpapiDeviceKeyStore
        let done = if sealing {
            CryptProtectData(&input, windows::core::PCWSTR::null(), Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output) // WHAT: sealed for this user. WHY: no prompt; a program has nobody to show one to
        } else {
            CryptUnprotectData(&input, None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output) // WHAT: opened. WHY: only for the user who sealed it
        };
        done.map_err(|e| e.to_string())?;
        let len = usize::try_from(output.cbData).unwrap_or(0); // WHAT: how much came out. WHY: copied
        if output.pbData.is_null() {
            return Ok(Vec::new()); // WHAT: nothing came out. WHY: nothing to free
        }
        let out = std::slice::from_raw_parts(output.pbData, len).to_vec(); // WHAT: copied out. WHY: DPAPI's buffer is freed next
        std::ptr::write_bytes(output.pbData, 0, len); // WHAT: cleared. WHY: an opened key never lingers in freed memory
        let _ = LocalFree(HLOCAL(output.pbData.cast())); // WHAT: freed. WHY: DPAPI's own
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0); // WHAT: a count of folders made. WHY: two tests reading the clock in the same nanosecond never share one
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(); // WHAT: a fresh name. WHY: tests run side by side
        let count = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed); // WHAT: this folder's number. WHY: as above
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("keys-{n}-{count}")) // WHAT: the folder. WHY: its own, every time
    }

    #[test]
    fn a_key_is_made_once_and_the_same_key_comes_back() {
        let dir = folder();
        let first = load_or_make(&dir).unwrap();
        let again = load_or_make(&dir).unwrap();
        assert_eq!(first.id(), again.id(), "the same computer every run");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(FILE)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "only its owner may read it");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_first_runs_at_once_agree_on_one_key() {
        let dir = folder(); // WHAT: a fresh folder. WHY: no key in it yet
        let runs: Vec<_> = (0..8).map(|_| { let dir = dir.clone(); std::thread::spawn(move || load_or_make(&dir).unwrap().id()) }).collect(); // WHAT: eight first runs together. WHY: as two apps opening at once
        let ids: std::collections::BTreeSet<String> = runs.into_iter().map(|r| r.join().unwrap()).collect(); // WHAT: the ids each got. WHY: must all be one
        assert_eq!(ids.len(), 1, "one key for the computer: {ids:?}");
        assert_eq!(load_or_make(&dir).unwrap().id(), ids.into_iter().next().unwrap(), "and it is the one on the disk"); // WHAT: the same key read later. WHY: the one on the disk
        assert!(std::fs::read_dir(&dir).unwrap().flatten().all(|e| e.file_name() == FILE), "no part file left"); // WHAT: only the key file. WHY: every part file removed
        let _ = std::fs::remove_dir_all(&dir); // WHAT: tidied. WHY: nothing left behind
    }

    #[test]
    fn without_hard_links_a_new_key_is_moved_in_only_into_an_empty_place() {
        let dir = folder(); // WHAT: a fresh folder. WHY: no key in it yet
        std::fs::create_dir_all(&dir).unwrap(); // WHAT: made. WHY: the files go in it
        let no_links = |_: &Path, _: &Path| Err(std::io::Error::other("hard links not supported")); // WHAT: a disk without hard links (FAT, exFAT, some shares). WHY: re-review: the fallback replaced a key
        let (first, second, kept) = (dir.join("first.part"), dir.join("second.part"), dir.join(FILE)); // WHAT: two runs' part files, and the key's place. WHY: two first runs at once
        std::fs::write(&first, b"first").unwrap(); // WHAT: the first run's key. WHY: put in first
        std::fs::write(&second, b"second").unwrap(); // WHAT: the second run's. WHY: must not replace it
        place_new(&first, &kept, no_links).unwrap(); // WHAT: into the empty place. WHY: moved
        place_new(&second, &kept, no_links).unwrap(); // WHAT: into a place already taken. WHY: let go, not moved over
        assert_eq!(std::fs::read(&kept).unwrap(), b"first", "the key already there is never replaced"); // WHAT: the first key. WHY: two first runs agree
        assert!(!first.exists() && !second.exists(), "no part file is left behind"); // WHAT: tidied. WHY: never a stray copy of a key
        let _ = std::fs::remove_dir_all(&dir); // WHAT: tidied. WHY: a test leaves nothing
    }

    #[test]
    fn a_damaged_key_is_said_and_never_replaced() {
        let dir = folder();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), b"short").unwrap();
        assert!(load_or_make(&dir).is_err());
        assert_eq!(std::fs::read(dir.join(FILE)).unwrap(), b"short", "left as it was");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_key_is_never_written_into_a_file_someone_else_made() {
        let dir = folder(); // WHAT: a fresh folder. WHY: its own files
        std::fs::create_dir_all(&dir).unwrap(); // WHAT: made. WHY: the file goes in it
        let planted = dir.join("planted"); // WHAT: a file somebody else made. WHY: the attack
        std::fs::write(&planted, b"theirs").unwrap(); // WHAT: written first. WHY: there before the key
        assert!(write_private(&planted, b"ours").is_err(), "an existing file is never written into"); // WHAT: refused. WHY: create_new
        assert_eq!(std::fs::read(&planted).unwrap(), b"theirs"); // WHAT: untouched. WHY: never written into
        let _ = std::fs::remove_dir_all(&dir); // WHAT: tidied. WHY: a test leaves nothing
    }
}
