//! A computer's device key, and the lines it signs (docs/OFFICE-ENROLLMENT.md
//! §3 and §4).
//!
//! Every app on every computer has its own Ed25519 key. The Office Server
//! keeps the office's list of them and is the only authority on which may
//! connect. Two things are signed with it, both as plain lines of ASCII, each
//! ended by `\n`, because two JSON writers never agree on spacing and a
//! signature over how something was printed isn't one:
//!
//! - the request to be let in (`excalibur-enroll/1`), bound to the server's
//!   certificate pin so a request made to an impostor can't be relayed;
//! - every request after (`excalibur-request/1`), bound to the method, the
//!   path and query exactly as sent, the body's SHA-256 and the time.
//!
//! The .NET side (`LatCon Estimating` and the Citadel cell) builds the same
//! bytes; `docs/office-enrollment-vectors.json` holds the fixed key, id and
//! signatures both sides test against.

use base64::Engine;
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

/// The label a device id is worked out under. Citadel's own ids use
/// `citadel-device/1` over the same key, so the two never collide.
pub const DEVICE_LABEL: &str = "excalibur-device/1"; // WHAT: §3's label. WHY: the id is SHA-256 of this, "\n", then the key's 32 bytes

/// The first line of an enrol proof.
pub const ENROLL_LABEL: &str = "excalibur-enroll/1"; // WHAT: §3's first line. WHY: a proof can never be read as a request signature, or the other way round

/// The first line of a request signature.
pub const REQUEST_LABEL: &str = "excalibur-request/1"; // WHAT: §4's first line. WHY: as above

/// The header carrying the device id.
pub const HEADER_DEVICE: &str = "X-Excalibur-Device"; // WHAT: §4's header. WHY: the server looks the key up by it
/// The header carrying the time the request was signed, in Unix seconds.
pub const HEADER_TIME: &str = "X-Excalibur-Time"; // WHAT: §4's header. WHY: checked within ±300 s
/// The header carrying the base64 Ed25519 signature.
pub const HEADER_SIGNATURE: &str = "X-Excalibur-Signature"; // WHAT: §4's header. WHY: the signature itself
/// The header carrying the body's SHA-256, the same 64 hex digits the
/// signature's body line holds. With it the server checks the signature
/// before it reads a byte of the body, then holds the body to it.
pub const HEADER_BODY: &str = "X-Excalibur-Body-SHA256"; // WHAT: the declared body hash. WHY: a body is read only for a request already proven; without it, the server reads at most a small body

/// How far a signed time may be from the server's clock, either way.
pub const WINDOW_SECONDS: i64 = 300; // WHAT: ±300 s. WHY: §4; a signature older than this is refused, and the replay cache need keep no longer

/// The pin line an enrol proof carries when the server serves no HTTPS.
pub const NO_PIN: &str = "none"; // WHAT: the word in place of a pin. WHY: a server run with HTTPS off (a test, a Linux box behind its own proxy) still has a line there, the same word the office certificate uses

/// The apps that may ask to be let in, as the request names them.
pub const APPS: &[&str] = &["view", "cad", "archetype", "latcon-estimating"]; // WHAT: §3's list. WHY: anything else is refused before it is stored

/// The longest request target (path and query) that is signed.
pub const LONGEST_TARGET: usize = 2048; // WHAT: 2 KiB. WHY: far longer than any route and its query; a bound keeps hostile headers cheap

/// The longest machine name, Windows user name or display name kept.
pub const LONGEST_NAME: usize = 128; // WHAT: 128 characters. WHY: Windows' own limits are shorter; a stranger's request can't fill the list with megabytes

/// The device id of a 32-byte public key: the first 16 bytes of
/// SHA-256("excalibur-device/1\n" + key), as 32 lowercase hex digits.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn device_id(public_key: &[u8; 32]) -> String {
    let mut hasher = Sha256::new(); // WHAT: one SHA-256. WHY: §3
    hasher.update(DEVICE_LABEL.as_bytes()); // WHAT: the label. WHY: first
    hasher.update(b"\n"); // WHAT: the line end after it. WHY: §3 writes the label as a line
    hasher.update(public_key); // WHAT: the key's raw 32 bytes, not its base64. WHY: the same bytes Citadel's DeviceKeys.IdOf hashes
    crate::update::hex(&hasher.finalize()[..16]) // WHAT: the first 16 bytes, lowercase hex. WHY: 32 characters, as Citadel's ids
}

/// Whether `text` reads as a device id: 32 lowercase hex digits.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn is_device_id(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) // WHAT: the shape and nothing else. WHY: a header that isn't one is refused before the database is asked
}

/// A public key sent as base64 (exactly 44 characters for 32 bytes) that is
/// a point on the curve, or `None`.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn parse_public_key(text: &str) -> Option<[u8; 32]> {
    if text.len() != 44 {
        return None; // WHAT: the wrong length. WHY: 32 bytes are always 44 characters of padded base64
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(text).ok()?; // WHAT: standard base64, padded. WHY: what .NET's Convert.ToBase64String writes
    let key: [u8; 32] = bytes.try_into().ok()?; // WHAT: exactly 32 bytes. WHY: an Ed25519 public key
    if base64::engine::general_purpose::STANDARD.encode(key) != text {
        return None; // WHAT: a spelling that doesn't round-trip (stray low bits in the padding). WHY: one key has one text, so one key has one row
    }
    let point = VerifyingKey::from_bytes(&key).ok()?; // WHAT: a point on the curve. WHY: a key that isn't one can never sign anything, and is never stored
    if point.is_weak() {
        return None; // WHAT: a small-order point. WHY: such a key "verifies" signatures nobody made
    }
    Some(key)
}

/// A public key as the wire writes it: standard padded base64.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn public_key_text(public_key: &[u8; 32]) -> String {
    base64::engine::general_purpose::STANDARD.encode(public_key) // WHAT: 44 characters. WHY: §3's form
}

/// The SHA-256 of a request body, as 64 lowercase hex digits (of no bytes
/// when there is no body).
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn body_hash(body: &[u8]) -> String {
    crate::update::hex(&Sha256::digest(body)) // WHAT: the body's digest. WHY: §4's fifth line
}

/// A field that goes on a line of its own: one line of printable text, no
/// longer than `longest` characters.
fn one_line(field: &str, longest: usize) -> bool {
    !field.is_empty() && field.chars().count() <= longest && !field.chars().any(char::is_control) // WHAT: present, bounded, no line breaks or other control characters. WHY: no field can move another into its place
}

/// A method: upper-case ASCII letters only.
fn is_method(method: &str) -> bool {
    (1..=10).contains(&method.len()) && method.bytes().all(|b| b.is_ascii_uppercase()) // WHAT: GET, POST, PUT, DELETE and the like. WHY: anything else can't be a method and is never signed
}

/// A request target: starts with "/", printable ASCII without spaces.
fn is_target(target: &str) -> bool {
    target.starts_with('/') && target.len() <= LONGEST_TARGET && target.bytes().all(|b| (b'!'..=b'~').contains(&b)) // WHAT: what goes on the request line. WHY: Citadel's rule, so both sides refuse the same targets
}

/// 64 lowercase hex digits: a SHA-256 written out.
fn is_hex64(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) // WHAT: the shape. WHY: a body hash or a pin, and nothing else
}

/// A server pin line: 64 lowercase hex digits, or [`NO_PIN`].
fn is_pin_line(pin: &str) -> bool {
    pin == NO_PIN || is_hex64(pin) // WHAT: the two forms. WHY: the server compares it with its own
}

/// What an enrol proof signs (§3), or why the fields can't be signed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enroll<'a> {
    pub server_pin: &'a str, // WHAT: the server's 8714 pin, or "none". WHY: a request made to an impostor can't be relayed
    pub public_key: &'a str, // WHAT: the key as base64. WHY: signed as the text sent
    pub app: &'a str, // WHAT: view, cad, archetype or latcon-estimating. WHY: who is asking
    pub machine: &'a str, // WHAT: the computer's name. WHY: what an admin sees
    pub os_user: &'a str, // WHAT: the Windows user. WHY: likewise
    pub time: i64, // WHAT: Unix seconds. WHY: a proof from long ago is refused
}

impl Enroll<'_> {
    /// Exactly the bytes signed: seven lines, each ended by `\n`.
    ///
    /// # Errors
    ///
    /// When a field isn't well formed, so it can't be signed or checked.
    pub fn message(&self) -> Result<Vec<u8>, String> {
        if !is_pin_line(self.server_pin) {
            return Err("the server's pin isn't 64 lowercase hex digits".into()); // WHAT: refused. WHY: a proof bound to nothing binds nothing
        }
        if parse_public_key(self.public_key).is_none() {
            return Err("the public key isn't a 32-byte Ed25519 key in base64".into()); // WHAT: refused. WHY: as above
        }
        if !APPS.contains(&self.app) {
            return Err("the app isn't one of view, cad, archetype or latcon-estimating".into()); // WHAT: refused. WHY: the list is fixed
        }
        if !one_line(self.machine, LONGEST_NAME) || !one_line(self.os_user, LONGEST_NAME) {
            return Err("the computer's name or the Windows user name isn't one line of text".into()); // WHAT: refused. WHY: a line break would move the next field
        }
        if self.time < 0 {
            return Err("the time is before 1970".into()); // WHAT: refused. WHY: never a real clock
        }
        Ok(format!(
            "{ENROLL_LABEL}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            self.server_pin, self.public_key, self.app, self.machine, self.os_user, self.time
        )
        .into_bytes()) // WHAT: the seven lines, UTF-8. WHY: §3 exactly; a machine or user name outside ASCII is its UTF-8 bytes on both sides
    }
}

/// What a request signature signs (§4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request<'a> {
    pub server_pin: &'a str, // WHAT: the server's 8714 pin, or "none". WHY: a request signed for one server is never good at another, even the same device's
    pub device_id: &'a str, // WHAT: the signing device's id. WHY: the key is found by it
    pub method: &'a str, // WHAT: GET, POST... WHY: a signed GET is never a POST
    pub target: &'a str, // WHAT: the path and query exactly as sent. WHY: a signature for one route is never another's
    pub body_sha256: &'a str, // WHAT: the body's digest in hex. WHY: the body can't be changed on the way
    pub time: i64, // WHAT: Unix seconds. WHY: checked within ±300 s
}

impl Request<'_> {
    /// Exactly the bytes signed: seven lines, each ended by `\n`: the label,
    /// the server's pin, the device id, the method, the target, the body's
    /// SHA-256 and the time.
    ///
    /// # Errors
    ///
    /// When a field isn't well formed, so it can't be signed or checked.
    pub fn message(&self) -> Result<Vec<u8>, String> {
        if !is_pin_line(self.server_pin) || !is_device_id(self.device_id) || !is_method(self.method) || !is_target(self.target) || !is_hex64(self.body_sha256) || self.time < 0 {
            return Err("a request field isn't well formed, so it can't be signed or checked".into()); // WHAT: refused whole. WHY: Citadel's rule; hostile headers only ever make it refuse
        }
        Ok(format!(
            "{REQUEST_LABEL}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            self.server_pin, self.device_id, self.method, self.target, self.body_sha256, self.time
        )
        .into_bytes()) // WHAT: the seven lines, ASCII. WHY: §4, with the pin line the security review asked for
    }
}

/// A signature from its base64 text (88 characters), or `None`.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn parse_signature(text: &str) -> Option<Signature> {
    if text.len() != 88 {
        return None; // WHAT: the wrong length. WHY: 64 bytes are always 88 characters of padded base64
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(text).ok()?; // WHAT: standard base64. WHY: as .NET writes it
    let bytes: [u8; 64] = bytes.try_into().ok()?; // WHAT: exactly 64 bytes. WHY: an Ed25519 signature
    Some(Signature::from_bytes(&bytes)) // WHAT: the signature. WHY: checked against the key next
}

/// Whether `signature` (base64) is `public_key`'s signature of `message`.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn verify(public_key: &[u8; 32], message: &[u8], signature: &str) -> bool {
    let Some(signature) = parse_signature(signature) else { return false }; // WHAT: a signature that doesn't read. WHY: never verifies
    let Ok(key) = VerifyingKey::from_bytes(public_key) else { return false }; // WHAT: a key that isn't a point. WHY: never verifies
    if key.is_weak() {
        return false; // WHAT: a small-order key. WHY: never trusted to have signed anything
    }
    key.verify_strict(message, &signature).is_ok() // WHAT: the strict check. WHY: refuses the malleable forms a lax check lets through, so one message has one signature and the replay cache can't be stepped round
}

/// The three headers a signed request carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    pub device: String, // WHAT: X-Excalibur-Device. WHY: §4
    pub time: String, // WHAT: X-Excalibur-Time. WHY: §4
    pub signature: String, // WHAT: X-Excalibur-Signature. WHY: §4
    pub body: String, // WHAT: X-Excalibur-Body-SHA256, the body line's hash. WHY: the signature is checked before the body is read
}

/// A device key: the private half, kept on this computer, and what it signs.
pub struct DeviceKey {
    signing: SigningKey, // WHAT: the Ed25519 private key. WHY: never leaves the computer; dalek clears it when dropped
}

impl std::fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DeviceKey({})", self.id()) // WHAT: the id only. WHY: a log line never carries the private half
    }
}

impl DeviceKey {
    /// The key whose private half is these 32 bytes.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn from_secret(secret: &[u8; 32]) -> DeviceKey {
        DeviceKey { signing: SigningKey::from_bytes(secret) } // WHAT: the key. WHY: read back from where it is kept
    }

    /// A new key from the operating system's random numbers.
    ///
    /// # Errors
    ///
    /// When the system can't give random numbers.
    #[cfg(feature = "client")]
    pub fn generate() -> Result<DeviceKey, String> {
        let mut secret = zeroed(); // WHAT: room for 32 random bytes. WHY: the private half
        rustls::crypto::ring::default_provider()
            .secure_random
            .fill(&mut secret)
            .map_err(|_| "this computer couldn't give random numbers for its key".to_string())?; // WHAT: the system's random numbers, through the TLS library already in the build. WHY: no new dependency for one key
        let key = DeviceKey::from_secret(&secret); // WHAT: the key. WHY: made once, on first run
        secret.fill(0); // WHAT: the copy cleared. WHY: only the key itself holds it now
        Ok(key)
    }

    /// The private half, for keeping. Cleared by the caller once kept.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn secret(&self) -> [u8; 32] {
        self.signing.to_bytes() // WHAT: the 32 bytes. WHY: sealed by DPAPI or written to a 0600 file
    }

    /// The public half's 32 bytes.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn public_key(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes() // WHAT: the public key. WHY: sent to the server, and the id is worked out from it
    }

    /// The public half as the wire writes it.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn public_key_text(&self) -> String {
        public_key_text(&self.public_key()) // WHAT: base64. WHY: §3's form
    }

    /// This key's device id.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn id(&self) -> String {
        device_id(&self.public_key()) // WHAT: worked out from the public half. WHY: neither side has to trust the other's
    }

    /// The base64 signature of `message`.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn sign(&self, message: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.signing.sign(message).to_bytes()) // WHAT: Ed25519, deterministic. WHY: the same message always signs the same, so the test vectors hold
    }

    /// The proof for an enrol request (§3). `public_key` in `fields` must be
    /// this key's.
    ///
    /// # Errors
    ///
    /// When a field isn't well formed, or names another key.
    pub fn enroll_proof(&self, fields: &Enroll<'_>) -> Result<String, String> {
        if fields.public_key != self.public_key_text() {
            return Err("the request names another key".into()); // WHAT: refused. WHY: a proof for someone else's key proves nothing
        }
        Ok(self.sign(&fields.message()?)) // WHAT: the seven lines, signed. WHY: §3
    }

    /// The three headers for one request (§4).
    ///
    /// # Errors
    ///
    /// When the method or target isn't well formed.
    pub fn sign_request(&self, server_pin: &str, method: &str, target: &str, body: &[u8], time: i64) -> Result<Signed, String> {
        let id = self.id(); // WHAT: this key's id. WHY: a field and the first header
        let hash = body_hash(body); // WHAT: the body's digest. WHY: the body line, and its own header
        let message = Request { server_pin, device_id: &id, method, target, body_sha256: &hash, time }.message()?; // WHAT: the seven lines. WHY: §4
        Ok(Signed { device: id, time: time.to_string(), signature: self.sign(&message), body: hash }) // WHAT: the headers. WHY: put on the request
    }
}

/// A device key that signs requests, never two the same.
///
/// Ed25519 signs the same message the same way every time, and §4's lines
/// carry no nonce, so two identical requests in the same second would carry
/// one signature and the server's replay cache would refuse the second. So a
/// request identical to one already signed at that second is signed a second
/// later instead, well inside the ±300 s window.
pub struct Signer {
    key: DeviceKey, // WHAT: the device key. WHY: signs
    recent: std::sync::Mutex<std::collections::VecDeque<(String, i64)>>, // WHAT: what was signed lately, and at which second. WHY: never the same signature twice
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Signer({})", self.key.id()) // WHAT: the id only. WHY: never the private half
    }
}

impl Signer {
    /// A signer for `key`.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn new(key: DeviceKey) -> Signer {
        Signer { key, recent: std::sync::Mutex::new(std::collections::VecDeque::new()) }
    }

    /// The key itself.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn key(&self) -> &DeviceKey {
        &self.key // WHAT: borrowed. WHY: its id, and the enrol proof
    }

    /// The headers for one request, at `now` or a little after when the same
    /// request was already signed then.
    ///
    /// # Errors
    ///
    /// When the method or target isn't well formed.
    pub fn sign(&self, server_pin: &str, method: &str, target: &str, body: &[u8], now: i64) -> Result<Signed, String> {
        let what = format!("{method} {target} {}", body_hash(body)); // WHAT: the request, less its time. WHY: what makes two signatures the same
        let mut time = now; // WHAT: the second to sign at. WHY: moved on only when taken
        if let Ok(mut recent) = self.recent.lock() {
            while recent.front().is_some_and(|(_, at)| *at < now - WINDOW_SECONDS) {
                recent.pop_front(); // WHAT: forgotten once past the window. WHY: the server forgets them too
            }
            while recent.iter().any(|(w, at)| *w == what && *at == time) {
                time += 1; // WHAT: the next second. WHY: a fresh signature
            }
            recent.push_back((what, time)); // WHAT: noted. WHY: the next identical request moves on
            if recent.len() > 10_000 {
                recent.pop_front(); // WHAT: bounded. WHY: a long burst can't grow it without end
            }
        }
        self.key.sign_request(server_pin, method, target, body, time) // WHAT: signed. WHY: §4
    }
}

/// `POST /api/v1/devices/request`'s body (§3). Also carried, as `device`, by
/// a join from outside the office, which admits the computer it came from.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Ask {
    pub public_key: String, // WHAT: the device's public key, base64. WHY: the server keeps it and works the id out from it
    pub app: String, // WHAT: view, cad, archetype or latcon-estimating. WHY: shown to the admin
    #[serde(default)]
    pub app_version: String, // WHAT: the app's version. WHY: shown in the list; not signed
    pub machine: String, // WHAT: the computer's name. WHY: shown, and signed
    pub os_user: String, // WHAT: the Windows user. WHY: shown, and signed
    #[serde(default)]
    pub display_name: String, // WHAT: the person's own name, as the app knows it. WHY: "Dana wants to use..."; not signed
    #[serde(default)]
    pub platform: String, // WHAT: windows-x64 and the like. WHY: shown; not signed
    pub time: i64, // WHAT: Unix seconds. WHY: signed; an old proof is refused
    pub proof: String, // WHAT: the base64 signature of the seven lines. WHY: proves the asker holds the key, and which server it meant
}

impl Ask {
    /// A request for `key`, signed for the server whose pin is `server_pin`.
    ///
    /// # Errors
    ///
    /// When a field isn't well formed.
    #[allow(clippy::too_many_arguments)] // WHAT: every field of the request. WHY: §3 lists them; a struct for them would be the request itself
    pub fn signed(key: &DeviceKey, server_pin: &str, app: &str, app_version: &str, machine: &str, os_user: &str, display_name: &str, platform: &str, time: i64) -> Result<Ask, String> {
        let public_key = key.public_key_text(); // WHAT: the key as sent. WHY: signed as this text
        let proof = key.enroll_proof(&Enroll { server_pin, public_key: &public_key, app, machine, os_user, time })?; // WHAT: the seven lines signed. WHY: §3
        Ok(Ask {
            public_key,
            app: app.to_string(),
            app_version: app_version.to_string(),
            machine: machine.to_string(),
            os_user: os_user.to_string(),
            display_name: display_name.to_string(),
            platform: platform.to_string(),
            time,
            proof,
        })
    }

    /// Whether the proof is this request's key's signature for a server
    /// whose pin is `server_pin`; the key's 32 bytes when it is.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn proven(&self, server_pin: &str) -> Option<[u8; 32]> {
        let key = parse_public_key(&self.public_key)?; // WHAT: the key, read. WHY: a key that isn't one proves nothing
        let message = Enroll { server_pin, public_key: &self.public_key, app: &self.app, machine: &self.machine, os_user: &self.os_user, time: self.time }.message().ok()?; // WHAT: the seven lines as the server sees them. WHY: its own pin, never the asker's idea of it
        verify(&key, &message, &self.proof).then_some(key) // WHAT: the key, when the proof holds. WHY: stored only then
    }
}

/// What a device request is answered with.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Answer {
    pub device_id: String, // WHAT: the id the server worked out. WHY: the app checks it against its own
    pub state: String, // WHAT: waiting or approved. WHY: §3
}

/// `GET /api/v1/devices/me`'s answer (§3).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Me {
    pub state: String, // WHAT: waiting, approved, refused or removed. WHY: the waiting screen reads it every 5 s
    #[serde(default)]
    pub admin: bool, // WHAT: whether this device is an admin. WHY: the Computers list shows only for admins
    #[serde(default)]
    pub approved_by: Option<String>, // WHAT: who approved it. WHY: shown
    #[serde(default)]
    pub approved_at: Option<String>, // WHAT: when, RFC 3339. WHY: shown
}

/// One device, as an admin's Computers list shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Info {
    pub id: String, // WHAT: the device id. WHY: the one the approve, refuse and remove calls name
    pub app: String, // WHAT: which app. WHY: "wants to use LatCon Estimating"
    #[serde(default)]
    pub app_version: String, // WHAT: its version. WHY: shown
    pub machine: String, // WHAT: the computer. WHY: shown
    pub os_user: String, // WHAT: the Windows user. WHY: shown
    #[serde(default)]
    pub display_name: String, // WHAT: the person's name. WHY: shown first
    #[serde(default)]
    pub platform: String, // WHAT: windows-x64 and the like. WHY: shown
    pub state: String, // WHAT: waiting, approved, refused or removed. WHY: which list it is in
    #[serde(default)]
    pub admin: bool, // WHAT: whether it may approve others. WHY: Make or Unmake admin
    pub asked: String, // WHAT: when it asked, RFC 3339. WHY: "Asked 2 minutes ago"
    #[serde(default)]
    pub asked_from: String, // WHAT: the address it asked from. WHY: "from 192.168.1.44"
    #[serde(default)]
    pub approved_by: Option<String>, // WHAT: who approved it. WHY: shown
    #[serde(default)]
    pub approved_at: Option<String>, // WHAT: when. WHY: shown
    #[serde(default)]
    pub how: String, // WHAT: asked, claimed, migrated, joined or citadel. WHY: how it came in, for whoever reads the list later
    #[serde(default)]
    pub scope: String, // WHAT: "all", or "citadel" for a computer approved for the Citadel cell only. WHY: an admin widens it with Approve
}

/// A device id as a person compares it: its first eight hex digits in two
/// groups of four, upper case ("29AC 6B9D").
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn short_id(id: &str) -> String {
    let head: String = id.chars().take(8).collect::<String>().to_uppercase(); // WHAT: the first eight digits. WHY: enough to tell two computers apart by eye
    if head.len() < 8 { return head; } // WHAT: too short to group. WHY: shown as it is
    format!("{} {}", &head[..4], &head[4..]) // WHAT: two groups of four. WHY: easy to read out over the phone
}

/// The app's name for people, as an admin's list says it.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn app_title(app: &str) -> &str {
    match app {
        "view" => "Excalibur View", // WHAT: View. WHY: its own name
        "cad" => "Excalibur CAD", // WHAT: CAD. WHY: likewise
        "archetype" => "Archetype", // WHAT: Archetype. WHY: likewise
        "latcon-estimating" => "LatCon Estimating", // WHAT: Citadel's app at LatCon. WHY: likewise
        other => other, // WHAT: anything else, as it is. WHY: never stored, but never hidden either
    }
}

/// 32 zero bytes, to be filled.
#[cfg(feature = "client")]
fn zeroed() -> [u8; 32] {
    [0u8; 32] // WHAT: empty room. WHY: filled from the system's random numbers
}

/// This computer's name, as Windows or the system says it.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn machine_name() -> String {
    let named = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok().filter(|n| !n.trim().is_empty()); // WHAT: Windows' name, or the shell's. WHY: what an admin recognises
    let named = named.or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|n| n.trim().to_string())).filter(|n| !n.is_empty()); // WHAT: else the system's file. WHY: a Mac or Linux box without the variable
    clean(&named.unwrap_or_else(|| "this computer".to_string())) // WHAT: one short line. WHY: signed and shown
}

/// The signed-in user's name, as the system says it.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn os_user() -> String {
    clean(&std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "user".to_string())) // WHAT: Windows' user, or the shell's. WHY: "signed in to Windows as dana"
}

/// This build's platform, as the list shows it.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "windows-x64",
        ("windows", "aarch64") => "windows-arm64",
        ("macos", _) => "macos",
        ("linux", "x86_64") => "linux-x64",
        ("ios", _) => "ipad",
        ("android", _) => "android",
        _ => "other",
    } // WHAT: a short word. WHY: shown to an admin
}

/// Text made one short line: control characters dropped, at most
/// `LONGEST_NAME` characters, never empty.
fn clean(text: &str) -> String {
    let one: String = text.chars().filter(|c| !c.is_control()).take(LONGEST_NAME).collect::<String>().trim().to_string(); // WHAT: printable, bounded. WHY: a field that isn't one line is never signed
    if one.is_empty() { "unknown".to_string() } else { one } // WHAT: never empty. WHY: an empty field can't be signed
}

/// Unix seconds of a UTC RFC 3339 time as the server writes it
/// ("2026-11-11T09:30:00Z", fractions of a second allowed), or `None`.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn unix_of(text: &str) -> Option<i64> {
    let bytes = text.trim().as_bytes(); // WHAT: the text's bytes. WHY: read by position
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' || bytes[13] != b':' || bytes[16] != b':' || !matches!(bytes.last(), Some(b'Z' | b'z')) {
        return None; // WHAT: not the server's form. WHY: never guessed at
    }
    let number = |from: usize, to: usize| std::str::from_utf8(&bytes[from..to]).ok()?.parse::<i64>().ok(); // WHAT: a number at a place. WHY: each field
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?); // WHAT: the date. WHY: days since 1970
    let (hours, minutes, seconds) = (number(11, 13)?, number(14, 16)?, number(17, 19)?); // WHAT: the time of day. WHY: seconds into the day
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hours > 23 || minutes > 59 || seconds > 60 {
        return None; // WHAT: out of range. WHY: not a time
    }
    let (year, month) = if month <= 2 { (year - 1, month + 9) } else { (year, month - 3) }; // WHAT: the year from March. WHY: the days-from-civil sum
    let era = year.div_euclid(400); // WHAT: the 400-year era. WHY: as above
    let year_of_era = year - era * 400; // WHAT: the year inside its era. WHY: as above
    let day_of_year = (153 * month + 2) / 5 + day - 1; // WHAT: the day, counted from March. WHY: as above
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year; // WHAT: the day inside its era. WHY: as above
    let days = era * 146_097 + day_of_era - 719_468; // WHAT: days since 1970-01-01. WHY: Howard Hinnant's sum
    Some(days * 86_400 + hours * 3600 + minutes * 60 + seconds)
}

/// The time now, in Unix seconds.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX)) // WHAT: seconds since 1970. WHY: §4's time; a clock before 1970 reads as 0 and is refused by the window
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixed key the vectors are made with: bytes 0 to 31.
    fn fixed() -> DeviceKey {
        let mut secret = [0u8; 32];
        for (i, b) in secret.iter_mut().enumerate() {
            *b = u8::try_from(i).unwrap(); // WHAT: 0, 1, 2 ... 31. WHY: easy to type into any language's test
        }
        DeviceKey::from_secret(&secret)
    }

    const PIN: &str = "3f2a91c0d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f"; // WHAT: a made-up server pin. WHY: the vectors' fixed pin

    /// The vectors, as JSON, worked out here.
    fn vectors() -> serde_json::Value {
        let key = fixed();
        let public = key.public_key_text();
        let enroll = Enroll { server_pin: PIN, public_key: &public, app: "latcon-estimating", machine: "DANA-LAPTOP", os_user: "dana", time: 1_794_410_000 };
        let enroll_message = String::from_utf8(enroll.message().unwrap()).unwrap();
        let get = key.sign_request(PIN, "GET", "/api/v1/devices/me", b"", 1_794_410_000).unwrap();
        let body = br#"{"admin":false}"#;
        let post_target = format!("/api/v1/devices/{}/approve", key.id());
        let post = key.sign_request(PIN, "POST", &post_target, body, 1_794_410_005).unwrap();
        let get_message = String::from_utf8(Request { server_pin: PIN, device_id: &key.id(), method: "GET", target: "/api/v1/devices/me", body_sha256: &body_hash(b""), time: 1_794_410_000 }.message().unwrap()).unwrap();
        let post_message = String::from_utf8(Request { server_pin: PIN, device_id: &key.id(), method: "POST", target: &post_target, body_sha256: &body_hash(body), time: 1_794_410_005 }.message().unwrap()).unwrap();
        serde_json::json!({
            "contract": "docs/OFFICE-ENROLLMENT.md sections 3 and 4",
            "notes": [
                "The private key is the 32 bytes 0x00, 0x01 ... 0x1f (an Ed25519 seed, RFC 8032).",
                "device_id = lowercase hex of the first 16 bytes of SHA-256(ASCII \"excalibur-device/1\\n\" followed by the raw 32-byte public key).",
                "Keys and signatures are standard base64 with padding (44 and 88 characters).",
                "Every signed line ends with \\n, the last one too. The enrol proof's server pin line is the 8714 SPKI SHA-256 in lowercase hex, or the word none when the server serves no HTTPS.",
                "The body hash of a request with no body is the SHA-256 of no bytes.",
                "excalibur-request/1 signs seven lines: the label, the server's pin (or none), the device id, the method, the target, the body hash and the time. The server pin line is the same one the enrol proof carries.",
                "X-Excalibur-Body-SHA256 carries the body hash line, so the server checks the signature before reading the body. A request without it is read only up to 1 MiB."
            ],
            "private_key_hex": crate::update::hex(&key.secret()),
            "public_key": public,
            "device_id": key.id(),
            "enroll": {
                "server_pin": PIN, "app": "latcon-estimating", "machine": "DANA-LAPTOP", "os_user": "dana", "time": 1_794_410_000,
                "message": enroll_message,
                "proof": key.enroll_proof(&enroll).unwrap()
            },
            "server_pin": PIN,
            "requests": [
                { "method": "GET", "target": "/api/v1/devices/me", "body": "", "body_sha256": body_hash(b""), "time": 1_794_410_000, "message": get_message,
                  "headers": { HEADER_DEVICE: get.device, HEADER_TIME: get.time, HEADER_SIGNATURE: get.signature, HEADER_BODY: get.body } },
                { "method": "POST", "target": post_target, "body": String::from_utf8(body.to_vec()).unwrap(), "body_sha256": body_hash(body), "time": 1_794_410_005, "message": post_message,
                  "headers": { HEADER_DEVICE: post.device, HEADER_TIME: post.time, HEADER_SIGNATURE: post.signature, HEADER_BODY: post.body } }
            ]
        })
    }

    fn vectors_file() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/office-enrollment-vectors.json") // WHAT: the shared file. WHY: the .NET side tests against it
    }

    #[test]
    fn the_vectors_file_is_what_this_code_signs() {
        let made = serde_json::to_string_pretty(&vectors()).unwrap() + "\n"; // WHAT: the vectors, worked out now. WHY: compared with the file
        if std::env::var_os("EXCALIBUR_WRITE_VECTORS").is_some() {
            std::fs::write(vectors_file(), &made).unwrap(); // WHAT: written, only when asked. WHY: a change to the vectors is a change made on purpose
        }
        let kept = std::fs::read_to_string(vectors_file()).expect("docs/office-enrollment-vectors.json is there");
        assert_eq!(kept.replace("\r\n", "\n"), made, "the vectors file and the code disagree");
    }

    #[test]
    fn the_device_id_is_the_label_and_the_raw_key_hashed() {
        let key = fixed();
        let mut input = b"excalibur-device/1\n".to_vec();
        input.extend(key.public_key()); // WHAT: label, line end, raw key. WHY: §3 written out by hand
        assert_eq!(key.id(), crate::update::hex(&Sha256::digest(&input)[..16]));
        assert!(is_device_id(&key.id()));
        assert!(!is_device_id(&key.id().to_uppercase()), "upper case is not the form");
    }

    #[test]
    fn a_public_key_reads_only_in_its_one_form() {
        let key = fixed();
        assert_eq!(parse_public_key(&key.public_key_text()), Some(key.public_key()));
        assert!(parse_public_key(&key.public_key_text()[..43]).is_none());
        assert!(parse_public_key(&base64::engine::general_purpose::STANDARD.encode([0u8; 32])).is_none(), "a small-order point is refused");
        let mut low = key.public_key_text().into_bytes();
        low[42] = if low[42] == b'A' { b'B' } else { low[42] }; // WHAT: a different last character. WHY: may not round-trip, or names another key
        let text = String::from_utf8(low).unwrap();
        if let Some(other) = parse_public_key(&text) {
            assert_eq!(public_key_text(&other), text, "only a round-tripping spelling reads");
        }
    }

    #[test]
    fn a_field_with_a_line_break_is_never_signed() {
        let key = fixed();
        let public = key.public_key_text();
        let good = Enroll { server_pin: PIN, public_key: &public, app: "view", machine: "PC", os_user: "dana", time: 1 };
        assert!(good.message().is_ok());
        assert!(Enroll { machine: "PC\nview", ..good.clone() }.message().is_err());
        assert!(Enroll { os_user: "", ..good.clone() }.message().is_err());
        assert!(Enroll { app: "word", ..good.clone() }.message().is_err());
        assert!(Enroll { server_pin: "ABC", ..good.clone() }.message().is_err());
        assert!(Enroll { server_pin: NO_PIN, ..good.clone() }.message().is_ok(), "a server without HTTPS signs the word none");
        assert!(key.sign_request(PIN, "GET", "/a b", b"", 1).is_err(), "a space in the target");
        assert!(key.sign_request(PIN, "get", "/a", b"", 1).is_err(), "a method in lower case");
        assert!(key.sign_request(PIN, "GET", "a", b"", 1).is_err(), "a target without its slash");
    }

    #[test]
    fn a_server_time_reads_as_unix_seconds() {
        assert_eq!(unix_of("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_of("2026-11-11T09:30:00.123456Z"), Some(1_794_389_400));
        assert_eq!(unix_of("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(unix_of("not a time"), None);
    }

    #[test]
    fn the_same_request_twice_in_one_second_is_signed_twice_apart() {
        let signer = Signer::new(fixed());
        let first = signer.sign(PIN, "GET", "/api/v1/devices/me", b"", 1000).unwrap();
        let second = signer.sign(PIN, "GET", "/api/v1/devices/me", b"", 1000).unwrap();
        assert_ne!(first.signature, second.signature, "never one signature twice");
        assert_eq!(second.time, "1001");
        let other = signer.sign(PIN, "GET", "/api/v1/projects", b"", 1000).unwrap();
        assert_eq!(other.time, "1000", "another request keeps its own second");
    }

    #[test]
    fn a_signature_verifies_only_for_its_own_message() {
        let key = fixed();
        let signed = key.sign_request(PIN, "POST", "/api/v1/x?y=1", b"{}", 100).unwrap();
        let message = Request { server_pin: PIN, device_id: &key.id(), method: "POST", target: "/api/v1/x?y=1", body_sha256: &body_hash(b"{}"), time: 100 }.message().unwrap();
        assert!(verify(&key.public_key(), &message, &signed.signature));
        let other = Request { server_pin: PIN, device_id: &key.id(), method: "POST", target: "/api/v1/x?y=2", body_sha256: &body_hash(b"{}"), time: 100 }.message().unwrap();
        assert!(!verify(&key.public_key(), &other, &signed.signature), "another query");
        assert!(!verify(&key.public_key(), &message, "not a signature"));
    }
}
