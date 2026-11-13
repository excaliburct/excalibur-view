//! The Citadel cell's TLS key, held by the Office Server (docs/OFFICE-ENROLLMENT.md §6b).
//!
//! The cell serves HTTPS on 47110 under a restricted token that can't make or
//! import a private key anywhere Windows' TLS can use it. So the Office
//! Server, running as the system, keeps the cell's key itself: a persisted,
//! non-exportable machine key in Windows' own key store (the Microsoft
//! Software Key Storage Provider), whose permissions give the system full
//! control and the cell's own SID read (use) only, and nobody else anything.
//! The cell is handed the key's name and the certificate (public) on its
//! command line, and opens the key by name.
//!
//! Where the key comes from, decided by [`plan`]:
//!
//! - **first start after the move:** Citadel Server 1.0's `tls.pfx`, brought
//!   into the cell's folder by the move, is imported once; its certificate is
//!   kept, so every computer's pin still matches; the file itself is moved out
//!   of the cell's reach, into the Office Server's own locked folder;
//! - **a new office** (nothing to move): a new key is made with its
//!   permissions already set, and a certificate made for it;
//! - **every later start** (a restart, a new cell version, an update or a
//!   rollback): the kept key is opened by name, its permissions checked and,
//!   if anyone changed them, set back (said in the log as an alarm); a key
//!   owned by anyone but the system or administrators is refused.
//!
//! What is decided is decided here, in plain functions tested on any machine;
//! `cell::tlskey` (Windows) only hands it to Windows.

use std::path::{Path, PathBuf}; // WHAT: file paths. WHY: the kept files

/// The folder, inside the Office Server's own locked folder, where the cell's
/// key's name, its certificate and the moved `tls.pfx` are kept.
pub const FOLDER: &str = "citadel-tls"; // WHAT: `<service folder>\citadel-tls`. WHY: the system's and administrators' only, never the cell's
/// The kept key's name.
pub const KEY_NAME_FILE: &str = "key-name.txt"; // WHAT: one line, the key's name in Windows' key store. WHY: the key is opened by it at every start
/// The kept certificate, DER.
pub const CERTIFICATE_FILE: &str = "certificate.der"; // WHAT: the certificate the computers pinned. WHY: handed to the cell at every start
/// 1.0's certificate file, as the move leaves it in the cell's folder.
pub const PFX: &str = "tls.pfx"; // WHAT: Citadel's own name (ServerPaths.Certificate). WHY: imported once, then moved out of the cell's reach
/// The largest `tls.pfx` read.
pub const PFX_MOST: u64 = 64 * 1024; // WHAT: 64 KiB. WHY: Citadel's own limit (DpapiCertificateFile.MaxBytes); a planted giant is refused
/// The largest certificate passed to the cell.
pub const CERTIFICATE_MOST: usize = 4096; // WHAT: 4 KiB of DER. WHY: a P-256 self-made certificate is about 500 bytes; the command line stays short
/// What a new key's name starts with.
pub const KEY_PREFIX: &str = "ExcaliburOfficeCell-"; // WHAT: the prefix. WHY: an administrator looking at the machine's keys sees whose it is
/// DPAPI's extra secret for Citadel's `tls.pfx`.
pub const PFX_PURPOSE: &[u8] = b"Citadel Server tls.pfx/1"; // WHAT: Citadel's own purpose string (Dpapi.Purpose). WHY: the file opens only with it

/// The system's SID.
pub const SYSTEM: &str = "S-1-5-18"; // WHAT: LocalSystem. WHY: the Office Server runs as it, and keeps full control of the key
/// The administrators group's SID.
pub const ADMINISTRATORS: &str = "S-1-5-32-544"; // WHAT: BUILTIN\Administrators. WHY: an owner Windows itself gives keys the system makes
/// Full control of a file.
pub const FULL: u32 = 0x001F_01FF; // WHAT: FILE_ALL_ACCESS. WHY: the system's entry
/// Read of a file.
pub const READ: u32 = 0x0012_0089; // WHAT: FILE_GENERIC_READ. WHY: the cell's entry: enough to use the key, never to change or delete it

/// The kept folder.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn folder(home: &Path) -> PathBuf {
    home.join(FOLDER) // WHAT: `<service folder>\citadel-tls`. WHY: as [`FOLDER`]
}

/// The key's permissions, as SDDL: protected (nothing inherited from the key
/// store's folder), the system in full, the cell's SID read, nobody else.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn key_sddl(cell_sid: &str) -> String {
    format!("D:P(A;;FA;;;SY)(A;;FR;;;{cell_sid})") // WHAT: SYSTEM full (FA), the cell read (FR). WHY: Creede, 2026-11-13: only the cell's SID may use it; no other account gains anything
}

/// One entry of a key's permissions, as Windows reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub allow: bool, // WHAT: an allow entry (not a deny or anything else). WHY: only allow entries are expected
    pub inherited: bool, // WHAT: whether it came from the folder above. WHY: none may
    pub mask: u32, // WHAT: the rights. WHY: compared exactly
    pub sid: String, // WHAT: who, as an SID string. WHY: compared exactly
}

/// Whether a key's permissions are exactly ours: protected, the system in
/// full and the cell's SID read, and nothing else at all.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn locked_as_ours(protected: bool, entries: &[Entry], cell_sid: &str) -> bool {
    let want = [(SYSTEM, FULL), (cell_sid, READ)]; // WHAT: the two entries. WHY: [`key_sddl`]
    protected // WHAT: nothing from above. WHY: the key store's folder grants others
        && entries.len() == want.len() // WHAT: exactly two. WHY: a third entry is somebody else's way in
        && want.iter().all(|(sid, mask)| entries.iter().any(|e| e.allow && !e.inherited && e.sid == *sid && e.mask == *mask)) // WHAT: each, exactly. WHY: more rights for the cell, or less for the system, is a change
}

/// Whether a key's owner is one this server accepts: the system, or the
/// administrators group (the owner Windows may give a key the system made).
/// Any other owner made the key, or took it, and can always grant itself
/// access, so the key is refused.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn owner_ok(owner: &str) -> bool {
    owner == SYSTEM || owner == ADMINISTRATORS // WHAT: one of the two. WHY: nobody else may own the office's key
}

/// What a key name may be: letters, digits, `-`, `_`, `{`, `}` and `.`, at
/// most 100. Windows names imported keys itself (a GUID); a new key is named
/// [`KEY_PREFIX`] and 32 hex digits.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn is_key_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 100 && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_{}.".contains(c)) // WHAT: plain characters only. WHY: it goes on the cell's command line and in a file
}

/// A new key's name, from 16 random bytes.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn new_key_name(random: [u8; 16]) -> String {
    format!("{KEY_PREFIX}{}", hub::update::hex(&random)) // WHAT: the prefix and 32 hex digits. WHY: nobody can guess it to plant a key under it first
}

/// What the start does about the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    Import, // WHAT: import the moved `tls.pfx`. WHY: the first start after the move; the computers' pins are kept
    Make, // WHAT: make a new key and certificate. WHY: a new office, with nothing moved
    Reuse, // WHAT: open the kept key. WHY: every later start
    ReuseAndIgnorePfx, // WHAT: open the kept key; a `tls.pfx` in the cell's folder is left alone and named in an alarm. WHY: once the office holds a key, nothing the cell's folder holds replaces it
}

/// The cell's mark that it has started before (Citadel's `CellMove.WriteMark`).
pub const STARTED_MARK: &str = "state/moved-from.json"; // WHAT: written at the cell's first good start, or by the move. WHY: a cell that has served before has a certificate the computers pinned

/// What the start does, from what is there: a `tls.pfx` in the cell's
/// folder, the kept key's name and certificate, and whether the cell has
/// started before.
///
/// # Errors
///
/// When only half of the kept pair is there (the key's name without its
/// certificate or the other way round), or nothing is kept although the cell
/// has served before. The cell is then not started, because making a new key
/// would change the pin every computer holds.
pub fn plan(pfx_in_cell: bool, kept_name: bool, kept_certificate: bool, started_before: bool) -> Result<Plan, String> {
    match (kept_name, kept_certificate) {
        (true, true) if pfx_in_cell => Ok(Plan::ReuseAndIgnorePfx), // WHAT: held already; the file left alone. WHY: the cell can write its own folder, so a file there never replaces the office's key
        (true, true) => Ok(Plan::Reuse), // WHAT: held. WHY: the ordinary start
        (false, false) if pfx_in_cell => Ok(Plan::Import), // WHAT: the moved file, imported once. WHY: the first start after the move
        (false, false) if started_before => Err(format!("the Citadel cell has served before, but its key isn't kept in {FOLDER} any more, so the cell was not started: a new key would change the certificate every computer has pinned. Put {FOLDER} back from a backup; or, to start again with a new certificate that every computer must pair with afresh, delete {STARTED_MARK} in the cell's data folder")), // WHAT: refused. WHY: never a silent new identity
        (false, false) => Ok(Plan::Make), // WHAT: nothing anywhere, and a cell that never served. WHY: a new office
        _ => Err(format!("the Citadel cell's kept key is only half there ({KEY_NAME_FILE} and {CERTIFICATE_FILE} in {FOLDER} must both be), so the cell was not started; a new key would change the certificate every computer has pinned")), // WHAT: refused. WHY: never a silent new identity
    }
}

/// The point `0x04 ‖ X ‖ Y` from a CNG `BCRYPT_ECCKEY_BLOB` holding a P-256
/// public key (magic `ECS1`, 32-byte coordinates), or none.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn p256_point(blob: &[u8]) -> Option<Vec<u8>> {
    const ECS1: u32 = 0x3153_4345; // WHAT: BCRYPT_ECDSA_PUBLIC_P256_MAGIC. WHY: only a P-256 signing key
    let magic = u32::from_le_bytes(blob.get(0..4)?.try_into().ok()?); // WHAT: the blob's kind. WHY: checked
    let size = u32::from_le_bytes(blob.get(4..8)?.try_into().ok()?); // WHAT: each coordinate's length. WHY: 32 for P-256
    if magic != ECS1 || size != 32 || blob.len() != 8 + 64 {
        return None; // WHAT: not a P-256 public key blob. WHY: Citadel's keys are P-256 only
    }
    let mut point = vec![0x04]; // WHAT: uncompressed. WHY: the form a certificate holds
    point.extend_from_slice(&blob[8..]); // WHAT: X then Y. WHY: as above
    Some(point)
}

/// A raw `r ‖ s` ECDSA P-256 signature (as CNG makes it) written as the DER
/// `Ecdsa-Sig-Value` a certificate holds, or none if it isn't 64 bytes.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn der_signature(raw: &[u8]) -> Option<Vec<u8>> {
    if raw.len() != 64 {
        return None; // WHAT: not P-256's. WHY: nothing else is made
    }
    let integer = |half: &[u8]| {
        let trimmed: Vec<u8> = half.iter().copied().skip_while(|b| *b == 0).collect(); // WHAT: leading zeros dropped. WHY: DER's shortest form
        let mut body = if trimmed.is_empty() { vec![0] } else { trimmed }; // WHAT: zero is one zero byte. WHY: likewise
        if body[0] & 0x80 != 0 {
            body.insert(0, 0); // WHAT: a zero in front of a high bit. WHY: DER integers are signed
        }
        let mut out = vec![0x02, u8::try_from(body.len()).unwrap_or(0)]; // WHAT: INTEGER and its length (at most 33). WHY: DER
        out.extend(body);
        out
    };
    let (r, s) = (integer(&raw[..32]), integer(&raw[32..])); // WHAT: the two halves. WHY: r then s
    let mut out = vec![0x30, u8::try_from(r.len() + s.len()).unwrap_or(0)]; // WHAT: SEQUENCE and its length (at most 70). WHY: DER
    out.extend(r);
    out.extend(s);
    Some(out)
}

/// The certificate a new office's cell serves, as Citadel's own `SelfSigned.Make`
/// makes it: `CN=Citadel Server`, this computer's name and `localhost`, ten
/// years, a server certificate that signs only, never an authority.
///
/// # Errors
///
/// When rcgen refuses the parameters.
pub fn certificate_params(machine: &str, now: time::OffsetDateTime) -> Result<rcgen::CertificateParams, String> {
    let name: String = machine.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>().trim_matches('-').to_string(); // WHAT: letters, digits and dashes. WHY: Citadel's SafeName
    let name = if (1..=63).contains(&name.len()) { name } else { "citadel-server".to_string() }; // WHAT: a usable DNS name. WHY: likewise
    let mut params = rcgen::CertificateParams::new(vec![name, "localhost".to_string()]).map_err(|e| format!("the cell's certificate: {e}"))?; // WHAT: the two names. WHY: as Citadel's
    params.distinguished_name = rcgen::DistinguishedName::new(); // WHAT: rcgen's default name gone. WHY: Citadel's subject only
    params.distinguished_name.push(rcgen::DnType::CommonName, "Citadel Server"); // WHAT: CN=Citadel Server. WHY: as Citadel's
    params.not_before = now - time::Duration::minutes(5); // WHAT: five minutes back. WHY: a clock a little behind still accepts it
    params.not_after = now + time::Duration::days(3652); // WHAT: ten years. WHY: SelfSigned.Lasts
    params.is_ca = rcgen::IsCa::ExplicitNoCa; // WHAT: never an authority. WHY: as Citadel's basic constraints
    params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature]; // WHAT: signs only. WHY: as Citadel's
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth]; // WHAT: a server's. WHY: as Citadel's
    Ok(params)
}

/// The cell's two arguments for its key: `--tls-key NAME --tls-cert BASE64`.
///
/// # Errors
///
/// When the name isn't plain or the certificate is empty or too big.
pub fn cell_args(name: &str, certificate: &[u8]) -> Result<Vec<String>, String> {
    use base64::Engine as _; // WHAT: base64. WHY: the certificate on the command line
    if !is_key_name(name) {
        return Err(format!("the kept key's name {name:?} isn't a plain name, so the cell was not started")); // WHAT: refused. WHY: never an odd string on the command line
    }
    if certificate.is_empty() || certificate.len() > CERTIFICATE_MOST {
        return Err("the kept certificate is empty or too big, so the cell was not started".to_string()); // WHAT: refused. WHY: likewise
    }
    Ok(vec!["--tls-key".into(), name.into(), "--tls-cert".into(), base64::engine::general_purpose::STANDARD.encode(certificate)]) // WHAT: the four words. WHY: Citadel's CellOptions reads them
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(sid: &str, mask: u32) -> Entry {
        Entry { allow: true, inherited: false, mask, sid: sid.to_string() } // WHAT: one allow entry. WHY: the tests build lists of them
    }

    #[test]
    fn only_the_system_in_full_and_the_cell_reading_count_as_locked() {
        let cell = "S-1-5-80-1-2-3-4-5"; // WHAT: a cell's SID. WHY: as cell_sid makes them
        let ours = vec![entry(SYSTEM, FULL), entry(cell, READ)]; // WHAT: exactly ours. WHY: passes
        assert!(locked_as_ours(true, &ours, cell), "ours"); // WHAT: passes. WHY: as set
        assert!(!locked_as_ours(false, &ours, cell), "not protected: the store's folder grants others"); // WHAT: refused. WHY: inheritance would add entries
        let mut more = ours.clone();
        more.push(entry("S-1-5-32-545", READ)); // WHAT: Users added. WHY: the tampering asked about
        assert!(!locked_as_ours(true, &more, cell), "a third entry"); // WHAT: refused. WHY: anybody else gains nothing
        assert!(!locked_as_ours(true, &[entry(SYSTEM, FULL), entry(cell, FULL)], cell), "the cell given full control"); // WHAT: refused. WHY: read only
        assert!(!locked_as_ours(true, &[entry(SYSTEM, FULL), entry("S-1-5-80-9", READ)], cell), "another cell's SID"); // WHAT: refused. WHY: this cell's own
        let mut inherited = ours.clone();
        inherited[1].inherited = true; // WHAT: the cell's entry from above. WHY: never inherited
        assert!(!locked_as_ours(true, &inherited, cell), "an inherited entry"); // WHAT: refused. WHY: as above
        let mut denied = ours;
        denied[0].allow = false; // WHAT: a deny where the system's allow was. WHY: not ours
        assert!(!locked_as_ours(true, &denied, cell), "a deny entry"); // WHAT: refused. WHY: likewise
        assert_eq!(key_sddl(cell), format!("D:P(A;;FA;;;SY)(A;;FR;;;{cell})"), "what is set"); // WHAT: the SDDL. WHY: matches what is checked
    }

    #[test]
    fn a_key_owned_by_anyone_but_the_system_or_administrators_is_refused() {
        assert!(owner_ok(SYSTEM) && owner_ok(ADMINISTRATORS), "the two"); // WHAT: accepted. WHY: the system made it
        assert!(!owner_ok("S-1-5-21-1-2-3-1001"), "an ordinary account"); // WHAT: refused. WHY: it planted or took the key
        assert!(!owner_ok("S-1-5-80-1-2-3-4-5"), "the cell itself"); // WHAT: refused. WHY: the cell never owns its key
    }

    #[test]
    fn the_plan_imports_once_reuses_after_and_never_lets_the_cell_s_folder_replace_the_key() {
        assert_eq!(plan(true, false, false, true), Ok(Plan::Import), "the first start after the move"); // WHAT: import. WHY: the pins are kept
        assert_eq!(plan(false, false, false, false), Ok(Plan::Make), "a new office"); // WHAT: made. WHY: nothing to keep
        assert_eq!(plan(false, true, true, true), Ok(Plan::Reuse), "a restart"); // WHAT: reused. WHY: the ordinary start
        assert_eq!(plan(true, true, true, true), Ok(Plan::ReuseAndIgnorePfx), "a tls.pfx planted later"); // WHAT: the kept key. WHY: the cell can't swap the office's key
        assert!(plan(false, true, false, true).is_err() && plan(true, false, true, false).is_err(), "half a pair stops the cell"); // WHAT: refused. WHY: never a silent new pin
        assert!(plan(false, false, false, true).unwrap_err().contains("has served before"), "a lost key is never quietly replaced"); // WHAT: refused. WHY: the computers' pins
    }

    #[test]
    fn key_names_are_plain_and_new_ones_unguessable() {
        let name = new_key_name([0xab; 16]); // WHAT: a new name. WHY: its form
        assert_eq!(name, format!("{KEY_PREFIX}{}", "ab".repeat(16)), "prefix and 32 hex"); // WHAT: as made. WHY: plain
        assert!(is_key_name(&name) && is_key_name("{3F2504E0-4F89-11D3-9A0C-0305E82C3301}") && is_key_name("te-1b2c_3.x"), "ours and Windows' own"); // WHAT: accepted. WHY: imported keys are named by Windows
        for bad in ["", "a b", "a\"b", "..\\x", &"a".repeat(101)] {
            assert!(!is_key_name(bad), "{bad:?}"); // WHAT: refused. WHY: never on the command line
        }
        assert!(cell_args("a b", b"x").is_err() && cell_args("k", b"").is_err() && cell_args("k", &[0; CERTIFICATE_MOST + 1]).is_err(), "bad arguments refused"); // WHAT: refused. WHY: likewise
        assert_eq!(cell_args("k", b"\x30\x00").unwrap(), vec!["--tls-key", "k", "--tls-cert", "MAA="], "the four words"); // WHAT: the arguments. WHY: as CellOptions reads them
    }

    #[test]
    fn a_cng_p256_public_blob_becomes_the_point_a_certificate_holds() {
        let mut blob = 0x3153_4345u32.to_le_bytes().to_vec(); // WHAT: ECS1. WHY: a P-256 public key
        blob.extend(32u32.to_le_bytes()); // WHAT: 32-byte coordinates. WHY: P-256
        blob.extend([7u8; 64]); // WHAT: X and Y. WHY: any values
        let point = p256_point(&blob).unwrap(); // WHAT: the point. WHY: compared with the certificate's
        assert_eq!(point.len(), 65); // WHAT: 0x04 and 64 bytes. WHY: uncompressed
        assert_eq!(point[0], 4); // WHAT: uncompressed. WHY: as a certificate holds it
        blob[3] = b'3'; // WHAT: another magic, ECS3 (P-384's): the little-endian magic's last byte. WHY: refused
        assert!(p256_point(&blob).is_none(), "only P-256"); // WHAT: refused. WHY: Citadel's keys are P-256
        assert!(p256_point(&[0; 8]).is_none(), "too short"); // WHAT: refused. WHY: never read past the end
    }

    #[test]
    fn a_raw_signature_is_written_as_der_and_checks_out() {
        use ring::signature::{EcdsaKeyPair, KeyPair as _, ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_FIXED_SIGNING};
        let rng = ring::rand::SystemRandom::new(); // WHAT: randomness. WHY: a key and signatures
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap(); // WHAT: a P-256 key. WHY: signs r||s as CNG does
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng).unwrap(); // WHAT: opened. WHY: below
        for n in 0..50u8 {
            let message = [n; 40]; // WHAT: something to sign. WHY: many tries cover high bits and leading zeros
            let raw = key.sign(&rng, &message).unwrap(); // WHAT: r||s. WHY: the form CNG gives
            let der = der_signature(raw.as_ref()).unwrap(); // WHAT: as a certificate holds it. WHY: the conversion tested
            ring::signature::UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, key.public_key().as_ref()).verify(&message, &der).unwrap(); // WHAT: checks out as DER. WHY: a client reads it so
        }
        assert!(der_signature(&[1; 63]).is_none(), "only 64 bytes"); // WHAT: refused. WHY: nothing else is P-256
    }

    #[test]
    fn a_new_office_s_certificate_is_citadel_s_own_shape_signed_by_a_key_held_elsewhere() {
        use ring::signature::{EcdsaKeyPair, KeyPair as _, ECDSA_P256_SHA256_FIXED_SIGNING};
        struct Held(EcdsaKeyPair, Vec<u8>); // WHAT: a key the certificate maker can't see. WHY: as the Office Server's key in Windows' store
        impl rcgen::RemoteKeyPair for Held {
            fn public_key(&self) -> &[u8] {
                &self.1 // WHAT: the point. WHY: what rcgen writes
            }
            fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
                let raw = self.0.sign(&ring::rand::SystemRandom::new(), msg).map_err(|_| rcgen::Error::RemoteKeyError)?; // WHAT: r||s. WHY: as CNG gives
                der_signature(raw.as_ref()).ok_or(rcgen::Error::RemoteKeyError) // WHAT: as DER. WHY: the product's conversion
            }
            fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
                &rcgen::PKCS_ECDSA_P256_SHA256 // WHAT: ECDSA P-256 SHA-256. WHY: Citadel's
            }
        }
        let rng = ring::rand::SystemRandom::new(); // WHAT: randomness. WHY: a key
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap(); // WHAT: a key. WHY: held apart
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng).unwrap(); // WHAT: opened. WHY: below
        let point = key.public_key().as_ref().to_vec(); // WHAT: its point. WHY: rcgen writes it
        let pair = rcgen::KeyPair::from_remote(Box::new(Held(key, point.clone()))).unwrap(); // WHAT: the remote key. WHY: as on Windows
        let cert = certificate_params("LATCON-SERVER_1", time::OffsetDateTime::now_utc()).unwrap().self_signed(&pair).unwrap(); // WHAT: made. WHY: the new office's certificate
        let der = cert.der().to_vec(); // WHAT: its bytes. WHY: checked
        assert!(der.windows(point.len()).any(|w| w == point.as_slice()), "it holds the held key's point"); // WHAT: the SPKI is the key's. WHY: the pin follows the key
        assert!(der.windows(14).any(|w| w == b"Citadel Server"), "CN=Citadel Server"); // WHAT: the subject. WHY: as Citadel's
        assert!(der.windows(14).any(|w| w == b"LATCON-SERVER1"), "the machine's name, cleaned"); // WHAT: the SAN. WHY: SafeName drops odd characters
        let parsed = rustls::pki_types::CertificateDer::from(der.clone()); // WHAT: as a TLS client sees it. WHY: it must parse
        assert!(!parsed.is_empty()); // WHAT: there. WHY: plain check
    }
}
