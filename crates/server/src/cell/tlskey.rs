//! The Windows half of `crate::cellkey`: the cell's TLS key made, imported,
//! opened, checked and locked in Windows' own key store (the Microsoft
//! Software Key Storage Provider, machine keys), as the system.
//!
//! Everything decided is decided in `crate::cellkey`: the plan, the
//! permissions, who may own the key, the names and the certificate's shape.
//! This file only hands those to Windows.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows_sys::Win32::Security::Cryptography::{
    CertCloseStore, CertCreateCertificateContext, CertEnumCertificatesInStore, CertFreeCertificateContext, CertGetCertificateContextProperty, CryptUnprotectData, NCryptCreatePersistedKey, NCryptExportKey, NCryptFinalizeKey, NCryptFreeObject, NCryptGetProperty, NCryptOpenKey, NCryptOpenStorageProvider, NCryptSetProperty, NCryptSignHash, PFXImportCertStore, BCRYPT_ECCPUBLIC_BLOB, BCRYPT_ECDSA_P256_ALGORITHM, CERT_CONTEXT, CERT_KEY_PROV_INFO_PROP_ID, CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB, CRYPT_KEY_PROV_INFO, CRYPT_MACHINE_KEYSET, MS_KEY_STORAGE_PROVIDER, NCRYPT_EXPORT_POLICY_PROPERTY, NCRYPT_MACHINE_KEY_FLAG, NCRYPT_SECURITY_DESCR_PROPERTY, NCRYPT_SILENT_FLAG, PKCS12_ALWAYS_CNG_KSP, PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
};
use windows_sys::Win32::Security::{GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorLength, GetSecurityDescriptorOwner, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, INHERITED_ACE, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED};

use crate::cellkey::{self, Plan};

/// What the cell is handed: its key's name and the certificate.
pub struct Tls {
    pub name: String, // WHAT: the key's name in the machine's key store. WHY: the cell opens it by name
    pub certificate: Vec<u8>, // WHAT: the certificate, DER. WHY: the cell serves it with the key
}

/// A string as Windows wants it: UTF-16, ended by a zero.
fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect() // WHAT: UTF-16 and its end. WHY: every W call takes this
}

/// A CNG handle, let go when dropped.
struct Ncrypt(usize); // WHAT: an NCRYPT_HANDLE (a provider or a key). WHY: freed once, whatever path returns

impl Drop for Ncrypt {
    fn drop(&mut self) {
        if self.0 != 0 {
            // SAFETY: the handle came from NCryptOpenStorageProvider, NCryptOpenKey or NCryptCreatePersistedKey and is freed once here.
            unsafe { NCryptFreeObject(self.0) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: freeing our own CNG handle
        }
    }
}

/// A CNG status as a refusal in words.
fn status(what: &str, code: i32) -> Result<(), String> {
    if code == 0 { Ok(()) } else { Err(format!("{what} failed (0x{code:08X})")) } // WHAT: ok, or said with Windows' code. WHY: the alarm says which step
}

/// The cell's key, ready: made, imported or opened as the plan says, owned
/// by the system or administrators, locked to the system and the cell's SID,
/// and matching the certificate.
///
/// # Errors
///
/// When any step fails; the cell is then not started (said as an alarm by the caller).
pub fn ready(home: &Path, data: &Path, cell_sid: &str) -> Result<Tls, String> {
    let kept = cellkey::folder(home); // WHAT: `<service folder>\citadel-tls`. WHY: the system's and administrators' only (it inherits the locked service folder)
    std::fs::create_dir_all(&kept).map_err(|e| format!("could not make {} ({e})", kept.display()))?; // WHAT: made once. WHY: the key's name and certificate are kept there
    let (name_file, cert_file) = (kept.join(cellkey::KEY_NAME_FILE), kept.join(cellkey::CERTIFICATE_FILE)); // WHAT: the two kept files. WHY: read below
    let kept_name = std::fs::read_to_string(&name_file).ok().map(|t| t.trim().to_string()); // WHAT: the key's name, if kept. WHY: the plan
    let kept_cert = std::fs::read(&cert_file).ok(); // WHAT: the certificate, if kept. WHY: likewise
    let pfx = data.join(cellkey::PFX); // WHAT: 1.0's tls.pfx, where the move left it. WHY: imported once
    let pfx_there = std::fs::symlink_metadata(&pfx).is_ok(); // WHAT: whether anything is there by that name. WHY: the plan; a link is refused when read
    let started_before = std::fs::symlink_metadata(data.join(cellkey::STARTED_MARK)).is_ok(); // WHAT: whether the cell has served before. WHY: then a lost key is never quietly replaced
    let plan = cellkey::plan(pfx_there, kept_name.is_some(), kept_cert.is_some(), started_before)?; // WHAT: what to do. WHY: crate::cellkey decides
    let provider = provider()?; // WHAT: the Software KSP. WHY: every step below uses it
    let sddl = cellkey::key_sddl(cell_sid); // WHAT: the key's permissions. WHY: set and checked
    let (name, certificate, fresh) = match plan {
        Plan::Import => {
            let (name, certificate) = import(&pfx)?; // WHAT: the key into the machine's store, its certificate out. WHY: the first start after the move
            keep(&name_file, name.as_bytes())?; // WHAT: the name kept first. WHY: a start cut short after this reuses the key rather than importing again
            keep(&cert_file, &certificate)?; // WHAT: the certificate kept. WHY: handed to the cell at every start
            move_out(&pfx, &kept); // WHAT: tls.pfx moved out of the cell's reach. WHY: the cell only ever uses the key, never holds it
            (name, certificate, true)
        }
        Plan::Make => {
            let name = cellkey::new_key_name(rand::random()); // WHAT: a new, unguessable name. WHY: nobody can plant a key under it first
            let certificate = make(&provider, &name, &sddl)?; // WHAT: the key, made already locked, and its certificate. WHY: a new office
            keep(&name_file, name.as_bytes())?; // WHAT: kept. WHY: as above
            keep(&cert_file, &certificate)?; // WHAT: kept. WHY: as above
            (name, certificate, false)
        }
        Plan::Reuse | Plan::ReuseAndIgnorePfx => {
            if plan == Plan::ReuseAndIgnorePfx {
                tracing::error!("ALARM: {} is in the Citadel cell's folder, but this office already holds the cell's key; the file was not used", pfx.display()); // WHAT: said. WHY: something put it there after the key was made
            }
            (kept_name.unwrap_or_default(), kept_cert.unwrap_or_default(), false)
        }
    };
    if !cellkey::is_key_name(&name) {
        return Err(format!("the kept key's name in {} isn't a plain name", name_file.display())); // WHAT: refused. WHY: never passed to Windows or the cell
    }
    let key = open(&provider, &name)?; // WHAT: the key, by name. WHY: checked below
    lock(&key, cell_sid, &sddl, fresh)?; // WHAT: owner checked, permissions ours (set back if changed). WHY: only the cell's SID may use it
    matches(&key, &certificate)?; // WHAT: the key is the certificate's. WHY: the cell would fail to serve otherwise, and the pin is the key's
    Ok(Tls { name, certificate })
}

/// Writes a kept file whole: a new file beside it, then renamed over it.
fn keep(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let part = path.with_extension("part"); // WHAT: the new copy's name. WHY: renamed into place
    std::fs::write(&part, bytes).map_err(|e| format!("could not write {} ({e})", part.display()))?; // WHAT: written. WHY: in the locked folder, which only the system and administrators can change
    std::fs::rename(&part, path).map_err(|e| format!("could not keep {} ({e})", path.display())) // WHAT: in place. WHY: never half written
}

/// Moves `tls.pfx` out of the cell's folder into the kept folder (as
/// `tls.pfx`, or `tls-<seconds>.pfx` beside an older one). Said in the log
/// when it can't be.
fn move_out(pfx: &Path, kept: &Path) {
    let moved = (|| -> std::io::Result<std::path::PathBuf> {
        let bytes = read_plain(pfx)?; // WHAT: read again, never through a link. WHY: copied whole
        let mut to = kept.join(cellkey::PFX); // WHAT: where it is kept. WHY: the Office Server's own folder
        if to.exists() {
            to = kept.join(format!("tls-{}.pfx", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))); // WHAT: a new name. WHY: an older one is never written over
        }
        std::fs::OpenOptions::new().write(true).create_new(true).open(&to).and_then(|mut f| std::io::Write::write_all(&mut f, &bytes).and_then(|()| f.sync_all()))?; // WHAT: a new file, inheriting the locked folder's permissions. WHY: never readable by the cell
        std::fs::remove_file(pfx)?; // WHAT: the cell's copy gone. WHY: the cell only uses the key
        Ok(to)
    })();
    match moved {
        Ok(to) => tracing::warn!("Citadel Server 1.0's tls.pfx was imported into the machine's key store for the cell, and kept as {}", to.display()), // WHAT: said. WHY: an administrator knows where it went
        Err(e) => tracing::error!("ALARM: {} was imported, but could not be moved out of the cell's folder ({e})", pfx.display()), // WHAT: said. WHY: the cell could read its key's file
    }
}

/// Reads a small file the cell can change, never through a link: opened
/// without following a reparse point and checked on that same handle.
fn read_plain(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _; // WHAT: reading. WHY: below
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _}; // WHAT: Windows' attributes and open flags. WHY: below
    let file = std::fs::OpenOptions::new().read(true).custom_flags(0x0020_0000).open(path)?; // WHAT: FILE_FLAG_OPEN_REPARSE_POINT. WHY: a link is opened as itself, never followed
    let meta = file.metadata()?; // WHAT: what was opened. WHY: checked on the handle read
    if !meta.is_file() || meta.file_attributes() & 0x400 != 0 || meta.len() > cellkey::PFX_MOST {
        return Err(std::io::Error::other(format!("{} is not a plain file of at most 64 KiB", path.display()))); // WHAT: refused. WHY: the system never reads through a link the cell made
    }
    let mut bytes = Vec::new(); // WHAT: the contents. WHY: returned
    file.take(cellkey::PFX_MOST + 1).read_to_end(&mut bytes)?; // WHAT: read, bounded. WHY: as above
    Ok(bytes)
}

/// The Microsoft Software Key Storage Provider.
fn provider() -> Result<Ncrypt, String> {
    let mut handle = 0usize; // WHAT: filled by Windows. WHY: the provider
    // SAFETY: the out pointer is a live local; the provider name is Windows' own constant.
    let code = unsafe { NCryptOpenStorageProvider(&mut handle, MS_KEY_STORAGE_PROVIDER, 0) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: opening the software key store
    status("opening Windows' software key store", code)?;
    Ok(Ncrypt(handle))
}

/// A machine key, by name.
fn open(provider: &Ncrypt, name: &str) -> Result<Ncrypt, String> {
    let text = wide(name); // WHAT: the name. WHY: as Windows takes it
    let mut key = 0usize; // WHAT: filled by Windows. WHY: the key
    // SAFETY: the provider is live; the name is a zero-ended UTF-16 local; the out pointer is a live local.
    let code = unsafe { NCryptOpenKey(provider.0, &mut key, text.as_ptr(), 0, NCRYPT_MACHINE_KEY_FLAG | NCRYPT_SILENT_FLAG) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: opening the cell's machine key
    status(&format!("opening the Citadel cell's key {name}"), code)?;
    Ok(Ncrypt(key))
}

/// Imports 1.0's DPAPI-sealed `tls.pfx` into the machine's key store
/// (non-exportable, through CNG), and returns the key's name, as Windows
/// named it, and the certificate.
fn import(pfx: &Path) -> Result<(String, Vec<u8>), String> {
    let sealed = read_plain(pfx).map_err(|e| format!("could not read {} ({e})", pfx.display()))?; // WHAT: the sealed file. WHY: opened below
    let mut opened = unprotect(&sealed)?; // WHAT: the PKCS #12. WHY: imported below
    let found = import_opened(&opened); // WHAT: imported. WHY: below
    opened.iter_mut().for_each(|b| *b = 0); // WHAT: the key's bytes wiped. WHY: never left in memory
    found
}

/// The work of [`import`], on the opened PKCS #12.
fn import_opened(pkcs12: &[u8]) -> Result<(String, Vec<u8>), String> {
    let blob = CRYPT_INTEGER_BLOB { cbData: u32::try_from(pkcs12.len()).map_err(|_| "tls.pfx is too big")?, pbData: pkcs12.as_ptr().cast_mut() }; // WHAT: the PKCS #12. WHY: as the call takes it
    let empty = wide(""); // WHAT: an empty password. WHY: tried after none, as Windows wants the exact one used
    let mut store = std::ptr::null_mut(); // WHAT: the temporary store. WHY: filled below
    for password in [std::ptr::null(), empty.as_ptr()] {
        // SAFETY: the blob points at `pkcs12`, alive for the call; the password is null or a zero-ended local.
        store = unsafe { PFXImportCertStore(&blob, password, CRYPT_MACHINE_KEYSET | PKCS12_ALWAYS_CNG_KSP) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: importing 1.0's certificate and key as machine keys, not exportable
        if !store.is_null() {
            break; // WHAT: imported. WHY: one password matched
        }
    }
    if store.is_null() {
        return Err(format!("Windows couldn't import tls.pfx ({})", std::io::Error::last_os_error())); // WHAT: refused. WHY: said
    }
    let mut found: Result<(String, Vec<u8>), String> = Err("tls.pfx holds no certificate with a key".to_string()); // WHAT: the answer so far. WHY: one certificate with a key is expected
    let mut seen = 0; // WHAT: certificates with a key. WHY: exactly one
    let mut context: *mut CERT_CONTEXT = std::ptr::null_mut(); // WHAT: the current certificate. WHY: enumerated
    loop {
        // SAFETY: the store is live; the previous context is ours and is freed by this call.
        context = unsafe { CertEnumCertificatesInStore(store, context) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: walking the imported certificates
        if context.is_null() {
            break; // WHAT: the end. WHY: all looked at
        }
        if let Some(name) = machine_key_name(context) {
            seen += 1; // WHAT: counted. WHY: exactly one
            // SAFETY: the context is live; its encoded bytes are valid for cbCertEncoded bytes.
            let der = unsafe { std::slice::from_raw_parts((*context).pbCertEncoded, (*context).cbCertEncoded as usize) }.to_vec(); // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the certificate's bytes copied out
            found = Ok((name, der)); // WHAT: the key's name and the certificate. WHY: returned
        }
    }
    // SAFETY: the store is ours, closed once; the enumeration ended, so no context is held.
    unsafe { CertCloseStore(store, 0) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: closing the temporary store
    if seen > 1 {
        return Err("tls.pfx holds more than one certificate with a key".to_string()); // WHAT: refused. WHY: which the computers pinned would be a guess
    }
    found
}

/// The name of a certificate's key, when it is a machine key in the Software
/// KSP; none otherwise.
fn machine_key_name(context: *const CERT_CONTEXT) -> Option<String> {
    let mut size = 0u32; // WHAT: the property's size. WHY: asked first
    // SAFETY: the context is live; a null buffer asks for the size.
    if unsafe { CertGetCertificateContextProperty(context, CERT_KEY_PROV_INFO_PROP_ID, std::ptr::null_mut(), &mut size) } == 0 || size == 0 { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the size of the key's provider information
        return None; // WHAT: no key. WHY: a certificate alone
    }
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)]; // WHAT: room, aligned. WHY: the structure is read in place
    // SAFETY: the buffer holds `size` bytes, aligned for the structure.
    if unsafe { CertGetCertificateContextProperty(context, CERT_KEY_PROV_INFO_PROP_ID, buffer.as_mut_ptr().cast(), &mut size) } == 0 { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the key's provider information
        return None;
    }
    // SAFETY: Windows wrote a CRYPT_KEY_PROV_INFO at the buffer's start; its strings point into the same buffer.
    let info = unsafe { &*buffer.as_ptr().cast::<CRYPT_KEY_PROV_INFO>() }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: reading the provider information
    let text = |p: *const u16| -> String {
        if p.is_null() {
            return String::new(); // WHAT: none. WHY: never read
        }
        // SAFETY: a zero-ended UTF-16 string inside the buffer.
        let len = (0..).take_while(|i| unsafe { *p.add(*i) } != 0).count(); // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: its length
        // SAFETY: as above, `len` units long.
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) }) // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the string read
    };
    let provider = text(info.pwszProvName); // WHAT: the provider's name. WHY: the Software KSP only
    (provider == "Microsoft Software Key Storage Provider" && info.dwFlags & CRYPT_MACHINE_KEYSET != 0).then(|| text(info.pwszContainerName)) // WHAT: the key's name, for a machine key in the Software KSP. WHY: the only kind the cell is given
}

/// Opens a DPAPI blob sealed for this computer with Citadel's purpose.
fn unprotect(sealed: &[u8]) -> Result<Vec<u8>, String> {
    let input = CRYPT_INTEGER_BLOB { cbData: u32::try_from(sealed.len()).map_err(|_| "tls.pfx is too big")?, pbData: sealed.as_ptr().cast_mut() }; // WHAT: the sealed bytes. WHY: as the call takes them
    let entropy = CRYPT_INTEGER_BLOB { cbData: u32::try_from(cellkey::PFX_PURPOSE.len()).unwrap_or(0), pbData: cellkey::PFX_PURPOSE.as_ptr().cast_mut() }; // WHAT: Citadel's purpose. WHY: the file opens only with it
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() }; // WHAT: filled by Windows. WHY: the opened bytes
    // SAFETY: every pointer is to a live local or to `sealed`; the output is freed once below with LocalFree.
    let ok = unsafe { CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN | CRYPTPROTECT_LOCAL_MACHINE, &mut output) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: opening 1.0's sealed certificate file, as the system
    if ok == 0 || output.pbData.is_null() {
        return Err(format!("Windows couldn't open tls.pfx ({})", std::io::Error::last_os_error())); // WHAT: refused. WHY: said
    }
    // SAFETY: Windows wrote cbData bytes at pbData; they are copied, wiped and freed once.
    let opened = unsafe {
        let bytes = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize); // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the opened bytes
        let copy = bytes.to_vec(); // WHAT: our copy. WHY: Windows' is wiped and freed next
        bytes.iter_mut().for_each(|b| *b = 0); // WHAT: wiped. WHY: never left in freed memory
        LocalFree(output.pbData.cast()); // WHAT: let go. WHY: Windows allocated it
        copy
    };
    Ok(opened)
}

/// Makes a new P-256 machine key named `name`, locked with `sddl` before it
/// is finished (so it never exists with any other permissions), not
/// exportable; and a certificate for it in Citadel's own shape, signed with it.
fn make(provider: &Ncrypt, name: &str, sddl: &str) -> Result<Vec<u8>, String> {
    let text = wide(name); // WHAT: the name. WHY: as Windows takes it
    let mut handle = 0usize; // WHAT: filled by Windows. WHY: the new key
    // SAFETY: the provider is live; the algorithm is Windows' constant; the name is a zero-ended local; the out pointer is live.
    let code = unsafe { NCryptCreatePersistedKey(provider.0, &mut handle, BCRYPT_ECDSA_P256_ALGORITHM, text.as_ptr(), 0, NCRYPT_MACHINE_KEY_FLAG) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: a new P-256 machine key
    status("making the Citadel cell's key", code)?;
    let key = Ncrypt(handle); // WHAT: owned. WHY: freed whatever happens
    set_permissions(&key, sddl)?; // WHAT: locked before it exists on disk. WHY: never any other permissions
    let policy = 0u32.to_le_bytes(); // WHAT: no export at all. WHY: the key never leaves Windows' store
    // SAFETY: the key is live; the property name is Windows' constant; the value is a live local of 4 bytes.
    status("making the cell's key not exportable", unsafe { NCryptSetProperty(key.0, NCRYPT_EXPORT_POLICY_PROPERTY, policy.as_ptr(), 4, 0) })?; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the export policy
    // SAFETY: the key is live.
    status("finishing the cell's key", unsafe { NCryptFinalizeKey(key.0, NCRYPT_SILENT_FLAG) })?; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the key written to the store
    let point = public_point(&key)?; // WHAT: its public point. WHY: the certificate holds it
    let held = rcgen::KeyPair::from_remote(Box::new(Held { key: handle, point })).map_err(|e| format!("the cell's certificate: {e}"))?; // WHAT: the key as rcgen signs with it. WHY: the private key never leaves Windows
    let machine = std::env::var("COMPUTERNAME").unwrap_or_default(); // WHAT: this computer's name. WHY: the certificate's first name, as Citadel's
    let made = cellkey::certificate_params(&machine, time::OffsetDateTime::now_utc())?.self_signed(&held).map_err(|e| format!("the cell's certificate: {e}"))?; // WHAT: the certificate. WHY: a new office
    drop(held); // WHAT: rcgen's hold let go before the key's handle. WHY: the handle is freed with `key`
    Ok(made.der().to_vec())
}

/// The key, as rcgen signs with it: the handle and its public point.
struct Held {
    key: usize, // WHAT: the key's handle, owned by the caller. WHY: signs
    point: Vec<u8>, // WHAT: 0x04 ‖ X ‖ Y. WHY: rcgen writes it into the certificate
}

impl rcgen::RemoteKeyPair for Held {
    fn public_key(&self) -> &[u8] {
        &self.point // WHAT: the point. WHY: as rcgen wants it
    }

    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
        use sha2::Digest as _; // WHAT: SHA-256. WHY: ECDSA signs the hash
        let hash = sha2::Sha256::digest(msg); // WHAT: the message's SHA-256. WHY: ECDSA with SHA-256
        let mut raw = [0u8; 64]; // WHAT: r ‖ s. WHY: P-256's size
        let mut size = 0u32; // WHAT: filled by Windows. WHY: checked
        // SAFETY: the key is live while the caller holds it; every buffer is a live local of the size given.
        let code = unsafe { NCryptSignHash(self.key, std::ptr::null(), hash.as_ptr(), 32, raw.as_mut_ptr(), 64, &mut size, NCRYPT_SILENT_FLAG) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: signing the new certificate with the cell's key
        if code != 0 || size != 64 {
            return Err(rcgen::Error::RemoteKeyError); // WHAT: refused. WHY: no certificate without a good signature
        }
        cellkey::der_signature(&raw).ok_or(rcgen::Error::RemoteKeyError) // WHAT: as a certificate holds it. WHY: DER
    }

    fn algorithm(&self) -> &'static rcgen::SignatureAlgorithm {
        &rcgen::PKCS_ECDSA_P256_SHA256 // WHAT: ECDSA P-256 SHA-256. WHY: Citadel's
    }
}

/// A key's public point, read from Windows.
fn public_point(key: &Ncrypt) -> Result<Vec<u8>, String> {
    let mut blob = [0u8; 8 + 64]; // WHAT: room for a P-256 public blob. WHY: anything bigger isn't one
    let mut size = 0u32; // WHAT: filled by Windows. WHY: checked
    // SAFETY: the key is live; the buffer is a live local of the size given.
    let code = unsafe { NCryptExportKey(key.0, 0, BCRYPT_ECCPUBLIC_BLOB, std::ptr::null(), blob.as_mut_ptr(), 72, &mut size, 0) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the public half only
    status("reading the cell's key's public half", code)?;
    cellkey::p256_point(&blob[..(size as usize).min(72)]).ok_or_else(|| "the Citadel cell's key isn't a P-256 key".to_string()) // WHAT: the point. WHY: P-256 only
}

/// Sets a key's permissions from SDDL: protected when Windows takes that
/// flag, its DACL only.
fn set_permissions(key: &Ncrypt, sddl: &str) -> Result<(), String> {
    let text = wide(sddl); // WHAT: the SDDL. WHY: as Windows takes it
    let mut descriptor: *mut core::ffi::c_void = std::ptr::null_mut(); // WHAT: filled by Windows. WHY: the parsed SDDL
    // SAFETY: the SDDL is a zero-ended local; the descriptor Windows allocates is freed once below.
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(text.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) } == 0 { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the key's permissions parsed
        return Err(format!("the key's permissions don't read ({})", std::io::Error::last_os_error())); // WHAT: refused. WHY: never set without them
    }
    // SAFETY: the descriptor is the self-relative one Windows just made.
    let length = unsafe { GetSecurityDescriptorLength(descriptor) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: its length
    let mut code = 0; // WHAT: the last result. WHY: said
    for flags in [DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, DACL_SECURITY_INFORMATION] {
        // SAFETY: the key is live; the descriptor is `length` bytes long and alive until freed below.
        code = unsafe { NCryptSetProperty(key.0, NCRYPT_SECURITY_DESCR_PROPERTY, descriptor.cast::<u8>(), length, flags) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the key's permissions set
        if code == 0 {
            break; // WHAT: set. WHY: done; the check after reads what Windows kept
        }
    }
    // SAFETY: allocated by Windows above, freed once.
    unsafe { LocalFree(descriptor) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the parsed descriptor let go
    status("setting the Citadel cell's key's permissions", code)
}

/// A key's owner and permissions, as Windows reports them.
fn permissions(key: &Ncrypt) -> Result<(String, bool, Vec<cellkey::Entry>), String> {
    let which = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION; // WHAT: the owner and the DACL. WHY: both are checked
    let mut size = 0u32; // WHAT: the descriptor's size. WHY: asked first
    // SAFETY: the key is live; a null buffer asks for the size.
    status("reading the cell's key's permissions", unsafe { NCryptGetProperty(key.0, NCRYPT_SECURITY_DESCR_PROPERTY, std::ptr::null_mut(), 0, &mut size, which) })?; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the descriptor's size
    let mut buffer = vec![0u64; (size as usize).div_ceil(8).max(1)]; // WHAT: room, aligned. WHY: read in place
    // SAFETY: the buffer holds at least `size` bytes.
    status("reading the cell's key's permissions", unsafe { NCryptGetProperty(key.0, NCRYPT_SECURITY_DESCR_PROPERTY, buffer.as_mut_ptr().cast(), size, &mut size, which) })?; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the descriptor
    let descriptor: *mut core::ffi::c_void = buffer.as_mut_ptr().cast(); // WHAT: the self-relative descriptor. WHY: read below
    // SAFETY: the descriptor is the one Windows wrote into `buffer`, alive for this whole block; every out pointer is a live local;
    // the ACL and SIDs read point into `buffer`; each SID string Windows allocates is freed once.
    unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: reading the key's owner and DACL
        let (mut control, mut revision) = (0u16, 0u32); // WHAT: filled by Windows. WHY: whether the DACL is protected
        GetSecurityDescriptorControl(descriptor, &mut control, &mut revision);
        let protected = control & SE_DACL_PROTECTED != 0; // WHAT: protected. WHY: nothing inherited
        let (mut owner, mut defaulted): (PSID, i32) = (std::ptr::null_mut(), 0); // WHAT: filled by Windows. WHY: the owner
        GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted);
        let owner = sid_text(owner); // WHAT: the owner as text. WHY: compared
        let (mut present, mut dacl) = (0i32, std::ptr::null_mut::<ACL>()); // WHAT: filled by Windows. WHY: the DACL
        GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted);
        let mut entries = Vec::new(); // WHAT: the entries. WHY: compared
        if present == 0 || dacl.is_null() {
            entries.push(cellkey::Entry { allow: true, inherited: false, mask: u32::MAX, sid: "S-1-1-0".into() }); // WHAT: no DACL means everyone, in full. WHY: written as that, so it never passes as ours
        } else {
            for i in 0..u32::from((*dacl).AceCount) {
                let mut ace: *mut core::ffi::c_void = std::ptr::null_mut(); // WHAT: the entry. WHY: read below
                if GetAce(dacl, i, &mut ace) == 0 || ace.is_null() {
                    continue; // WHAT: unreadable. WHY: the count then won't match ours
                }
                let header = *ace.cast::<ACE_HEADER>(); // WHAT: its kind and flags. WHY: below
                let allow = header.AceType == 0; // WHAT: ACCESS_ALLOWED_ACE_TYPE. WHY: anything else isn't ours
                let (mask, sid) = if header.AceType <= 1 {
                    let body = ace.cast::<ACCESS_ALLOWED_ACE>(); // WHAT: an allow or deny entry (the same shape). WHY: its mask and SID
                    ((*body).Mask, sid_text((&raw mut (*body).SidStart).cast())) // WHAT: rights and who. WHY: compared
                } else {
                    (0, String::new()) // WHAT: some other kind. WHY: never ours
                };
                entries.push(cellkey::Entry { allow, inherited: u32::from(header.AceFlags) & INHERITED_ACE != 0, mask, sid });
            }
        }
        Ok((owner, protected, entries))
    }
}

/// An SID as text, or empty.
///
/// # Safety
///
/// `sid` must be null or point at a valid SID.
unsafe fn sid_text(sid: PSID) -> String {
    if sid.is_null() {
        return String::new(); // WHAT: none. WHY: never matches
    }
    let mut text: *mut u16 = std::ptr::null_mut(); // WHAT: filled by Windows. WHY: the SID string
    // SAFETY: the caller's SID; the string Windows allocates is read and freed once.
    unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: an SID written as text
        if ConvertSidToStringSidW(sid, &mut text) == 0 || text.is_null() {
            return String::new(); // WHAT: unreadable. WHY: never matches
        }
        let len = (0..).take_while(|i| *text.add(*i) != 0).count(); // WHAT: its length. WHY: zero-ended
        let out = String::from_utf16_lossy(std::slice::from_raw_parts(text, len)); // WHAT: as text. WHY: compared
        LocalFree(text.cast()); // WHAT: let go. WHY: Windows allocated it
        out
    }
}

/// Checks the key's owner and permissions; sets the permissions back when
/// they aren't ours (an alarm unless the key was only just imported), and
/// checks again. A key owned by anyone but the system or administrators is
/// refused, never fixed: whoever owns it could always grant itself access.
fn lock(key: &Ncrypt, cell_sid: &str, sddl: &str, fresh: bool) -> Result<(), String> {
    let (owner, protected, entries) = permissions(key)?; // WHAT: as it is. WHY: checked
    if !cellkey::owner_ok(&owner) {
        return Err(format!("the Citadel cell's key is owned by {owner}, not the system or administrators, so it isn't used")); // WHAT: refused. WHY: someone else made or took it
    }
    if cellkey::locked_as_ours(protected, &entries, cell_sid) {
        return Ok(()); // WHAT: as set. WHY: nothing to do
    }
    if !fresh {
        tracing::error!("ALARM: the Citadel cell's key's permissions had been changed ({entries:?}, protected {protected}); they were set back to the system and the cell only"); // WHAT: said. WHY: somebody changed them
    }
    set_permissions(key, sddl)?; // WHAT: set back. WHY: only the cell's SID may use it
    let (owner, protected, entries) = permissions(key)?; // WHAT: read again. WHY: what Windows kept is what counts
    if cellkey::owner_ok(&owner) && cellkey::locked_as_ours(protected, &entries, cell_sid) {
        Ok(())
    } else {
        Err(format!("the Citadel cell's key's permissions couldn't be set to the system and the cell only (Windows kept {entries:?}, protected {protected})")) // WHAT: refused. WHY: the cell is never started on a key others can use
    }
}

/// Checks that the key is the one the certificate holds.
fn matches(key: &Ncrypt, certificate: &[u8]) -> Result<(), String> {
    let point = public_point(key)?; // WHAT: the key's point. WHY: compared
    let size = u32::try_from(certificate.len()).map_err(|_| "the kept certificate is too big")?; // WHAT: its length. WHY: as the call takes it
    // SAFETY: the bytes are alive for the call; the context Windows makes is freed once below.
    let context = unsafe { CertCreateCertificateContext(X509_ASN_ENCODING | PKCS_7_ASN_ENCODING, certificate.as_ptr(), size) }; // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the kept certificate parsed
    if context.is_null() {
        return Err("the kept certificate doesn't read".to_string()); // WHAT: refused. WHY: nothing to serve
    }
    // SAFETY: the context is live; its public key bits are valid for cbData bytes; freed once after.
    let same = unsafe { // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage -- reviewed Windows FFI: the certificate's public key read
        let bits = &(*(*context).pCertInfo).SubjectPublicKeyInfo.PublicKey; // WHAT: the certificate's key. WHY: compared
        let held = std::slice::from_raw_parts(bits.pbData, bits.cbData as usize) == point.as_slice(); // WHAT: the same point. WHY: the key is the certificate's
        CertFreeCertificateContext(context); // WHAT: let go. WHY: Windows allocated it
        held
    };
    if same { Ok(()) } else { Err("the Citadel cell's key isn't the one its certificate holds".to_string()) } // WHAT: refused. WHY: the computers' pins are the certificate's
}
