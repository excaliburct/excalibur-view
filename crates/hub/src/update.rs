//! Updates.
//!
//! One place publishes, everybody pulls from their own server, and nothing
//! installs that was not signed by the key the publisher holds.
//!
//! The shape of it:
//!
//! 1. A tag on the private repository builds the installer and signs a
//!    [`Release`] describing it. The signing key never leaves the publisher.
//! 2. A company's own Hyperview server offers that release at
//!    `/api/v1/update/latest`, either mirroring the published feed or serving
//!    a file an administrator uploaded by hand — a shop with no internet on
//!    the floor can still be updated.
//! 3. A seat checks on start, downloads in the background, checks the digest
//!    **and** the signature against a key compiled into the running program,
//!    and installs on the next launch. Never in the middle of a session:
//!    somebody three hours into a takeoff does not get restarted.
//!
//! Two things a shop needs that a consumer app does not:
//!
//! - **Pinning.** An administrator can hold every seat on one version. A
//!   fabricator in the middle of a bid package should not have the tool that
//!   priced the first half behave differently on the second.
//! - **Staging.** A release is not offered until it is marked ready, so one
//!   seat can try it before the other five get it.

use serde::{Deserialize, Serialize};

/// Which stream of releases a seat follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// What the office runs.
    #[default]
    Stable,
    /// What the person testing the next one runs.
    Preview,
}

/// A published build.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Release {
    /// `1.4.2`.
    pub version: String,
    pub channel: Channel,
    /// `windows-x64`, `linux-x64`.
    pub platform: String,
    /// When it was published, RFC 3339.
    pub published: String,
    /// What changed, in words the office will read.
    pub notes: String,
    /// Where the installer is. Relative to the server it came from, or
    /// absolute if the administrator pointed somewhere else.
    pub download: String,
    pub bytes: u64,
    /// SHA-256 of the installer, lower-case hex.
    pub digest: String,
    /// Ed25519 over the signing payload below, lower-case hex. Verified
    /// against a public key compiled into the running program.
    pub signature: String,
    /// Which public key signed it, so a key can be rotated without every old
    /// build refusing every new release.
    pub key: String,
    /// A seat older than this cannot talk to a server of this generation and
    /// must update before it can carry on.
    #[serde(default)]
    pub minimum_api_version: u32,
}

impl Release {
    /// Exactly the bytes that get signed.
    ///
    /// Spelled out field by field rather than signing the JSON, because two
    /// JSON encoders will not agree on key order or number formatting, and a
    /// signature that depends on how something was printed is not a signature.
    pub fn signing_payload(&self) -> Vec<u8> {
        let text = format!(
            "hyperview-release-v1\n\
             version={}\n\
             channel={}\n\
             platform={}\n\
             bytes={}\n\
             digest={}\n\
             minimum_api_version={}\n",
            self.version,
            match self.channel {
                Channel::Stable => "stable",
                Channel::Preview => "preview",
            },
            self.platform,
            self.bytes,
            self.digest.to_lowercase(),
            self.minimum_api_version,
        );
        text.into_bytes()
    }
}

/// What a server answers when a seat asks whether there is anything new.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateOffer {
    /// Absent means there is nothing newer, or the administrator has pinned
    /// this office to what it is already running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<Release>,
    /// Set when an administrator has held this installation on one version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_to: Option<String>,
    /// True when the seat is too old to talk to this server at all. It is not
    /// a suggestion any more.
    #[serde(default)]
    pub required: bool,
    /// True when the server was still looking at the publisher's feed when it
    /// answered, so "nothing newer" means "nothing newer yet". A seat that
    /// asked on somebody's behalf asks again shortly rather than telling them
    /// they are up to date.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub looking: bool,
}

/// Compares two versions the way a person reads them: 1.10.0 is newer than
/// 1.9.9. Anything unparseable sorts as older, so a broken version string can
/// never present itself as an upgrade.
pub fn newer(candidate: &str, running: &str) -> bool {
    match (parse(candidate), parse(running)) {
        (Some(a), Some(b)) => a > b,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Orders two versions. Anything unparseable sorts below everything else, so a
/// release with a broken version string can never win a comparison.
pub fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    match (parse(a), parse(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.trim().parse().ok()?;
    let minor = parts.next().unwrap_or("0").trim().parse().ok()?;
    let patch = parts.next().unwrap_or("0").trim().parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// What the running program will accept.
///
/// The public keys are compiled in. A release signed by a key that is not in
/// here does not install, whoever is serving it, which is what stops a
/// compromised or merely misconfigured server from pushing anything it likes
/// onto six machines.
#[derive(Clone, Debug)]
pub struct Trusted {
    pub keys: Vec<(String, [u8; 32])>,
}

impl Trusted {
    /// The list a build was compiled with, as `(name, 64 hex characters)`.
    /// Anything malformed is left out rather than trusted, so a typo in the
    /// list can only ever make a build refuse more, never accept more.
    pub fn pinned(list: &[(&str, &str)]) -> Trusted {
        let keys = list
            .iter()
            .filter_map(|(name, text)| {
                let bytes = unhex(text)?;
                let key = <[u8; 32]>::try_from(bytes.as_slice()).ok()?;
                Some((name.to_string(), key))
            })
            .collect();
        Trusted { keys }
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn key(&self, named: &str) -> Option<&[u8; 32]> {
        self.keys.iter().find(|(n, _)| n == named).map(|(_, k)| k)
    }

    /// Checks a release against the keys this build trusts and the bytes that
    /// actually arrived.
    ///
    /// Both halves matter. The digest says the download is whole; the
    /// signature says the publisher meant to publish it. A download that
    /// matches its digest and nothing else proves only that whoever wrote the
    /// digest also wrote the file.
    pub fn check(&self, release: &Release, installer: &[u8]) -> Result<(), Refusal> {
        use sha2::{Digest, Sha256};

        let Some(key) = self.key(&release.key) else {
            return Err(Refusal::UnknownKey(release.key.clone()));
        };
        let digest = hex(&Sha256::digest(installer));
        if digest != release.digest.to_lowercase() {
            return Err(Refusal::WrongDigest {
                expected: release.digest.to_lowercase(),
                found: digest,
            });
        }
        if installer.len() as u64 != release.bytes {
            return Err(Refusal::WrongSize {
                expected: release.bytes,
                found: installer.len() as u64,
            });
        }
        let signature = unhex(&release.signature).ok_or(Refusal::MalformedSignature)?;
        verify(key, &release.signing_payload(), &signature)?;
        Ok(())
    }
}

/// Why an update was not installed. Every one of these is said out loud rather
/// than retried quietly: a seat that will not update needs somebody to know.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    UnknownKey(String),
    WrongDigest { expected: String, found: String },
    WrongSize { expected: u64, found: u64 },
    MalformedSignature,
    BadSignature,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::UnknownKey(name) => write!(
                f,
                "That update is signed with a key this program does not know ({name}). \
                 It has not been installed."
            ),
            Refusal::WrongDigest { expected, found } => write!(
                f,
                "That update did not arrive whole: expected {expected}, got {found}. \
                 It has not been installed."
            ),
            Refusal::WrongSize { expected, found } => write!(
                f,
                "That update is the wrong size: expected {expected} bytes, got {found}. \
                 It has not been installed."
            ),
            Refusal::MalformedSignature => {
                write!(f, "That update's signature is not readable. It has not been installed.")
            }
            Refusal::BadSignature => write!(
                f,
                "That update's signature does not match its contents. It has not been \
                 installed, and somebody should be told."
            ),
        }
    }
}

impl std::error::Error for Refusal {}

// ---- putting a checked build in place ------------------------------------

/// Puts a new copy of a program where the running one is, so the next start
/// runs it.
///
/// Windows will not let a running program's file be written or deleted, but it
/// will let it be *renamed*. So the running file steps aside to `.old`, the new
/// one takes its name, and the process that is running carries on from the
/// renamed file untouched until it exits. Nobody is restarted in the middle of
/// anything; the next start simply is the new version.
///
/// Only ever called with bytes that [`Trusted::check`] has already passed.
/// If the second rename fails the first is undone, so a failed update leaves
/// the program exactly as it was rather than leaving no program at all.
pub fn swap_in(program: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let fresh = program.with_extension("new");
    let old = program.with_extension("old");
    std::fs::write(&fresh, bytes)?;
    // On Linux (and anywhere else files carry permissions) the new build is a
    // program the moment it takes the old one's name, as the old one was.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(program).map(|m| m.permissions().mode()).unwrap_or(0o755) | 0o700;
        std::fs::set_permissions(&fresh, std::fs::Permissions::from_mode(mode))?;
    }
    // A leftover from the last update. If an older copy is somehow still
    // running from it this fails, and the rename below replaces it instead.
    let _ = std::fs::remove_file(&old);
    std::fs::rename(program, &old)?;
    if let Err(e) = std::fs::rename(&fresh, program) {
        let _ = std::fs::rename(&old, program);
        let _ = std::fs::remove_file(&fresh);
        return Err(e);
    }
    Ok(())
}

/// Tidies up after [`swap_in`], once the new version is the one running.
pub fn tidy_after_update(program: &std::path::Path) {
    let _ = std::fs::remove_file(program.with_extension("old"));
    let _ = std::fs::remove_file(program.with_extension("new"));
}

// ---- the same thing on a Mac ------------------------------------------------
//
// Only a Mac calls any of this, but all of it is built everywhere, because
// none of it needs macOS to compile -- `ditto`, `codesign` and `xattr` are
// names of programs, not system calls. Built everywhere, it can be tested
// everywhere, and the parts that decide whether a download is fit to install
// are exactly the parts worth testing.
//
// A Mac program is not a file. It is a folder -- `Excalibur View.app` -- with
// a signature over everything in it, symbolic links inside it that have to
// survive being copied, and a quarantine mark macOS puts on anything that came
// off the internet. Writing over the executable inside it, the way
// [`swap_in`] does on Windows, breaks the signature and macOS then refuses to
// open it at all. So the whole folder is replaced at once.
//
// Three of macOS's own tools do the parts that must be done exactly right:
//
// - `ditto` unpacks the zip. An ordinary unzip flattens the symbolic links a
//   bundle keeps in `Contents/Frameworks` and strips extended attributes, and
//   either one invalidates the signature.
// - `codesign` says whether the unpacked bundle is really signed and whole.
//   We have already checked it against our own key by this point; this is
//   macOS's own opinion, asked separately, so a bundle that would not open
//   is found here rather than after it has replaced the working one.
// - `xattr` takes the quarantine mark off, which is what otherwise produces
//   "downloaded from the internet, are you sure" on a version the person did
//   not download and never saw arrive.

/// Where a bundle's pieces are put while it is being unpacked and checked.
fn beside(app: &std::path::Path, what: &str) -> std::path::PathBuf {
    let name = app.file_name().and_then(|n| n.to_str()).unwrap_or("app");
    let folder = app.parent().unwrap_or(std::path::Path::new("."));
    folder.join(format!(".{name}.{what}"))
}

/// Replaces an installed `.app` with the one in `zipped`, so the next start
/// runs it.
///
/// `zipped` is a zip of the bundle as `ditto` makes one -- which is what the
/// release carries for exactly this reason. The caller has already checked it
/// against the publisher's key; nothing here is a substitute for that.
///
/// Everything happens in the folder the app is already in, so the final step
/// is a rename within one filesystem rather than a copy that can half-finish.
/// If that step fails the old bundle is put back.
pub fn swap_bundle_in(app: &std::path::Path, zipped: &[u8]) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};

    let folder = app
        .parent()
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "that is not an installed program"))?;

    // The common reason an update cannot be put in place: the app is in
    // /Applications and was put there by somebody else, or by an installer
    // running as an administrator. Say so now, rather than after downloading.
    if !writable(folder) {
        return Err(Error::new(
            ErrorKind::PermissionDenied,
            format!(
                "{} cannot be written to, so this copy cannot replace itself. \
                 Download the new version and drag it in.",
                folder.display()
            ),
        ));
    }

    let work = beside(app, "update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;

    let result = (|| {
        let archive = work.join("download.zip");
        std::fs::write(&archive, zipped)?;

        let unpacked = work.join("unpacked");
        run("/usr/bin/ditto", &["-x", "-k", path(&archive)?, path(&unpacked)?])?;

        let fresh = only_bundle(&unpacked)?;
        run("/usr/bin/codesign", &["--verify", "--strict", "--deep", path(&fresh)?])?;
        // Not being able to take the quarantine mark off is not a reason to
        // stop: there may not be one.
        let _ = run("/usr/bin/xattr", &["-dr", "com.apple.quarantine", path(&fresh)?]);

        // Into the folder the app lives in, so the swap below is a rename.
        let staged = beside(app, "staged");
        let _ = std::fs::remove_dir_all(&staged);
        std::fs::rename(&fresh, &staged)?;
        exchange(app, &staged)
    })();

    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Puts `fresh` where `app` is, and the old `app` aside.
///
/// Both are in the same folder by this point, so each step is a rename within
/// one filesystem: it either happens or it does not, and there is no state in
/// between where the person has half a program. If the second rename fails
/// anyway, what was working goes back before the error is returned — the one
/// outcome that must never happen here is a computer left with no program at
/// all because an update went wrong.
fn exchange(app: &std::path::Path, fresh: &std::path::Path) -> std::io::Result<()> {
    let previous = beside(app, "previous");
    let _ = std::fs::remove_dir_all(&previous);
    std::fs::rename(app, &previous)?;
    if let Err(e) = std::fs::rename(fresh, app) {
        let _ = std::fs::rename(&previous, app);
        return Err(e);
    }
    Ok(())
}

/// Takes away what [`swap_bundle_in`] left behind, once the new version is
/// the one running.
pub fn tidy_after_bundle_update(app: &std::path::Path) {
    for what in ["previous", "staged", "update"] {
        let _ = std::fs::remove_dir_all(beside(app, what));
    }
}

fn writable(folder: &std::path::Path) -> bool {
    let probe = folder.join(".excalibur-write-test");
    let ok = std::fs::write(&probe, b"x").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// The one `.app` a freshly unpacked release holds. More than one, or none,
/// means the archive is not what it claims to be, so nothing is replaced.
fn only_bundle(folder: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    use std::io::{Error, ErrorKind};
    let mut found: Vec<std::path::PathBuf> = std::fs::read_dir(folder)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .collect();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(Error::new(ErrorKind::NotFound, "that download holds no program")),
        n => Err(Error::new(
            ErrorKind::InvalidData,
            format!("that download holds {n} programs, so none of them was used"),
        )),
    }
}

fn path(p: &std::path::Path) -> std::io::Result<&str> {
    p.to_str().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{} is not a usable name", p.display()))
    })
}

/// Runs one of macOS's own tools, and turns "it said no" into an error that
/// says which tool and what it said.
fn run(tool: &str, args: &[&str]) -> std::io::Result<()> {
    let out = std::process::Command::new(tool).args(args).output()?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    let said = said.trim();
    let name = tool.rsplit('/').next().unwrap_or(tool);
    Err(std::io::Error::other(if said.is_empty() {
        format!("{name} would not do it")
    } else {
        format!("{name}: {said}")
    }))
}

/// The version written into a program file, read without running it.
///
/// Each program carries a line made by [`build_mark!`]. Reading it off the file
/// is how the server knows the copy already installed is older than the one
/// somebody just double-clicked, and how a seat knows not to install itself
/// over a newer copy.
pub fn version_in_file(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    version_in_bytes(&bytes)
}

pub fn version_in_bytes(bytes: &[u8]) -> Option<String> {
    // Built from two halves so the needle itself is not a complete mark
    // sitting in this program's own read-only data.
    let needle = [MARK_HEAD, MARK_TAIL].concat();
    let needle = needle.as_bytes();
    let mut from = 0;
    while let Some(at) = find(&bytes[from..], needle) {
        let start = from + at + needle.len();
        let rest = &bytes[start..bytes.len().min(start + 32)];
        if let Some(end) = rest.iter().position(|b| *b == 0x02) {
            let text = std::str::from_utf8(&rest[..end]).ok()?;
            if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit() || c == '.') {
                return Some(text.to_string());
            }
        }
        from = start;
    }
    None
}

#[doc(hidden)]
pub const MARK_HEAD: &str = "\x01hyperview-build";
#[doc(hidden)]
pub const MARK_TAIL: &str = "-version=";

/// A program's version, in a form [`version_in_file`] can read back off the
/// file. Use it once per program and keep it alive with `std::hint::black_box`.
#[macro_export]
macro_rules! build_mark {
    () => {
        concat!("\x01hyperview-build-version=", env!("CARGO_PKG_VERSION"), "\x02")
    };
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

/// Ed25519 verification.
///
/// Deliberately the only cryptography in this crate, and it is a check rather
/// than a construction: nothing here invents a scheme, chooses a nonce, or
/// holds a secret.
pub(crate) fn verify(key: &[u8; 32], message: &[u8], signature: &[u8]) -> Result<(), Refusal> {
    if signature.len() != 64 {
        return Err(Refusal::MalformedSignature);
    }
    let key = ed25519_dalek::VerifyingKey::from_bytes(key).map_err(|_| Refusal::MalformedSignature)?;
    let mut bytes = [0u8; 64];
    bytes.copy_from_slice(signature);
    let signature = ed25519_dalek::Signature::from_bytes(&bytes);
    key.verify_strict(message, &signature)
        .map_err(|_| Refusal::BadSignature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn a_publisher() -> (SigningKey, Trusted) {
        // A fixed key, so the test is the same every time it runs. A real
        // signing key is generated by the publisher on their own machine and
        // never appears in a repository.
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let trusted = Trusted {
            keys: vec![("mesafab-2026".to_string(), signing.verifying_key().to_bytes())],
        };
        (signing, trusted)
    }

    fn a_release(installer: &[u8], signing: &SigningKey) -> Release {
        use sha2::{Digest, Sha256};
        let mut release = Release {
            version: "1.4.2".into(),
            channel: Channel::Stable,
            platform: "windows-x64".into(),
            published: "2026-09-18T12:00:00Z".into(),
            notes: "Dynamic Fill.".into(),
            download: "/api/v1/update/1.4.2/download".into(),
            bytes: installer.len() as u64,
            digest: hex(&Sha256::digest(installer)),
            signature: String::new(),
            key: "mesafab-2026".into(),
            minimum_api_version: 1,
        };
        release.signature = hex(&signing.sign(&release.signing_payload()).to_bytes());
        release
    }

    #[test]
    fn a_version_can_be_read_off_a_program_file() {
        let mut file = b"MZ....some code....".to_vec();
        file.extend_from_slice(build_mark!().as_bytes());
        file.extend_from_slice(b"...more code...");
        assert_eq!(version_in_bytes(&file).as_deref(), Some(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn a_file_with_no_mark_has_no_version_rather_than_a_guessed_one() {
        assert_eq!(version_in_bytes(b"MZ nothing to see here"), None);
        // The needle on its own, with junk after it, is not a version.
        let mut file = [MARK_HEAD, MARK_TAIL].concat().into_bytes();
        file.extend_from_slice(b"abc\x02");
        assert_eq!(version_in_bytes(&file), None);
    }

    #[test]
    fn a_new_build_takes_the_old_ones_name_and_the_old_one_steps_aside() {
        let dir = std::env::temp_dir().join(format!("hv-swap-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let program = dir.join("Hyperview.exe");
        std::fs::write(&program, b"old build").unwrap();
        swap_in(&program, b"new build").unwrap();
        assert_eq!(std::fs::read(&program).unwrap(), b"new build");
        assert_eq!(std::fs::read(program.with_extension("old")).unwrap(), b"old build");
        // And again, with last time's leftover still there.
        swap_in(&program, b"newer build").unwrap();
        assert_eq!(std::fs::read(&program).unwrap(), b"newer build");
        tidy_after_update(&program);
        assert!(!program.with_extension("old").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_build_swapped_in_on_linux_is_still_a_program() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("hv-swap-mode-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let program = dir.join("hyperview");
        std::fs::write(&program, b"old build").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        swap_in(&program, b"new build").unwrap();
        assert_eq!(std::fs::read(&program).unwrap(), b"new build");
        assert_eq!(std::fs::metadata(&program).unwrap().permissions().mode() & 0o777, 0o755);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_pinned_key_is_left_out_rather_than_trusted() {
        let trusted = Trusted::pinned(&[
            ("good", "14d93acf4729859a3a7b01f458e118173a7a1859305193e84c3bd7513eb72790"),
            ("short", "14d93acf"),
            ("nonsense", "not hex at all"),
        ]);
        assert_eq!(trusted.keys.len(), 1);
        assert!(trusted.key("good").is_some());
    }

    #[test]
    fn a_properly_signed_release_is_accepted() {
        let (signing, trusted) = a_publisher();
        let installer = b"pretend this is an installer".to_vec();
        let release = a_release(&installer, &signing);
        assert_eq!(trusted.check(&release, &installer), Ok(()));
    }

    #[test]
    fn an_installer_that_was_tampered_with_is_refused() {
        let (signing, trusted) = a_publisher();
        let installer = b"pretend this is an installer".to_vec();
        let release = a_release(&installer, &signing);
        let meddled = b"pretend this is an installer!".to_vec();
        match trusted.check(&release, &meddled) {
            Err(Refusal::WrongDigest { .. }) => {}
            other => panic!("it must refuse: {other:?}"),
        }
    }

    #[test]
    fn a_release_signed_by_somebody_else_is_refused() {
        let (_, trusted) = a_publisher();
        let impostor = SigningKey::from_bytes(&[9u8; 32]);
        let installer = b"a perfectly good installer".to_vec();
        let mut release = a_release(&installer, &impostor);
        // They claim to be us, and the digest is honest. Only the signature
        // gives them away, which is the whole point of having one.
        release.key = "mesafab-2026".into();
        assert_eq!(
            trusted.check(&release, &installer),
            Err(Refusal::BadSignature)
        );
    }

    #[test]
    fn a_release_signed_with_a_key_we_do_not_know_is_refused() {
        let (_, trusted) = a_publisher();
        let other = SigningKey::from_bytes(&[3u8; 32]);
        let installer = b"who knows".to_vec();
        let mut release = a_release(&installer, &other);
        release.key = "somebody-elses-key".into();
        assert!(matches!(
            trusted.check(&release, &installer),
            Err(Refusal::UnknownKey(_))
        ));
    }

    #[test]
    fn changing_the_version_after_signing_is_caught() {
        let (signing, trusted) = a_publisher();
        let installer = b"an installer".to_vec();
        let mut release = a_release(&installer, &signing);
        release.version = "9.9.9".into();
        assert_eq!(
            trusted.check(&release, &installer),
            Err(Refusal::BadSignature)
        );
    }

    #[test]
    fn versions_compare_the_way_people_read_them() {
        assert!(newer("1.10.0", "1.9.9"));
        assert!(newer("2.0.0", "1.99.99"));
        assert!(!newer("1.4.2", "1.4.2"));
        assert!(!newer("1.4.1", "1.4.2"));
    }

    #[test]
    fn releases_sort_by_version_and_not_by_the_day_they_were_published() {
        let mut versions = vec!["1.5.0", "1.6.0", "1.10.0", "1.9.9", "nonsense"];
        versions.sort_by(|a, b| compare(b, a));
        assert_eq!(versions[0], "1.10.0", "ten is after nine, not before it");
        assert_eq!(*versions.last().unwrap(), "nonsense", "and rubbish sorts last");
    }

    #[test]
    fn a_version_string_that_makes_no_sense_never_looks_like_an_upgrade() {
        assert!(!newer("latest", "1.4.2"));
        assert!(!newer("", "1.4.2"));
        assert!(!newer("1.2.3.4", "1.4.2"));
        assert!(!newer("../../etc/passwd", "1.4.2"));
    }

    #[test]
    fn the_signing_payload_does_not_depend_on_how_json_was_printed() {
        let (signing, _) = a_publisher();
        let installer = b"an installer".to_vec();
        let release = a_release(&installer, &signing);
        let payload = String::from_utf8(release.signing_payload()).unwrap();
        assert!(payload.starts_with("hyperview-release-v1\n"));
        assert!(payload.contains("version=1.4.2\n"));
        // Nothing that is only presentation is signed.
        assert!(!payload.contains("Dynamic Fill"));
        assert!(!payload.contains("2026-09-18"));
    }

    #[test]
    fn nothing_on_offer_means_nothing_is_installed() {
        let offer = UpdateOffer {
            release: None,
            pinned_to: Some("1.4.2".into()),
            required: false,
            looking: false,
        };
        assert!(offer.release.is_none());
        let text = serde_json::to_string(&offer).unwrap();
        assert!(!text.contains("\"release\""), "{text}");
        // Said only when it is true, so an older seat reads the same answer
        // it always has.
        assert!(!text.contains("looking"), "{text}");
    }


    // ---- replacing a program that is a folder, not a file -------------------
    //
    // Only a Mac does this for real, because only a Mac needs `ditto` to
    // unpack a bundle without breaking its signature. Everything that decides
    // whether a download is fit to install, and everything that moves the
    // person's working program out of the way, is plain filesystem work and is
    // checked here on whatever this is running on.

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("excalibur-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn bundle(at: &std::path::Path, says: &str) -> std::path::PathBuf {
        let app = at.join("Excalibur View.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/Excalibur View"), says).unwrap();
        app
    }

    fn inside(app: &std::path::Path) -> String {
        std::fs::read_to_string(app.join("Contents/MacOS/Excalibur View")).unwrap()
    }

    #[test]
    fn the_new_program_takes_the_old_one_s_place() {
        let dir = scratch("exchange");
        let app = bundle(&dir, "the old one");
        let fresh = beside(&app, "staged");
        std::fs::create_dir_all(fresh.join("Contents/MacOS")).unwrap();
        std::fs::write(fresh.join("Contents/MacOS/Excalibur View"), "the new one").unwrap();

        exchange(&app, &fresh).unwrap();

        assert_eq!(inside(&app), "the new one");
        // The old one is kept until the new one has started once.
        assert_eq!(inside(&beside(&app, "previous")), "the old one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_swap_puts_the_working_program_back() {
        let dir = scratch("rollback");
        let app = bundle(&dir, "the old one");
        // Nothing staged, so the second rename cannot succeed. The person must
        // still have the program they had before.
        let missing = beside(&app, "staged");

        assert!(exchange(&app, &missing).is_err());
        assert!(app.exists(), "the program was left missing");
        assert_eq!(inside(&app), "the old one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_an_update_leaves_behind_is_cleared_away() {
        let dir = scratch("tidy");
        let app = bundle(&dir, "running");
        for what in ["previous", "staged", "update"] {
            std::fs::create_dir_all(beside(&app, what)).unwrap();
        }

        tidy_after_bundle_update(&app);

        for what in ["previous", "staged", "update"] {
            assert!(!beside(&app, what).exists(), "{what} was left behind");
        }
        assert!(app.exists(), "the program itself was taken away");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leftovers_are_hidden_and_sit_beside_the_program_not_inside_it() {
        let app = std::path::Path::new("/Applications/Excalibur View.app");
        let previous = beside(app, "previous");
        assert_eq!(previous.parent().unwrap(), std::path::Path::new("/Applications"));
        assert!(previous.file_name().unwrap().to_str().unwrap().starts_with('.'));
        assert!(!previous.starts_with(app));
    }

    #[test]
    fn a_download_holding_one_program_is_the_one_used() {
        let dir = scratch("one");
        let app = bundle(&dir, "just me");
        assert_eq!(only_bundle(&dir).unwrap(), app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_download_holding_no_program_is_refused() {
        let dir = scratch("none");
        std::fs::write(dir.join("readme.txt"), "nothing here").unwrap();
        let refused = only_bundle(&dir).unwrap_err();
        assert!(refused.to_string().contains("no program"), "{refused}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two programs in one download is not a thing a release ever is, so it is
    /// something else, and guessing which one to install is how the wrong one
    /// gets installed.
    #[test]
    fn a_download_holding_two_programs_is_refused_rather_than_guessed_at() {
        let dir = scratch("two");
        std::fs::create_dir_all(dir.join("One.app")).unwrap();
        std::fs::create_dir_all(dir.join("Two.app")).unwrap();
        let refused = only_bundle(&dir).unwrap_err();
        assert!(refused.to_string().contains("2 programs"), "{refused}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Unix only, and not because the code is. `swap_bundle_in` builds on every
    // platform and is tested on every platform above; this one test needs a
    // folder it is guaranteed not to be allowed to write to, and the only
    // portable way to know that is to ask whether this process is root --
    // which is a question Windows has no `geteuid` to answer. Asking it there
    // cost a green Windows build a linker error, on a test that was never
    // about Windows.
    #[cfg(unix)]
    #[test]
    fn a_program_in_a_folder_it_cannot_write_to_says_so_before_anything_else() {
        // The root directory is the one folder a test can rely on not being
        // writable, and it is exactly the case this guards: an app somebody
        // else installed, in a place this person does not own.
        if unsafe { libc_geteuid() } == 0 {
            return; // running as root, where everything is writable
        }
        let app = std::path::Path::new("/Excalibur View.app");
        let refused = swap_bundle_in(app, b"not even read").unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(refused.to_string().contains("drag it in"), "{refused}");
    }

    #[cfg(unix)]
    extern "C" {
        #[link_name = "geteuid"]
        fn libc_geteuid() -> u32;
    }
}
