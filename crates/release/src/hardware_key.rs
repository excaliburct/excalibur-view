//! Signing on a hardware security key (a YubiKey), in two halves.
//!
//! A hardware key never hands its key over. It stays plugged into one
//! computer and signs short messages there. So `sign` is split either side of
//! that computer:
//!
//! 1. [`prepare`] does everything `sign` does up to the signature, and instead
//!    of signing writes down the exact bytes to be signed, in a signing
//!    request (`excalibur-sign-request/1`). The request, the unsigned manifest
//!    and a copy of the installer are kept together in a new folder.
//! 2. A copy of the request is signed on the computer the key is plugged into,
//!    which writes a signatures file (`excalibur-signatures/1`).
//! 3. [`finish`] reads the folder's own kept request (never the copy that
//!    travelled), checks every signature against the key this program trusts,
//!    checks that nothing kept has changed, and returns the manifest `sign`
//!    would have written.
//!
//! What is signed does not change, and nor does how a seat checks it: the
//! message is [`Release::signing_payload`], the very bytes the key file signs.
//! Ed25519 signatures are deterministic (RFC 8032), so the same key over the
//! same bytes makes the same signature, and a release finished here is byte
//! for byte the release `sign` makes. The tests below prove that, and prove
//! each refusal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use hub::update::{hex, Release, Trusted};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const REQUEST_FORMAT: &str = "excalibur-sign-request/1";
pub const SIGNATURES_FORMAT: &str = "excalibur-signatures/1";
pub const PRODUCT: &str = "hyperview";
/// The manifest as it will be, but for its signature, kept beside the request.
pub const UNSIGNED: &str = "manifest.unsigned.json";
/// The folder the installer is kept in, under the name it was given.
pub const INSTALLER: &str = "installer";
const MOST_ITEMS: usize = 100;
const LARGEST_MESSAGE: usize = 1024 * 1024;
const LONGEST_WHAT: usize = 200;
/// The slots a hardware key signs from: the four standard ones and the
/// twenty "retired key" slots, which is where an imported key usually goes.
const PIV_SLOTS: &[&str] = &[
    "9a", "9c", "9d", "9e", "82", "83", "84", "85", "86", "87", "88", "89", "8a", "8b", "8c",
    "8d", "8e", "8f", "90", "91", "92", "93", "94", "95",
];

/// File 1: what is asked to be signed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub format: String,
    pub product: String,
    pub version: String,
    pub step: String,
    pub commit: String,
    pub made: String,
    pub slot: String,
    /// The raw 32-byte public key, base64.
    #[serde(rename = "publicKey")]
    pub public_key: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    /// Plain English, shown before it is signed.
    pub what: String,
    /// The exact bytes to sign, base64.
    pub message: String,
}

/// File 2: what came back. Fields this does not use (which hardware key,
/// when) are read past rather than refused.
#[derive(Clone, Debug, Deserialize)]
pub struct Signatures {
    pub format: String,
    pub product: String,
    pub version: String,
    pub step: String,
    pub commit: String,
    #[serde(rename = "publicKey")]
    pub public_key: String,
    pub signatures: Vec<Signed>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Signed {
    pub id: String,
    #[serde(rename = "messageSha256")]
    pub message_sha256: String,
    pub signature: String,
}

/// How a release is to be prepared.
pub struct How<'a> {
    /// The slot holding the release key, as two hex digits.
    pub slot: &'a str,
    /// Which trusted key, if the program trusts more than one.
    pub key_name: Option<&'a str>,
    /// The commit built, in full, or "".
    pub commit: &'a str,
    pub made: &'a str,
    /// Where the kept folder is made.
    pub out_dir: &'a Path,
}

#[derive(Debug)]
pub struct Prepared {
    pub folder: PathBuf,
    pub request: PathBuf,
}

pub struct Finished {
    /// The manifest, signed.
    pub release: Release,
    pub folder: PathBuf,
    /// The kept installer the manifest describes.
    pub installer: PathBuf,
}

/// The step a manifest for one platform is signed in.
pub fn step_for(release: &Release) -> String {
    format!("release-{}", release.platform)
}

/// Everything `sign` does up to the signature. `release` is the manifest
/// as `sign` describes it, unsigned; `installer` the bytes it describes.
pub fn prepare(
    trusted: &Trusted,
    how: &How,
    mut release: Release,
    installer_name: &str,
    installer: &[u8],
) -> Result<Prepared, String> {
    let slot = piv_slot(how.slot)?;
    let (name, public) = choose_key(trusted, how.key_name)?;
    if !is_commit(how.commit) {
        return Err(format!(
            "'{}' is not a full git commit id. Give all of it, or leave --commit off.",
            how.commit
        ));
    }
    if installer_name.contains(['/', '\\']) || installer_name.starts_with('.') {
        return Err(format!("'{installer_name}' is not a file name the installer can be kept under."));
    }
    release.key = name;
    release.signature = String::new();

    let message = release.signing_payload();
    let request = Request {
        format: REQUEST_FORMAT.into(),
        product: PRODUCT.into(),
        version: release.version.clone(),
        step: step_for(&release),
        commit: how.commit.to_string(),
        made: how.made.to_string(),
        slot,
        public_key: b64(&public),
        items: vec![Item {
            id: release.platform.clone(),
            what: what_it_is(&release),
            message: b64(&message),
        }],
    };
    // Held to the rules finish holds it to before anybody is asked to sign it.
    check_request(&request)?;

    let folder = new_folder(how.out_dir, &release.version, &request.step)?;
    let write = |path: PathBuf, bytes: &[u8]| {
        std::fs::write(&path, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))
    };
    write(folder.join(INSTALLER).join(installer_name), installer)?;
    write(folder.join(UNSIGNED), crate::manifest_text(&release)?.as_bytes())?;
    let path = folder.join(format!("{PRODUCT}-{}-{}.sign-request.json", request.version, request.step));
    let mut text = serde_json::to_string_pretty(&request).map_err(|e| e.to_string())?;
    text.push('\n');
    write(path.clone(), text.as_bytes())?;
    Ok(Prepared { folder, request: path })
}

/// The manifest `sign` would have written, from the kept request and the
/// signatures that came back. Refuses, in words, unless every check holds.
pub fn finish(trusted: &Trusted, request_path: &Path, signatures_path: &Path) -> Result<Finished, String> {
    let request: Request = read_json(request_path, "the kept signing request")?;
    let messages = check_request(&request)?;
    let public: [u8; 32] = unb64(&request.public_key, "the kept request's public key")?
        .try_into()
        .map_err(|_| "the kept request's public key is not 32 bytes".to_string())?;
    let name = trusted_name_for(trusted, &public)?;
    let folder = request_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    // Nothing built has changed since it was prepared.
    let mut release: Release = read_json(&folder.join(UNSIGNED), "the manifest kept with the request")?;
    let installer = the_kept_installer(&folder)?;
    let bytes = std::fs::read(&installer)
        .map_err(|e| format!("could not read {}: {e}", installer.display()))?;
    if bytes.len() as u64 != release.bytes || hex(&Sha256::digest(&bytes)) != release.digest.to_lowercase() {
        return Err(format!(
            "{} has changed since it was prepared, so the signature is over an installer that \
             is not the one here. Prepare it again.",
            installer.display()
        ));
    }
    let expected = BTreeMap::from([(release.platform.clone(), release.signing_payload())]);
    if expected != messages {
        return Err("The kept request no longer matches the manifest kept beside it. Prepare it again.".into());
    }

    let signatures: Signatures = read_json(signatures_path, "the signatures file")?;
    let mut signed = check_signatures(&request, &messages, &signatures, &public)?;
    release.key = name.clone();
    release.signature = signed.remove(&release.platform).map(|s| hex(&s)).unwrap_or_default();

    // The same check `sign` makes of its own work, against the trusted key.
    Trusted { keys: vec![(name, public)] }
        .check(&release, &bytes)
        .map_err(|e| format!("the finished manifest does not check out: {e}"))?;
    Ok(Finished { release, folder, installer })
}

/// Every check a signatures file has to pass. `request` is the release's own
/// kept request and `public` the key the program trusts: the key the
/// signatures file names has to be that one, and each signature is checked
/// against that one, never against what the file says.
pub fn check_signatures(
    request: &Request,
    messages: &BTreeMap<String, Vec<u8>>,
    signatures: &Signatures,
    public: &[u8; 32],
) -> Result<BTreeMap<String, [u8; 64]>, String> {
    let refuse = |why: String| Err(format!("The signatures file {why} Nothing was signed."));
    if signatures.format != SIGNATURES_FORMAT {
        return refuse(format!("is not in the format this reads ({SIGNATURES_FORMAT})."));
    }
    for (field, theirs, ours) in [
        ("product", &signatures.product, &request.product),
        ("version", &signatures.version, &request.version),
        ("step", &signatures.step, &request.step),
        ("commit", &signatures.commit, &request.commit),
    ] {
        if theirs != ours {
            return refuse(format!(
                "is for {field} '{theirs}', and this release's own request says '{ours}'."
            ));
        }
    }
    if unb64(&signatures.public_key, "the signatures file's public key")? != *public {
        return refuse("names a public key that is not the release key the program trusts.".into());
    }
    let mut got: BTreeMap<&str, &Signed> = BTreeMap::new();
    for entry in &signatures.signatures {
        if got.insert(entry.id.as_str(), entry).is_some() {
            return refuse(format!("has two signatures for {}.", entry.id));
        }
    }
    let missing: Vec<&str> = messages.keys().map(String::as_str).filter(|id| !got.contains_key(id)).collect();
    if !missing.is_empty() {
        return refuse(format!("has no signature for {}.", missing.join(", ")));
    }
    let extra: Vec<&str> = got.keys().copied().filter(|id| !messages.contains_key(*id)).collect();
    if !extra.is_empty() {
        return refuse(format!(
            "has signatures for {}, which this release did not ask for.",
            extra.join(", ")
        ));
    }

    let key = ed25519_dalek::VerifyingKey::from_bytes(public)
        .map_err(|_| "the trusted release key is not a usable key".to_string())?;
    let mut out = BTreeMap::new();
    for (id, message) in messages {
        let entry = got[id.as_str()];
        if entry.message_sha256 != hex(&Sha256::digest(message)) {
            return refuse(format!(
                "was signed over something other than {id}'s message: its SHA-256 is not this release's."
            ));
        }
        let raw = unb64(&entry.signature, &format!("the signature for {id}"))?;
        let Ok(bytes) = <[u8; 64]>::try_from(raw.as_slice()) else {
            return refuse(format!("has a signature for {id} that is not 64 bytes."));
        };
        // Strict, as a seat checks it (`hub::update::verify`).
        if key
            .verify_strict(message, &ed25519_dalek::Signature::from_bytes(&bytes))
            .is_err()
        {
            return refuse(format!(
                "has a signature for {id} that does not check out against the release key the \
                 program trusts. Every installed copy would refuse it."
            ));
        }
        out.insert(id.clone(), bytes);
    }
    Ok(out)
}

/// That a request is one the format allows. Returns what it asks to have
/// signed, by id.
pub fn check_request(request: &Request) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let refuse = |why: String| Err(format!("The signing request {why}"));
    if request.format != REQUEST_FORMAT {
        return refuse(format!("is not in the format this reads ({REQUEST_FORMAT})."));
    }
    if request.product != PRODUCT {
        return refuse(format!("is for '{}', not Excalibur View ('{PRODUCT}').", request.product));
    }
    if request.version.is_empty() {
        return refuse("names no version.".into());
    }
    if !(1..=40).contains(&request.step.len())
        || !request.step.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return refuse("has no proper step name.".into());
    }
    if !is_commit(&request.commit) {
        return refuse("has a commit that is not a full git commit id.".into());
    }
    if !PIV_SLOTS.contains(&request.slot.as_str()) {
        return refuse("names no slot a hardware key signs from.".into());
    }
    if unb64(&request.public_key, "the signing request's public key")?.len() != 32 {
        return refuse("has a public key that is not 32 bytes.".into());
    }
    if !(1..=MOST_ITEMS).contains(&request.items.len()) {
        return refuse(format!("must ask for 1 to {MOST_ITEMS} signatures."));
    }
    let mut messages = BTreeMap::new();
    for item in &request.items {
        if !is_id(&item.id) {
            return refuse(format!("has an item called '{}'.", item.id));
        }
        if messages.contains_key(&item.id) {
            return refuse(format!("asks for {} twice.", item.id));
        }
        if item.what.trim().is_empty() || item.what.chars().count() > LONGEST_WHAT {
            return refuse(format!(
                "does not say what {} is in {LONGEST_WHAT} characters or fewer.",
                item.id
            ));
        }
        let message = unb64(&item.message, &format!("the message for {}", item.id))?;
        if message.is_empty() || message.len() > LARGEST_MESSAGE {
            return refuse(format!("has a message for {} that is empty or over 1 MiB.", item.id));
        }
        messages.insert(item.id.clone(), message);
    }
    Ok(messages)
}

/// (name, public key) of the release key to prepare for: the one the program
/// trusts, or the one named if it trusts several.
pub fn choose_key(trusted: &Trusted, name: Option<&str>) -> Result<(String, [u8; 32]), String> {
    let names = || trusted.keys.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ");
    match name {
        _ if trusted.keys.is_empty() => Err(
            "The program trusts no release key, so every installed copy would refuse whatever \
             this signed."
                .into(),
        ),
        None if trusted.keys.len() > 1 => Err(format!(
            "The program trusts more than one release key ({}). Say which with --key-name.",
            names()
        )),
        None => Ok(trusted.keys[0].clone()),
        Some(name) => trusted
            .key(name)
            .map(|key| (name.to_string(), *key))
            .ok_or_else(|| format!("'{name}' is not a release key the program trusts. It trusts: {}.", names())),
    }
}

fn trusted_name_for(trusted: &Trusted, public: &[u8]) -> Result<String, String> {
    trusted
        .keys
        .iter()
        .find(|(_, key)| key.as_slice() == public)
        .map(|(name, _)| name.clone())
        .ok_or_else(|| {
            "This was prepared for a key the program does not trust. Every installed copy would \
             refuse what it signed."
                .to_string()
        })
}

fn piv_slot(text: &str) -> Result<String, String> {
    let slot = text.trim().to_ascii_lowercase();
    if PIV_SLOTS.contains(&slot.as_str()) {
        Ok(slot)
    } else {
        Err(format!(
            "'{text}' is not a slot a hardware key signs from. Give --slot as the two hex digits \
             of the slot holding the release key: 9a, 9c, 9d, 9e, or 82 to 95."
        ))
    }
}

fn is_commit(text: &str) -> bool {
    matches!(text.len(), 0 | 40 | 64) && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_id(text: &str) -> bool {
    let mut bytes = text.bytes();
    let first_ok = bytes.next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    first_ok
        && text.len() <= 64
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
}

fn what_it_is(release: &Release) -> String {
    let channel = match release.channel {
        hub::update::Channel::Stable => "stable",
        hub::update::Channel::Preview => "preview",
    };
    let text = format!(
        "Excalibur View {} for {} ({channel} channel): {} bytes, SHA-256 {}",
        release.version,
        release.platform,
        release.bytes,
        &release.digest[..release.digest.len().min(16)]
    );
    text.chars().take(LONGEST_WHAT).collect()
}

/// A new folder for one signing, never one already used.
fn new_folder(parent: &Path, version: &str, step: &str) -> Result<PathBuf, String> {
    let base = format!("{PRODUCT}-{version}-{step}");
    let mut n = 1;
    loop {
        let name = if n == 1 { base.clone() } else { format!("{base}-{n}") };
        let folder = parent.join(name);
        match std::fs::create_dir(&folder) {
            Ok(()) => {
                std::fs::create_dir(folder.join(INSTALLER))
                    .map_err(|e| format!("could not make {}: {e}", folder.display()))?;
                return Ok(folder);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(format!("could not make {}: {e}", folder.display())),
        }
    }
}

fn the_kept_installer(folder: &Path) -> Result<PathBuf, String> {
    let kept = folder.join(INSTALLER);
    let found: Vec<PathBuf> = std::fs::read_dir(&kept)
        .map_err(|e| format!("could not read {}: {e}", kept.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_file())
        .collect();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(format!(
            "{} should hold the one installer that was prepared, and holds {}. Prepare it again.",
            kept.display(),
            found.len()
        )),
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, what: &str) -> Result<T, String> {
    let raw = std::fs::read(path).map_err(|e| format!("could not read {what} ({}): {e}", path.display()))?;
    if raw.starts_with(b"\xef\xbb\xbf") {
        return Err(format!(
            "{what} ({}) starts with a byte-order mark. The format is UTF-8 JSON without one.",
            path.display()
        ));
    }
    serde_json::from_slice(&raw).map_err(|e| format!("{what} ({}) is not readable: {e}", path.display()))
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(text: &str, what: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|_| format!("{what} is not base64."))
}

#[cfg(test)]
mod tests {
    //! A throwaway key stands in for the one on the hardware key: it signs a
    //! copy of the request the way the computer the key is plugged into does.
    //! The same key, [42; 32], that the hub's fixture tests use.

    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::{json, Value};

    const PUBLISHED: &str = "2026-09-23T12:00:00Z";
    const A_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
    const INSTALLER_BYTES: &[u8] = b"MZ pretend installer 1.5.0";

    fn throwaway() -> SigningKey {
        SigningKey::from_bytes(&[42u8; 32])
    }

    fn somebody_else() -> SigningKey {
        SigningKey::from_bytes(&[9u8; 32])
    }

    fn trusting(key: &SigningKey) -> Trusted {
        Trusted { keys: vec![("test-2026".into(), key.verifying_key().to_bytes())] }
    }

    /// A folder of its own for each test, in the system's temporary folder.
    struct Bench {
        dir: PathBuf,
    }

    impl Bench {
        fn new(name: &str) -> Bench {
            let dir = std::env::temp_dir().join(format!("hv-hardware-key-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("travelled")).unwrap();
            Bench { dir }
        }

        fn asked(&self) -> crate::ToSign {
            crate::ToSign {
                version: "1.5.0".into(),
                platform: "windows-x64".into(),
                channel: hub::update::Channel::Preview,
                notes: "Dynamic Fill.".into(),
                installer: "Setup.exe".into(),
                bytes: INSTALLER_BYTES.to_vec(),
            }
        }

        /// What `sign --key` writes, for the same release.
        fn with_key_file(&self) -> String {
            let key = throwaway();
            let mut release = crate::describe(&self.asked(), PUBLISHED.into(), "test-2026");
            release.signature = hex(&key.sign(&release.signing_payload()).to_bytes());
            crate::manifest_text(&release).unwrap()
        }

        fn prepare_with(&self, trusted: &Trusted, slot: &str, key_name: Option<&str>, commit: &str) -> Result<Prepared, String> {
            prepare(
                trusted,
                &How { slot, key_name, commit, made: PUBLISHED, out_dir: &self.dir },
                crate::describe(&self.asked(), PUBLISHED.into(), ""),
                "Setup.exe",
                INSTALLER_BYTES,
            )
        }

        fn prepare(&self) -> PathBuf {
            self.prepare_with(&trusting(&throwaway()), "9d", None, A_COMMIT).unwrap().request
        }

        /// What the computer with the hardware key does: signs a COPY of the
        /// request and writes a signatures file. The closures break things.
        fn hardware_key_signs(
            &self,
            request: &Path,
            key: &SigningKey,
            change_request: impl FnOnce(&mut Value),
            change: impl FnOnce(&mut Value),
        ) -> PathBuf {
            let copy = self.dir.join("travelled").join(request.file_name().unwrap());
            std::fs::copy(request, &copy).unwrap();
            let mut travelled: Value = serde_json::from_slice(&std::fs::read(&copy).unwrap()).unwrap();
            change_request(&mut travelled);
            let mut signatures = json!({
                "format": SIGNATURES_FORMAT,
                "product": travelled["product"], "version": travelled["version"],
                "step": travelled["step"], "commit": travelled["commit"],
                "publicKey": b64(&key.verifying_key().to_bytes()),
                "yubikey": 12345678,
                "signed": "2026-10-16T19:05:00Z",
                "signatures": travelled["items"].as_array().unwrap().iter().map(|item| {
                    let message = unb64(item["message"].as_str().unwrap(), "").unwrap();
                    json!({
                        "id": item["id"],
                        "messageSha256": hex(&Sha256::digest(&message)),
                        "signature": b64(&key.sign(&message).to_bytes()),
                    })
                }).collect::<Vec<_>>(),
            });
            change(&mut signatures);
            let path = copy.with_file_name(
                copy.file_name().unwrap().to_str().unwrap().replace(".sign-request.json", ".signatures.json"),
            );
            std::fs::write(&path, serde_json::to_vec_pretty(&signatures).unwrap()).unwrap();
            path
        }

        fn signed(&self, request: &Path) -> PathBuf {
            self.hardware_key_signs(request, &throwaway(), |_| {}, |_| {})
        }

        fn signed_with(&self, request: &Path, change: impl FnOnce(&mut Value)) -> PathBuf {
            self.hardware_key_signs(request, &throwaway(), |_| {}, change)
        }

        fn finish(&self, request: &Path, signatures: &Path) -> Result<String, String> {
            finish(&trusting(&throwaway()), request, signatures)
                .and_then(|done| crate::manifest_text(&done.release))
        }

        fn refuses(&self, fragment: &str, request: &Path, signatures: &Path) {
            match self.finish(request, signatures) {
                Ok(_) => panic!("finished when it should have refused ({fragment})"),
                Err(said) => assert!(said.contains(fragment), "expected '{fragment}' in: {said}"),
            }
        }

        fn edit(&self, path: &Path, change: impl FnOnce(&mut Value)) {
            let mut value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            change(&mut value);
            std::fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        }
    }

    impl Drop for Bench {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    // ---- the key-file path, and the round trip ----------------------------

    /// What `sign --key` wrote before any of this existed, for this release:
    /// taken from the program as it was, with the published date (which is
    /// not signed) set to the one used here.
    const SIGNED_WITH_THE_KEY_FILE: &str = r#"{
  "version": "1.5.0",
  "channel": "preview",
  "platform": "windows-x64",
  "published": "2026-09-23T12:00:00Z",
  "notes": "Dynamic Fill.",
  "download": "/api/v1/update/1.5.0/download?platform=windows-x64",
  "bytes": 26,
  "digest": "338ca2feb22406a863d7c1543a28507ac8104b760ab7626475009099ff049bb7",
  "signature": "67f4a1fad1a8609e7d4679d2ed41424d8c11f2ede004570dd4fb9f3636f18b4b88889b9aac63414a536cd17386865ed3c287ea17489aac6e2258d6cdbbc9f602",
  "key": "test-2026",
  "minimum_api_version": 1
}"#;

    #[test]
    fn the_key_file_still_signs_exactly_what_it_always_has() {
        let bench = Bench::new("key-file");
        assert_eq!(bench.with_key_file(), SIGNED_WITH_THE_KEY_FILE);
    }

    #[test]
    fn a_release_finished_on_the_hardware_key_is_byte_for_byte_the_key_files() {
        let bench = Bench::new("round-trip");
        let request = bench.prepare();
        let finished = bench.finish(&request, &bench.signed(&request)).unwrap();
        assert_eq!(finished, bench.with_key_file());
        assert_eq!(finished, SIGNED_WITH_THE_KEY_FILE);
        // And a seat takes it.
        let release: Release = serde_json::from_str(&finished).unwrap();
        assert_eq!(trusting(&throwaway()).check(&release, INSTALLER_BYTES), Ok(()));
    }

    #[test]
    fn finish_hands_back_the_kept_installer() {
        let bench = Bench::new("installer");
        let request = bench.prepare();
        let done = finish(&trusting(&throwaway()), &request, &bench.signed(&request)).unwrap();
        assert_eq!(std::fs::read(&done.installer).unwrap(), INSTALLER_BYTES);
        assert_eq!(done.installer.file_name().unwrap(), "Setup.exe");
    }

    // ---- what prepare writes ------------------------------------------------

    #[test]
    fn the_request_says_what_the_contract_says() {
        let bench = Bench::new("request");
        let request = bench.prepare();
        assert_eq!(
            request.file_name().unwrap(),
            "hyperview-1.5.0-release-windows-x64.sign-request.json"
        );
        let raw = std::fs::read(&request).unwrap();
        assert!(!raw.starts_with(b"\xef\xbb\xbf"));
        let made: Request = serde_json::from_slice(&raw).unwrap();
        assert_eq!(made.format, "excalibur-sign-request/1");
        assert_eq!(made.product, "hyperview");
        assert_eq!((made.version.as_str(), made.step.as_str()), ("1.5.0", "release-windows-x64"));
        assert_eq!((made.commit.as_str(), made.slot.as_str()), (A_COMMIT, "9d"));
        assert_eq!(unb64(&made.public_key, "").unwrap(), throwaway().verifying_key().to_bytes());
        assert_eq!(made.items.len(), 1);
        assert_eq!(made.items[0].id, "windows-x64");
        assert!(made.items[0].what.chars().count() <= 200);
        // The message is exactly what the key file signs and a seat checks.
        let release: Release = serde_json::from_str(SIGNED_WITH_THE_KEY_FILE).unwrap();
        assert_eq!(unb64(&made.items[0].message, "").unwrap(), release.signing_payload());
    }

    #[test]
    fn the_installer_is_kept_beside_it_and_a_second_prepare_makes_a_new_folder() {
        let bench = Bench::new("kept");
        let first = bench.prepare();
        let before = std::fs::read(&first).unwrap();
        let folder = first.parent().unwrap();
        assert_eq!(std::fs::read(folder.join(INSTALLER).join("Setup.exe")).unwrap(), INSTALLER_BYTES);
        assert!(folder.join(UNSIGNED).exists());

        let second = bench.prepare();
        assert_ne!(first.parent(), second.parent());
        assert!(second.parent().unwrap().ends_with("hyperview-1.5.0-release-windows-x64-2"));
        assert_eq!(std::fs::read(&first).unwrap(), before, "the first was left alone");
    }

    // ---- prepare refuses ----------------------------------------------------

    #[test]
    fn prepare_refuses_a_slot_that_is_not_a_slot_and_takes_one_in_capitals() {
        let bench = Bench::new("slots");
        for slot in ["", "8w", "9b", "96", "0x9d", "slot 9d"] {
            let refused = bench.prepare_with(&trusting(&throwaway()), slot, None, A_COMMIT).err();
            assert!(
                refused.as_deref().is_some_and(|said| said.contains("is not a slot a hardware key signs from")),
                "{slot}: {refused:?}"
            );
        }
        assert_eq!(std::fs::read_dir(&bench.dir).unwrap().count(), 1, "nothing kept but the travelled folder");
        let prepared = bench.prepare_with(&trusting(&throwaway()), "9D", None, A_COMMIT).unwrap();
        let made: Request = serde_json::from_slice(&std::fs::read(prepared.request).unwrap()).unwrap();
        assert_eq!(made.slot, "9d");
    }

    #[test]
    fn prepare_refuses_a_commit_that_is_not_a_whole_one() {
        let bench = Bench::new("commits");
        for commit in ["0123abc", "0123456789ABCDEF0123456789ABCDEF01234567", "not a commit"] {
            let refused = bench.prepare_with(&trusting(&throwaway()), "9d", None, commit).unwrap_err();
            assert!(refused.contains("Give all of it, or leave --commit off"), "{refused}");
        }
        assert!(bench.prepare_with(&trusting(&throwaway()), "9d", None, "").is_ok(), "\"\" is allowed");
    }

    #[test]
    fn prepare_refuses_when_the_program_trusts_no_key_or_several_and_none_is_named() {
        let bench = Bench::new("which-key");
        let none = Trusted { keys: vec![] };
        assert!(bench.prepare_with(&none, "9d", None, A_COMMIT).unwrap_err().contains("trusts no release key"));
        let several = Trusted {
            keys: vec![
                ("old-2025".into(), somebody_else().verifying_key().to_bytes()),
                ("test-2026".into(), throwaway().verifying_key().to_bytes()),
            ],
        };
        assert!(bench.prepare_with(&several, "9d", None, A_COMMIT).unwrap_err().contains("--key-name"));
        assert!(bench
            .prepare_with(&several, "9d", Some("nobody-2026"), A_COMMIT)
            .unwrap_err()
            .contains("is not a release key the program trusts"));
        let prepared = bench.prepare_with(&several, "9d", Some("test-2026"), A_COMMIT).unwrap();
        let made: Request = serde_json::from_slice(&std::fs::read(prepared.request).unwrap()).unwrap();
        assert_eq!(unb64(&made.public_key, "").unwrap(), throwaway().verifying_key().to_bytes());
    }

    #[test]
    fn prepare_refuses_an_installer_name_that_would_be_kept_somewhere_else() {
        let bench = Bench::new("names");
        for name in ["../Setup.exe", "sub/Setup.exe", ".hidden"] {
            let refused = prepare(
                &trusting(&throwaway()),
                &How { slot: "9d", key_name: None, commit: A_COMMIT, made: PUBLISHED, out_dir: &bench.dir },
                crate::describe(&bench.asked(), PUBLISHED.into(), ""),
                name,
                INSTALLER_BYTES,
            )
            .err()
            .unwrap_or_default();
            assert!(refused.contains("is not a file name the installer can be kept under"), "{name}: {refused}");
        }
    }

    // ---- every refusal finish makes -----------------------------------------

    #[test]
    fn a_wrong_signature_is_refused() {
        let bench = Bench::new("wrong-signature");
        let request = bench.prepare();
        let signed = bench.signed_with(&request, |s| {
            let mut raw = unb64(s["signatures"][0]["signature"].as_str().unwrap(), "").unwrap();
            raw[10] ^= 1;
            s["signatures"][0]["signature"] = json!(b64(&raw));
        });
        bench.refuses("does not check out against the release key the program trusts", &request, &signed);
    }

    #[test]
    fn a_signature_by_another_key_claiming_to_be_the_trusted_one_is_refused() {
        let bench = Bench::new("another-key");
        let request = bench.prepare();
        let signed = bench.hardware_key_signs(&request, &somebody_else(), |_| {}, |s| {
            s["publicKey"] = json!(b64(&throwaway().verifying_key().to_bytes()));
        });
        bench.refuses("does not check out against the release key the program trusts", &request, &signed);
    }

    #[test]
    fn a_signatures_file_naming_another_key_is_refused() {
        let bench = Bench::new("names-another-key");
        let request = bench.prepare();
        let signed = bench.hardware_key_signs(&request, &somebody_else(), |_| {}, |_| {});
        bench.refuses("names a public key that is not the release key the program trusts", &request, &signed);
    }

    #[test]
    fn an_id_missing_is_refused() {
        let bench = Bench::new("missing");
        let request = bench.prepare();
        let signed = bench.signed_with(&request, |s| s["signatures"] = json!([]));
        bench.refuses("has no signature for windows-x64", &request, &signed);
    }

    #[test]
    fn an_extra_id_is_refused() {
        let bench = Bench::new("extra");
        let request = bench.prepare();
        let signed = bench.signed_with(&request, |s| {
            let mut another = s["signatures"][0].clone();
            another["id"] = json!("linux-x64");
            s["signatures"].as_array_mut().unwrap().push(another);
        });
        bench.refuses("has signatures for linux-x64, which this release did not ask for", &request, &signed);
    }

    #[test]
    fn an_id_twice_is_refused() {
        let bench = Bench::new("twice");
        let request = bench.prepare();
        let signed = bench.signed_with(&request, |s| {
            let again = s["signatures"][0].clone();
            s["signatures"].as_array_mut().unwrap().push(again);
        });
        bench.refuses("has two signatures for windows-x64", &request, &signed);
    }

    #[test]
    fn a_changed_message_hash_is_refused() {
        let bench = Bench::new("hash");
        let request = bench.prepare();
        let signed = bench.signed_with(&request, |s| {
            s["signatures"][0]["messageSha256"] = json!(hex(&Sha256::digest(b"something else")));
        });
        bench.refuses("was signed over something other than windows-x64's message", &request, &signed);
    }

    #[test]
    fn the_copy_that_travelled_is_never_read() {
        let bench = Bench::new("travelled");
        let request = bench.prepare();
        let signed = bench.hardware_key_signs(
            &request,
            &throwaway(),
            |r| r["items"][0]["message"] = json!(b64(b"hyperview-release-v1\nversion=6.6.6\n")),
            |_| {},
        );
        bench.refuses("was signed over something other than windows-x64's message", &request, &signed);
    }

    #[test]
    fn an_installer_changed_after_prepare_is_refused() {
        let bench = Bench::new("changed");
        let request = bench.prepare();
        let signed = bench.signed(&request);
        let kept = request.parent().unwrap().join(INSTALLER).join("Setup.exe");
        std::fs::write(&kept, b"MZ pretend installer 1.5.1").unwrap();
        bench.refuses("has changed since it was prepared", &request, &signed);
    }

    #[test]
    fn an_installer_gone_or_joined_by_another_is_refused() {
        let bench = Bench::new("gone");
        let request = bench.prepare();
        let signed = bench.signed(&request);
        let kept = request.parent().unwrap().join(INSTALLER);
        std::fs::write(kept.join("Other.exe"), b"another").unwrap();
        bench.refuses("holds 2. Prepare it again", &request, &signed);
        std::fs::remove_file(kept.join("Other.exe")).unwrap();
        std::fs::remove_file(kept.join("Setup.exe")).unwrap();
        bench.refuses("holds 0. Prepare it again", &request, &signed);
    }

    #[test]
    fn the_kept_manifest_changed_after_prepare_is_refused() {
        let bench = Bench::new("kept-manifest");
        let request = bench.prepare();
        let signed = bench.signed(&request);
        bench.edit(&request.parent().unwrap().join(UNSIGNED), |m| m["minimum_api_version"] = json!(2));
        bench.refuses("no longer matches the manifest kept beside it", &request, &signed);
    }

    #[test]
    fn the_kept_request_changed_after_prepare_is_refused() {
        let bench = Bench::new("kept-request");
        let request = bench.prepare();
        let signed = bench.signed(&request);
        bench.edit(&request, |r| r["items"][0]["message"] = json!(b64(b"hyperview-release-v1\nsomething else\n")));
        bench.refuses("no longer matches the manifest kept beside it", &request, &signed);
    }

    #[test]
    fn another_product_version_step_or_commit_is_refused() {
        let bench = Bench::new("fields");
        let request = bench.prepare();
        for (field, value, fragment) in [
            ("product", "fabwire", "is for product 'fabwire'"),
            ("version", "1.5.1", "is for version '1.5.1'"),
            ("step", "release-linux-x64", "is for step 'release-linux-x64'"),
            ("commit", "ffffffffffffffffffffffffffffffffffffffff", "is for commit 'ffff"),
        ] {
            let signed = bench.signed_with(&request, |s| s[field] = json!(value));
            bench.refuses(fragment, &request, &signed);
        }
    }

    #[test]
    fn a_malformed_signatures_file_is_refused() {
        let bench = Bench::new("malformed-signatures");
        let request = bench.prepare();
        let broken = bench.dir.join("travelled").join("broken.json");
        for (raw, fragment) in [
            (&b"{ this is not json"[..], "is not readable"),
            (&b"\xef\xbb\xbf{}"[..], "starts with a byte-order mark"),
            (&b"[]"[..], "is not readable"),
        ] {
            std::fs::write(&broken, raw).unwrap();
            bench.refuses(fragment, &request, &broken);
        }
        let cases: Vec<(Box<dyn FnOnce(&mut Value)>, &str)> = vec![
            (Box::new(|s| s["format"] = json!("excalibur-signatures/2")), "is not in the format this reads"),
            (Box::new(|s| s.as_object_mut().unwrap().remove("publicKey").map(|_| ()).unwrap()), "is not readable"),
            (Box::new(|s| s["publicKey"] = json!("not base64!")), "public key is not base64"),
            (Box::new(|s| s["signatures"][0]["signature"] = json!("not base64!")), "signature for windows-x64 is not base64"),
            (
                Box::new(|s| {
                    let raw = unb64(s["signatures"][0]["signature"].as_str().unwrap(), "").unwrap();
                    s["signatures"][0]["signature"] = json!(b64(&raw[..63]));
                }),
                "has a signature for windows-x64 that is not 64 bytes",
            ),
            (Box::new(|s| { s["signatures"][0].as_object_mut().unwrap().remove("id"); }), "is not readable"),
        ];
        for (change, fragment) in cases {
            let signed = bench.signed_with(&request, change);
            bench.refuses(fragment, &request, &signed);
        }
        bench.refuses("could not read the signatures file", &request, &bench.dir.join("nowhere.json"));
    }

    #[test]
    fn a_request_for_a_key_the_program_no_longer_trusts_is_refused() {
        let bench = Bench::new("untrusted");
        let request = bench.prepare();
        let signed = bench.signed(&request);
        let refused = finish(&trusting(&somebody_else()), &request, &signed).err().unwrap_or_default();
        assert!(refused.contains("prepared for a key the program does not trust"), "{refused}");
    }

    #[test]
    fn a_malformed_kept_request_is_refused() {
        let bench = Bench::new("malformed-request");
        let request = bench.prepare();
        let signed = bench.signed(&request);
        let original = std::fs::read(&request).unwrap();
        let item = |field: &'static str, value: Value| -> Box<dyn Fn(&mut Value)> {
            Box::new(move |r| r["items"][0][field] = value.clone())
        };
        let cases: Vec<(Box<dyn Fn(&mut Value)>, &str)> = vec![
            (Box::new(|r| r["format"] = json!("something/1")), "is not in the format this reads"),
            (Box::new(|r| r["product"] = json!("arcvalley")), "is for 'arcvalley'"),
            (Box::new(|r| r["version"] = json!("")), "names no version"),
            (Box::new(|r| r["step"] = json!("Release")), "has no proper step name"),
            (Box::new(|r| r["step"] = json!("r".repeat(41))), "has no proper step name"),
            (Box::new(|r| r["commit"] = json!("0123abc")), "not a full git commit id"),
            (Box::new(|r| r["slot"] = json!("8w")), "names no slot"),
            (Box::new(|r| r["publicKey"] = json!(b64(&[7u8; 31]))), "public key that is not 32 bytes"),
            (Box::new(|r| r["items"] = json!([])), "must ask for 1 to 100 signatures"),
            (
                Box::new(|r| {
                    let one = r["items"][0].clone();
                    r["items"] = json!((0..101).map(|n| { let mut x = one.clone(); x["id"] = json!(format!("x{n}")); x }).collect::<Vec<_>>());
                }),
                "must ask for 1 to 100 signatures",
            ),
            (item("id", json!("Windows-x64")), "has an item called 'Windows-x64'"),
            (item("id", json!("x".repeat(65))), "has an item called"),
            (
                Box::new(|r| {
                    let again = r["items"][0].clone();
                    r["items"].as_array_mut().unwrap().push(again);
                }),
                "asks for windows-x64 twice",
            ),
            (item("what", json!("  ")), "does not say what windows-x64 is"),
            (item("what", json!("x".repeat(201))), "does not say what windows-x64 is"),
            (item("message", json!("")), "empty or over 1 MiB"),
            (item("message", json!(b64(&vec![b'm'; 1024 * 1024 + 1]))), "empty or over 1 MiB"),
            (item("message", json!("%%%")), "message for windows-x64 is not base64"),
        ];
        for (change, fragment) in cases {
            std::fs::write(&request, &original).unwrap();
            bench.edit(&request, |r| change(r));
            bench.refuses(fragment, &request, &signed);
        }
        std::fs::write(&request, b"not json").unwrap();
        bench.refuses("is not readable", &request, &signed);
    }

    #[test]
    fn and_with_everything_right_it_finishes() {
        // The control for every refusal above: the same steps, nothing broken.
        let bench = Bench::new("control");
        let request = bench.prepare();
        assert!(bench.finish(&request, &bench.signed(&request)).is_ok());
    }
}
