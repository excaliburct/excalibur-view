//! What every Excalibur app on one computer shares about the office's server
//! (docs/OFFICE-ENROLLMENT.md §2): its address and certificate pin, kept in
//! one per-user file, `%LocalAppData%\Excalibur\office.json` on Windows or
//! `~/.config/Excalibur/office.json` elsewhere (on iOS, the app's own
//! `Library/Application Support/Excalibur/office.json`), so the second app on a PC
//! never asks again; whether its office certificate was ever seen verified;
//! and which addresses a first contact may take a pin from.
//!
//! The file is JSON:
//!
//! ```json
//! { "address": "https://192.168.1.10:8714", "pin": "<64 lowercase hex>", "verified": false }
//! ```
//!
//! `address` is the server's HTTPS address; `pin` the SPKI SHA-256 of its
//! certificate; `verified` true once an app has seen a valid office
//! certificate for that pin, after which no app accepts the server
//! unverified again. Any other field another app wrote is kept as it was
//! whenever an app writes the file.
//!
//! The contract is Excalibur View's `docs/OFFICE-ENROLLMENT.md` §2; every
//! Excalibur app keeps this file the same.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The shared file's contents.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shared {
    pub address: String, // WHAT: the server's HTTPS address. WHY: the other apps find it here
    pub pin: String, // WHAT: its certificate pin. WHY: the other apps hold it to the same certificate
    #[serde(default)]
    pub verified: bool, // WHAT: whether an app has seen its office certificate verified. WHY: guard 6: never accepted unverified after
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>, // WHAT: every other field in the file, as it was. WHY: another app (View, CAD, Archetype, LatCon Estimating) may keep more there, and no app loses it
}

/// Where the shared file is for this user. On an iPad or iPhone, a file an
/// earlier build kept under `$HOME/.config` is moved across once.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn shared_file() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from); // WHAT: one environment variable, as a path. WHY: read the same way each time
    let here = shared_file_for(&Homes { local_app_data: var("LOCALAPPDATA"), xdg_config: var("XDG_CONFIG_HOME"), home: var("HOME") }, Platform::this())?; // WHAT: this system's place for it. WHY: §2.3, and Library/Application Support on iOS
    if Platform::this() == Platform::Ios {
        if let Some(old) = shared_file_for(&Homes { local_app_data: None, xdg_config: None, home: var("HOME") }, Platform::Unix) {
            match move_once(&old, &here) {
                Ok(true) => eprintln!("the office server's shared record was moved from {} to {}", old.display(), here.display()), // WHAT: noted in the log. WHY: the move can be seen
                Ok(false) => {} // WHAT: nothing to copy. WHY: a new install, or copied before
                Err(e) => eprintln!("the office server's shared record couldn't be moved from {}: {e}", old.display()), // WHAT: said. WHY: the app still connects by its own pin book
            }
        }
    }
    Some(here)
}

/// The variables a home folder is found from.
#[derive(Debug, Default)]
struct Homes {
    local_app_data: Option<PathBuf>, // WHAT: %LocalAppData%. WHY: Windows
    xdg_config: Option<PathBuf>, // WHAT: $XDG_CONFIG_HOME. WHY: Linux, and Android, whose app sets it
    home: Option<PathBuf>, // WHAT: $HOME. WHY: everything else
}

/// Which rule finds the shared file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    Windows, // WHAT: %LocalAppData%. WHY: §2.3
    Ios, // WHAT: the app's Library/Application Support. WHY: an iOS app may only make folders in Documents, Library and tmp
    Unix, // WHAT: ~/.config, or $XDG_CONFIG_HOME. WHY: §2.3; Linux, the Mac and Android
}

impl Platform {
    /// The one this program was built for.
    fn this() -> Platform {
        if cfg!(windows) {
            Platform::Windows // WHAT: Windows. WHY: as built
        } else if cfg!(target_os = "ios") {
            Platform::Ios // WHAT: iOS. WHY: as built
        } else {
            Platform::Unix // WHAT: the rest. WHY: as built
        }
    }
}

/// Where the shared file is, given the home variables and the platform.
///
/// On iOS `$HOME` is the app's sandbox container. Apple's File System
/// Programming Guide lets an app make folders only inside its `Documents`,
/// `Library` and `tmp` folders, and puts configuration files in
/// `Library/Application Support`; a `.config` folder at the container's top
/// is outside all three, so the record was never kept there.
fn shared_file_for(homes: &Homes, platform: Platform) -> Option<PathBuf> {
    let base = match platform {
        Platform::Windows => homes.local_app_data.clone(), // WHAT: %LocalAppData%. WHY: §2.3
        Platform::Ios => homes.home.as_ref().map(|h| h.join("Library").join("Application Support")), // WHAT: Library/Application Support. WHY: where an iOS app may keep its own files
        Platform::Unix => homes.xdg_config.clone().or_else(|| homes.home.as_ref().map(|h| h.join(".config"))), // WHAT: ~/.config. WHY: §2.3, unchanged
    };
    base.map(|b| b.join("Excalibur").join("office.json")) // WHAT: `Excalibur\office.json`. WHY: §2.3
}

/// Moves `old` to `new` once, for a file an earlier build kept somewhere it
/// no longer lives: only when `old` is a plain file (a link there is never
/// followed) and nothing at all is at `new` (not a file, not a link, not even
/// a link to nothing). True when it moved.
///
/// Race-free against a newer record appearing at `new` at any moment: the old
/// file is read through a handle opened without following links, written to
/// a temporary file beside `new`, and put in place with a hard link, which
/// fails if anything is at `new` by then. Nothing is ever written through an
/// existing file or a link. Only once the new one is in place is the old one
/// removed, so a second copy of a sign-in or a pin isn't left behind.
///
/// # Errors
///
/// When the new folder can't be made, or the copy or the link fails for any
/// reason but something already being at `new`.
pub fn move_once(old: &Path, new: &Path) -> std::io::Result<bool> {
    use std::io::ErrorKind;
    let Some(mut source) = open_plain(old)? else {
        return Ok(false); // WHAT: nothing to move, or not a plain file. WHY: a link is never followed, so its target is never read
    };
    if let Some(dir) = new.parent() {
        std::fs::create_dir_all(dir)?; // WHAT: the new folder. WHY: a fresh install has none yet
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()); // WHAT: a unique part for the temporary name. WHY: two starts at once never share one
    let name = new.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(); // WHAT: the file's own name. WHY: the temporary sits beside it, named after it
    let temporary = new.with_file_name(format!(".{name}.moving-{}-{stamp}", std::process::id())); // WHAT: a temporary file beside the target. WHY: same folder, so the link can't cross volumes
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?; // WHAT: a brand-new file. WHY: create_new never opens an existing file or a link
    let placed = std::io::copy(&mut source, &mut out).and_then(|_| out.sync_all()).and_then(|()| std::fs::hard_link(&temporary, new)); // WHAT: copied, flushed, and linked into place. WHY: the link fails if anything is at the target, so a newer record is never replaced
    let _ = std::fs::remove_file(&temporary); // WHAT: the temporary name gone. WHY: the record lives on under its own name, or wasn't placed
    match placed {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => return Ok(false), // WHAT: something got there first. WHY: never over a newer record (a pin or verified flag never goes back)
        Err(e) => return Err(e), // WHAT: the copy or link failed. WHY: said; the old file stays, so nothing is lost
    }
    std::fs::remove_file(old)?; // WHAT: the old copy removed. WHY: security review L3: no stale sign-in or pin left in the old place
    Ok(true)
}

/// `path` opened for reading if it is a plain file, without following a
/// link at its last step; `None` when there is nothing there, it is a link,
/// or it isn't a plain file.
fn open_plain(path: &Path) -> std::io::Result<Option<std::fs::File>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true); // WHAT: read only. WHY: the old file is never written
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW); // WHAT: refuse a link at the last step. WHY: security review: the check and the read are one step, so nothing can be swapped in between
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Ok(None); // WHAT: a link, refused. WHY: no O_NOFOLLOW here; the move itself only runs on iOS
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None), // WHAT: nothing there. WHY: a fresh install
        Err(e) if is_a_link(&e) => return Ok(None), // WHAT: a link. WHY: never followed
        Err(e) => return Err(e), // WHAT: something else. WHY: said
    };
    Ok(file.metadata()?.is_file().then_some(file)) // WHAT: kept only if it is a plain file. WHY: a folder or a device is never read as a record
}

/// Whether an open failed because the last step was a link (`O_NOFOLLOW`).
fn is_a_link(e: &std::io::Error) -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
    {
        e.raw_os_error() == Some(libc::ELOOP) // WHAT: ELOOP. WHY: what O_NOFOLLOW gives for a link
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
    {
        let _ = e; // WHAT: unused. WHY: no O_NOFOLLOW here; links are refused before opening
        false
    }
}

/// The shared file, read: `Ok(None)` when there is none, `Err` when it is
/// there but doesn't read (which never means "no pin").
///
/// # Errors
///
/// When the file is there and damaged.
pub fn read(path: &Path) -> Result<Option<Shared>, String> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > MOST_BYTES => return Err("the office server's shared record is damaged".into()), // WHAT: far too big, by its size alone. WHY: review: never read into memory first
        Ok(meta) if !meta.is_file() => return Err("the office server's shared record isn't a file".into()), // WHAT: a folder or a device in its place. WHY: never read
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None), // WHAT: none yet. WHY: no app on this PC has linked
        Err(_) => return Err("the office server's shared record can't be read".into()), // WHAT: there, unreadable. WHY: the same as damaged
    }
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() as u64 > MOST_BYTES => Err("the office server's shared record is damaged".into()), // WHAT: grew since it was looked at. WHY: never trusted
        Ok(bytes) => serde_json::from_slice::<Shared>(&bytes)
            .map_err(|_| "the office server's shared record is damaged".to_string())
            .and_then(|s| if crate::pinned::is_pin(&s.pin) { Ok(Some(s)) } else { Err("the office server's shared record holds no pin".to_string()) }), // WHAT: read, with a real pin. WHY: a damaged one is said, never a first contact
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None), // WHAT: none yet. WHY: no app on this PC has linked
        Err(_) => Err("the office server's shared record can't be read".into()), // WHAT: there, unreadable. WHY: the same as damaged
    }
}

/// The most the shared file may be.
const MOST_BYTES: u64 = 16 * 1024; // WHAT: 16 KiB. WHY: an address and a pin are a few hundred bytes

/// Writes the shared file whole.
///
/// # Errors
///
/// When it can't be written.
pub fn write(path: &Path, shared: &Shared) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; // WHAT: `Excalibur`. WHY: the first app makes it
    }
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()); // WHAT: a fresh number. WHY: two apps writing at once never share a part file
    let part = path.with_extension(format!("part{n}"));
    let written = std::fs::write(&part, serde_json::to_vec_pretty(shared).unwrap_or_default()).and_then(|()| std::fs::rename(&part, path)); // WHAT: written beside it, then moved. WHY: all at once
    if let Err(e) = written {
        let _ = std::fs::remove_file(&part); // WHAT: the leftover. WHY: never a stray file
        return Err(e.to_string());
    }
    Ok(())
}

/// `host:port` of an address, as the pin book keys a server.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn server_of(address: &str) -> Option<String> {
    let url = url::Url::parse(address.trim()).ok()?; // WHAT: read as the connection reads it. WHY: one server, however written
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase(); // WHAT: the host. WHY: as the pin book writes it
    Some(format!("{host}:{}", url.port_or_known_default()?)) // WHAT: host and port. WHY: the same key
}

/// Whether an IP address is on a local network: a private IPv4 range,
/// `fc00::/7`, `fe80::/10` or loopback (§2.2).
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn is_local(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(), // WHAT: 10/8, 172.16/12, 192.168/16, 127/8, 169.254/16. WHY: §2.2's private ranges, and the address Windows gives itself without a router
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80, // WHAT: ::1, fc00::/7, fe80::/10. WHY: §2.2
    }
}

/// Whether `host:port` is on a local network: every address it resolves to
/// is local. A name that resolves to nothing is not.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn host_is_local(host: &str, port: u16) -> bool {
    local_addresses(host, port).is_some() // WHAT: resolved, every address local. WHY: the one rule, in one place
}

/// Every address `host:port` resolves to, looked up once, when every one is
/// on a local network; `None` when any is not, or none is found. A first
/// contact connects to exactly these, so a name can't be answered by one
/// address at the check and another at the connection (DNS rebinding).
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn local_addresses(host: &str, port: u16) -> Option<Vec<std::net::SocketAddr>> {
    let bare = host.trim_start_matches('[').trim_end_matches(']'); // WHAT: an IPv6 address without its brackets. WHY: parsed as an address
    let found: Vec<std::net::SocketAddr> = match bare.parse::<IpAddr>() {
        Ok(ip) => vec![std::net::SocketAddr::new(ip, port)], // WHAT: an address as typed. WHY: nothing to look up
        Err(_) => std::net::ToSocketAddrs::to_socket_addrs(&(bare, port)).ok()?.collect(), // WHAT: every address the name gives, once. WHY: the same list is checked and connected to
    };
    (!found.is_empty() && found.iter().all(|a| is_local(a.ip()))).then_some(found) // WHAT: all local, or none taken. WHY: a name that also points outside could be answered from there
}

// ---- the office certificate (§2.4) ---------------------------------------------

/// What an office certificate says, once checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate {
    pub company: String, // WHAT: the office's name. WHY: shown
    pub license: String, // WHAT: its licence id. WHY: shown
    pub pin: String, // WHAT: the 8714 pin it vouches for. WHY: must match the connection
    pub citadel_pin: Option<String>, // WHAT: the cell's 47110 pin, if any. WHY: LatCon Estimating checks it
    pub issued: String, // WHAT: YYYY-MM-DD. WHY: shown
}

/// Reads and checks an office certificate: the six statement lines of §2.4,
/// then `signature <key name> <128 hex>`, an Ed25519 signature of the six
/// lines (each ended by `\n`) by one of `keys` (name, 64-hex public key).
///
/// The signature line's form is provisional: signing certificates is a later
/// step, and this reads what that step is planned to write.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn check_certificate(text: &str, keys: &[(&str, &str)]) -> Option<Certificate> {
    let lines: Vec<&str> = text.lines().collect(); // WHAT: its lines. WHY: read one by one
    if lines.len() < 7 || lines[0] != "excalibur-office-cert/1" {
        return None; // WHAT: not a certificate. WHY: never verified
    }
    let field = |i: usize, name: &str| lines[i].strip_prefix(name).and_then(|r| r.strip_prefix(' ')).map(str::to_string); // WHAT: one named line's value. WHY: in their fixed order
    let company = field(1, "company")?;
    let license = field(2, "license")?;
    let pin = field(3, "pin").filter(|p| crate::pinned::is_pin(p))?;
    let citadel = field(4, "citadel-pin")?;
    let issued = field(5, "issued")?;
    let mut signature = lines[6].split(' '); // WHAT: "signature", the key's name, the hex. WHY: provisional form, above
    if signature.next() != Some("signature") {
        return None;
    }
    let (name, hex) = (signature.next()?, signature.next()?);
    let statement = lines[..6].iter().fold(String::new(), |mut all, line| {
        all.push_str(line); // WHAT: the line. WHY: as signed
        all.push('\n'); // WHAT: its end. WHY: every line ended by a line feed
        all
    }); // WHAT: the six lines, each ended. WHY: what was signed
    let public = keys.iter().find(|(n, _)| *n == name).and_then(|(_, k)| crate::update::unhex(k))?; // WHAT: the named key, built in. WHY: only a key the app carries counts
    let public: [u8; 32] = public.try_into().ok()?;
    let signature: [u8; 64] = crate::update::unhex(hex)?.try_into().ok()?;
    let key = ed25519_dalek::VerifyingKey::from_bytes(&public).ok()?;
    key.verify_strict(statement.as_bytes(), &ed25519_dalek::Signature::from_bytes(&signature)).ok()?; // WHAT: the signature. WHY: only then is it verified
    Some(Certificate { company, license, pin, citadel_pin: (citadel != "none").then_some(citadel), issued })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_is_private_ipv4_fc00_fe80_and_loopback_only() {
        for local in ["10.0.0.5", "172.16.3.4", "172.31.255.1", "192.168.1.44", "127.0.0.1", "::1", "fd12::1", "fc00::5", "fe80::1", "::ffff:192.168.1.2"] {
            assert!(is_local(local.parse().unwrap()), "{local}");
        }
        for outside in ["8.8.8.8", "172.32.0.1", "100.64.0.1", "2001:db8::1", "::ffff:8.8.8.8"] {
            assert!(!is_local(outside.parse().unwrap()), "{outside}");
        }
        assert!(host_is_local("[fe80::1]", 8714));
        assert!(!host_is_local("no-such-host.invalid", 8714), "a name that resolves to nothing is not local");
    }

    #[test]
    fn an_ipad_keeps_the_shared_record_in_application_support_and_the_rest_are_unchanged() {
        let home = PathBuf::from("/var/mobile/Containers/Data/Application/APP"); // WHAT: an iOS app's container, as $HOME. WHY: the iPad case
        let homes = Homes { local_app_data: Some(PathBuf::from(r"C:\Users\sam\AppData\Local")), xdg_config: None, home: Some(home.clone()) }; // WHAT: every variable set. WHY: each platform takes its own
        assert_eq!(shared_file_for(&homes, Platform::Ios), Some(home.join("Library/Application Support/Excalibur/office.json"))); // WHAT: Library/Application Support. WHY: an iOS app can't make .config at its container's top
        assert_eq!(shared_file_for(&homes, Platform::Unix), Some(home.join(".config/Excalibur/office.json"))); // WHAT: ~/.config. WHY: unchanged
        assert_eq!(shared_file_for(&Homes { xdg_config: Some(PathBuf::from("/x")), ..Homes::default() }, Platform::Unix), Some(PathBuf::from("/x/Excalibur/office.json"))); // WHAT: $XDG_CONFIG_HOME first. WHY: unchanged, and how Android finds it
        assert_eq!(shared_file_for(&homes, Platform::Windows), Some(PathBuf::from(r"C:\Users\sam\AppData\Local").join("Excalibur").join("office.json"))); // WHAT: %LocalAppData%. WHY: unchanged
        assert_eq!(shared_file_for(&Homes::default(), Platform::Ios), None); // WHAT: no home. WHY: nowhere, rather than a guess
    }

    #[test]
    fn a_record_kept_by_an_earlier_ipad_build_is_moved_once_and_never_over_a_newer_one() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(); // WHAT: a fresh name. WHY: tests run side by side
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("office-move-{n}-{}", std::process::id())); // WHAT: under the build's own folder. WHY: never a shared temporary folder
        let homes = Homes { home: Some(base.clone()), ..Homes::default() }; // WHAT: this folder as $HOME. WHY: both places under it
        let old = shared_file_for(&homes, Platform::Unix).unwrap(); // WHAT: where 0.7.0 kept it on iOS. WHY: the old place
        let new = shared_file_for(&homes, Platform::Ios).unwrap(); // WHAT: where it goes now. WHY: the new place
        assert!(!move_once(&old, &new).unwrap(), "nothing to move"); // WHAT: no old record. WHY: a fresh install
        let kept = Shared { address: "https://192.168.1.10:8714".into(), pin: "a".repeat(64), verified: true, other: serde_json::Map::new() }; // WHAT: an old record, verified. WHY: what must survive
        write(&old, &kept).unwrap(); // WHAT: written in the old place. WHY: as 0.7.0 tried to
        let newer = Shared { pin: "b".repeat(64), verified: false, ..kept.clone() }; // WHAT: a newer record already in the new place. WHY: must never be overwritten, even with no check before the link
        write(&new, &newer).unwrap(); // WHAT: written. WHY: as an app linking later would
        assert!(!move_once(&old, &new).unwrap(), "a newer record wins"); // WHAT: not moved. WHY: the hard link refuses an existing target
        assert_eq!(read(&new), Ok(Some(newer)), "the newer record is untouched"); // WHAT: unchanged. WHY: an older pin never replaces a newer one
        assert!(old.exists(), "the old record stays when it wasn't moved"); // WHAT: kept. WHY: nothing lost
        std::fs::remove_file(&new).unwrap(); // WHAT: the new place empty again. WHY: the move itself, next
        assert!(move_once(&old, &new).unwrap(), "moved"); // WHAT: moved. WHY: nothing in the new place
        assert_eq!(read(&new), Ok(Some(kept)), "the same pin and verified flag"); // WHAT: read back. WHY: the iPad stays linked to the same server
        assert!(!old.exists(), "the old record is removed once moved"); // WHAT: gone. WHY: security review L3: no stale pin left behind
        let left: Vec<_> = std::fs::read_dir(new.parent().unwrap()).unwrap().flatten().map(|e| e.file_name()).collect(); // WHAT: what is in the new folder. WHY: no temporary left over
        assert_eq!(left, vec![std::ffi::OsString::from("office.json")], "only the record, no temporary file"); // WHAT: just the record. WHY: tidy
        let _ = std::fs::remove_dir_all(&base); // WHAT: tidied. WHY: a test leaves nothing
    }

    #[cfg(unix)]
    #[test]
    fn links_are_never_followed_at_either_end_of_a_move() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(); // WHAT: a fresh name. WHY: tests run side by side
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("office-links-{n}-{}", std::process::id())); // WHAT: under the build's own folder. WHY: never a shared temporary folder
        std::fs::create_dir_all(&base).unwrap(); // WHAT: made. WHY: the files go in it
        let (old, new, elsewhere) = (base.join("old.json"), base.join("new").join("office.json"), base.join("elsewhere.json")); // WHAT: the two places and a file a link could point at. WHY: the cases below
        std::fs::write(&elsewhere, b"someone else's file").unwrap(); // WHAT: a file that isn't the record. WHY: must never be read or written through a link
        std::os::unix::fs::symlink(&elsewhere, &old).unwrap(); // WHAT: the old place is a link. WHY: never followed
        assert!(!move_once(&old, &new).unwrap(), "a link as the source is not read"); // WHAT: refused. WHY: O_NOFOLLOW
        assert!(!new.exists() && old.symlink_metadata().is_ok(), "nothing written, the link left as it was"); // WHAT: nothing done. WHY: likewise
        std::fs::remove_file(&old).unwrap(); // WHAT: the link gone. WHY: a real old record next
        std::fs::write(&old, b"{}").unwrap(); // WHAT: a real old record. WHY: to be moved
        std::fs::create_dir_all(new.parent().unwrap()).unwrap(); // WHAT: the new folder. WHY: a link goes in it
        std::os::unix::fs::symlink(base.join("nowhere.json"), &new).unwrap(); // WHAT: a link to nothing at the new place. WHY: a write through it would land elsewhere
        assert!(!move_once(&old, &new).unwrap(), "a link to nothing still counts as something there"); // WHAT: refused. WHY: the hard link never goes through a link
        assert!(!base.join("nowhere.json").exists(), "nothing written through the link"); // WHAT: nothing created. WHY: create_new and the hard link never follow it
        assert_eq!(std::fs::read(&elsewhere).unwrap(), b"someone else's file"); // WHAT: untouched. WHY: never read or written
        let _ = std::fs::remove_dir_all(&base); // WHAT: tidied. WHY: a test leaves nothing
    }

    #[test]
    fn the_shared_file_reads_back_and_a_damaged_one_is_never_empty() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("office-{n}.json"));
        assert_eq!(read(&path), Ok(None));
        let shared = Shared { address: "https://192.168.1.10:8714".into(), pin: "a".repeat(64), verified: false, other: serde_json::Map::new() }; // WHAT: a record with no other fields. WHY: the plain case first
        write(&path, &shared).unwrap();
        assert_eq!(read(&path), Ok(Some(shared)));
        std::fs::write(&path, format!("{{\"address\":\"https://192.168.1.10:8714\",\"pin\":\"{}\",\"verified\":true,\"seats\":[1,2],\"by\":\"view\"}}", "b".repeat(64))).unwrap(); // WHAT: a record another app wrote, with fields of its own. WHY: they must survive this app's write
        let mut theirs = read(&path).unwrap().expect("it reads"); // WHAT: read with its extra fields. WHY: kept in `other`
        assert_eq!(theirs.other.get("by"), Some(&serde_json::json!("view")), "another app's field is read");
        theirs.verified = false; // WHAT: one field changed. WHY: as an app would
        write(&path, &theirs).unwrap(); // WHAT: written back. WHY: the extra fields go with it
        let again: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap(); // WHAT: the file as JSON. WHY: checked field by field
        assert_eq!((again["seats"].clone(), again["by"].clone(), again["verified"].clone()), (serde_json::json!([1, 2]), serde_json::json!("view"), serde_json::json!(false)), "other fields kept, ours changed");
        std::fs::write(&path, b"{\"address\":").unwrap();
        assert!(read(&path).is_err(), "damaged is said, never read as nothing");
        let _ = std::fs::remove_file(&path);
        assert_eq!(server_of("https://Office.:8714/"), Some("office:8714".into()));
    }

    #[test]
    fn a_certificate_verifies_only_with_a_built_in_key() {
        use ed25519_dalek::Signer;
        let key = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let public = crate::update::hex(&key.verifying_key().to_bytes());
        let statement = format!("excalibur-office-cert/1\ncompany LatCon\nlicense lic_1\npin {}\ncitadel-pin none\nissued 2026-11-11\n", "b".repeat(64));
        let text = format!("{statement}signature office-2026 {}\n", crate::update::hex(&key.sign(statement.as_bytes()).to_bytes()));
        let checked = check_certificate(&text, &[("office-2026", &public)]).expect("verifies");
        assert_eq!(checked.pin, "b".repeat(64));
        assert_eq!(checked.citadel_pin, None);
        assert!(check_certificate(&text, &[]).is_none(), "no built-in key, never verified");
        assert!(check_certificate(&text.replace("LatCon", "Other"), &[("office-2026", &public)]).is_none(), "changed, never verified");
    }

    #[test]
    fn an_oversized_shared_record_is_refused_by_its_size() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(); // WHAT: a fresh name. WHY: tests run side by side
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("big-office-{n}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut big = format!("{{\"address\":\"https://192.168.1.10:8714\",\"pin\":\"{}\",\"verified\":false}}", "a".repeat(64)).into_bytes(); // WHAT: a good record. WHY: only its size is wrong
        big.extend(vec![b' '; 20 * 1024]); // WHAT: padded past 16 KiB with spaces JSON allows. WHY: refused by its size, never parsed
        std::fs::write(&path, big).unwrap();
        assert!(read(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
