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
/// earlier build kept under `$HOME/.config` is copied across once.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn shared_file() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from); // WHAT: one environment variable, as a path. WHY: read the same way each time
    let here = shared_file_for(&Homes { local_app_data: var("LOCALAPPDATA"), xdg_config: var("XDG_CONFIG_HOME"), home: var("HOME") }, Platform::this())?; // WHAT: this system's place for it. WHY: §2.3, and Library/Application Support on iOS
    if Platform::this() == Platform::Ios {
        if let Some(old) = shared_file_for(&Homes { local_app_data: None, xdg_config: None, home: var("HOME") }, Platform::Unix) {
            match copied_once(&old, &here) {
                Ok(true) => eprintln!("the office server's shared record was copied from {} to {}", old.display(), here.display()), // WHAT: noted in the log. WHY: the move can be seen
                Ok(false) => {} // WHAT: nothing to copy. WHY: a new install, or copied before
                Err(e) => eprintln!("the office server's shared record couldn't be copied from {}: {e}", old.display()), // WHAT: said. WHY: the app still connects by its own pin book
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

/// Copies `old` to `new` once: only when `old` is a plain file and `new`
/// isn't there. The old file is left where it is. True when it copied.
///
/// # Errors
///
/// When the new folder can't be made or the copy fails.
fn copied_once(old: &Path, new: &Path) -> std::io::Result<bool> {
    let plain = std::fs::symlink_metadata(old).is_ok_and(|m| m.is_file()); // WHAT: a real file, not a link or a folder. WHY: a pin record is only ever taken from a file of our own
    if !plain || std::fs::symlink_metadata(new).is_ok() {
        return Ok(false); // WHAT: nothing to do. WHY: never over a newer record (a pin or a verified flag is never replaced by an older one), and nothing to copy from
    }
    if let Some(dir) = new.parent() {
        std::fs::create_dir_all(dir)?; // WHAT: the new folder. WHY: a fresh install has none yet
    }
    std::fs::copy(old, new)?; // WHAT: the old record, pin and all. WHY: the app stays linked to the same server after the upgrade
    Ok(true)
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
    fn a_record_kept_by_an_earlier_ipad_build_is_copied_once_and_never_over_a_newer_one() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(); // WHAT: a fresh name. WHY: tests run side by side
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("office-move-{n}-{}", std::process::id())); // WHAT: under the build's own folder. WHY: never a shared temporary folder
        let homes = Homes { home: Some(base.clone()), ..Homes::default() }; // WHAT: this folder as $HOME. WHY: both places under it
        let old = shared_file_for(&homes, Platform::Unix).unwrap(); // WHAT: where 0.7.0 kept it on iOS. WHY: the old place
        let new = shared_file_for(&homes, Platform::Ios).unwrap(); // WHAT: where it goes now. WHY: the new place
        assert!(!copied_once(&old, &new).unwrap(), "nothing to copy"); // WHAT: no old record. WHY: a fresh install
        let kept = Shared { address: "https://192.168.1.10:8714".into(), pin: "a".repeat(64), verified: true, other: serde_json::Map::new() }; // WHAT: an old record, verified. WHY: what must survive
        write(&old, &kept).unwrap(); // WHAT: written in the old place. WHY: as 0.7.0 tried to
        assert!(copied_once(&old, &new).unwrap(), "copied"); // WHAT: copied. WHY: none in the new place
        assert_eq!(read(&new), Ok(Some(kept.clone())), "the same pin and verified flag"); // WHAT: read back. WHY: the iPad stays linked to the same server
        assert!(old.exists(), "the old record is left alone"); // WHAT: not deleted. WHY: nothing is deleted
        let newer = Shared { pin: "b".repeat(64), verified: false, ..kept.clone() }; // WHAT: a newer record in the new place. WHY: must never be overwritten
        write(&new, &newer).unwrap(); // WHAT: written. WHY: as an app linking later would
        assert!(!copied_once(&old, &new).unwrap(), "once"); // WHAT: not copied again. WHY: the newer one wins
        assert_eq!(read(&new), Ok(Some(newer)), "the newer record is untouched"); // WHAT: unchanged. WHY: an older pin never replaces a newer one
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
