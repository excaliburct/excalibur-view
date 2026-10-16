//! Signing a Hyperview release.
//!
//! The private key is made on the publisher's own machine by `keygen` and never
//! goes anywhere else — not into the repository, not onto the build machine
//! unless the publisher puts it there themselves, and never near this program's
//! authors. Everything else here is either a signature over a release or a
//! check of one.
//!
//! Why this exists at all: a Hyperview server can hand six machines an
//! installer, and nothing about running a server should be enough to decide
//! what software those machines run. The signature is what separates
//! "distributing" from "deciding".
//!
//! ```text
//!   hyperview-release keygen mesafab-2026
//!   hyperview-release sign  --key hyperview-signing.key \
//!                           --version 1.5.0 --platform windows-x64 \
//!                           --notes "Dynamic Fill." Hyperview-Setup.exe
//!   hyperview-release check Hyperview-Setup.exe manifest.json
//! ```
//!
//! With the key on a hardware security key (a YubiKey) instead of in a file,
//! `sign` is done in two halves either side of the computer the key is plugged
//! into: `prepare` (everything but the signature) and `finish` (the signature
//! checked and the manifest written). See [`hardware_key`].

mod hardware_key;

use std::path::Path;
use std::process::ExitCode;

use ed25519_dalek::{Signer, SigningKey};
use hub::update::{hex, unhex, Channel, Release, Trusted};
use base64::Engine;
use sha2::{Digest, Sha256};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(|s| s.as_str()) {
        Some("keygen") => keygen(&args[1..]),
        Some("sign") => sign(&args[1..]),
        Some("prepare") => prepare(&args[1..]),
        Some("finish") => finish(&args[1..]),
        Some("check") => check(&args[1..]),
        Some("bundle") => bundle(&args[1..]),
        Some("public") => public(&args[1..]),
        // Double-clicked rather than typed. That is how this gets used exactly
        // once, by the person setting up signing for the first time, and a
        // usage message in a console window that closes again is no use to
        // them.
        None => {
            let outcome = walkthrough();
            wait();
            return match outcome {
                Ok(()) => ExitCode::SUCCESS,
                Err(message) => {
                    eprintln!("\n  {message}\n");
                    ExitCode::FAILURE
                }
            };
        }
        _ => {
            usage();
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("\n  {message}\n");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!(
        "\n\
         Signing Excalibur View releases.\n\n\
           keygen <name>                       make a signing key. Once, on your own machine.\n\
           public --key <file>                 print the public half, to paste into the program.\n\
           sign   --key <file> --version <v>   sign an installer and write its manifest.\n\
                  [--platform windows-x64] [--channel stable] [--notes \"...\"]\n\
                  [--out manifest.json] <installer>\n\
           check  <installer> <manifest.json>  verify one, the way a seat will.\n\
         \n\
         With the key on a hardware key (a YubiKey):\n\
           prepare --slot <slot> --version <v> sign, up to the signature: writes a signing\n\
                  [--platform ...] [--channel ...]  request, kept with the installer in a new\n\
                  [--notes \"...\"] [--out-dir .]    folder, for the computer the key is in.\n\
                  [--commit <id>] <installer>\n\
           finish --request <kept request>     check what came back signed and write the\n\
                  --signatures <file>          manifest, exactly as sign would have.\n\
                  [--out manifest.json]\n\
         \n\
           bundle --version <v> <installer> <manifest.json>\n\
                  [--note \"...\"] [--wave stable] [--out <file>.avpkg]\n\
                                               wrap a signed release as a Fleet package,\n\
                                               ready to put on the shelf.\n"
    );
}

fn value(args: &[String], name: &str) -> Option<String> {
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1).cloned()
}

fn positional(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for (i, arg) in args.iter().enumerate() {
        if skip {
            skip = false;
            continue;
        }
        if arg.starts_with("--") {
            // Every flag here takes a value.
            skip = true;
            let _ = i;
            continue;
        }
        out.push(arg.clone());
    }
    out
}

// ---- making a key ---------------------------------------------------------

/// What somebody gets when they double-click this.
///
/// One job: make the signing key, and make it obvious what to do with each
/// half. The private half is written to a file and never printed, so it cannot
/// end up in a screenshot of this window. The public half is both printed and
/// written to a file, because it is meant to be copied and sent on, and asking
/// somebody to retype sixty-four hex characters is asking for a typo nobody
/// will find for a week.
fn walkthrough() -> Result<(), String> {
    println!();
    println!("  EXCALIBUR HYPERVIEW — SIGNING KEY");
    println!();
    println!("  This makes the key that says a release really came from you.");
    println!();
    println!("  It comes in two halves.");
    println!();
    println!("    The private half stays on this computer, in a file. It is what");
    println!("    signs a release. Anyone who has it can ship software to every");
    println!("    machine running Excalibur View, so it belongs in a password manager");
    println!("    or a safe — never in the repository, never on a shared drive,");
    println!("    and never pasted into a chat window, including to me.");
    println!();
    println!("    The public half goes into the program, so every seat can check");
    println!("    a release was signed by you. That half is not a secret. Send it");
    println!("    on, print it, put it on a wall.");
    println!();
    println!("  Nothing here goes over a network.");
    println!();

    let year = this_year();
    let suggested = format!("mesafab-{year}");
    println!("  What should this key be called? A release records the name of the");
    println!("  key that signed it, so a key can be replaced later without every");
    println!("  older build refusing every newer release.");
    println!();
    print!("  Name [{suggested}]: ");
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let mut typed = String::new();
    let _ = std::io::stdin().read_line(&mut typed);
    let name = match typed.trim() {
        "" => suggested,
        given => given.to_string(),
    };

    keygen(&[name.clone()])?;

    // Beside the key, because the next thing that happens is somebody needs to
    // send this line on and a file is easier to attach than a console window is
    // to copy out of.
    let signing = Path::new("hyperview-signing.key");
    let text = std::fs::read_to_string(signing)
        .map_err(|e| format!("could not read the key back: {e}"))?;
    let secret = text.lines().nth(1).unwrap_or("").trim().to_string();
    let bytes = unhex(&secret).ok_or("the key file is not readable")?;
    let key: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| "the key file is not the right length")?;
    let publicly = hex(&SigningKey::from_bytes(&key).verifying_key().to_bytes());

    let share = Path::new("hyperview-public-key.txt");
    let _ = std::fs::write(
        share,
        format!(
            "Excalibur View signing key — the PUBLIC half.\n\
             \n\
             This one is safe to send on. It goes into the program so every seat\n\
             can check that a release was signed by the matching private half.\n\
             \n\
             name        {name}\n\
             public key  {publicly}\n\
             \n\
             As it appears in the program:\n\
             \n\
             (\"{name}\", \"{publicly}\"),\n"
        ),
    );

    println!("  ----------------------------------------------------------------");
    println!();
    println!("  DONE. Two files are now in this folder:");
    println!();
    println!("    hyperview-signing.key       KEEP THIS. Do not send it anywhere.");
    println!("    hyperview-public-key.txt    Send this one on.");
    println!();
    println!("  Back the first one up now, while you are thinking about it.");
    println!("  Losing it means no future release can be signed by this key.");
    println!();
    Ok(())
}

/// Holds a console window open that Windows would otherwise close the instant
/// this finishes, taking everything it said with it.
fn wait() {
    println!("  Press Enter to close.");
    let mut anything = String::new();
    let _ = std::io::stdin().read_line(&mut anything);
}

fn this_year() -> u64 {
    // Enough of a year for a default somebody is about to be shown and can
    // overtype. Not worth a date library.
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    1970 + seconds / 31_556_952
}

fn keygen(args: &[String]) -> Result<(), String> {
    let name = args
        .first()
        .cloned()
        .ok_or("Give the key a name, so a release can say which key signed it: keygen mesafab-2026")?;
    let path = Path::new("hyperview-signing.key");
    if path.exists() {
        return Err(format!(
            "{} already exists. Making a new key over the top of an old one would strand \
             every release the old one signed, so this will not do it. Move it out of the \
             way first if you really mean to.",
            path.display()
        ));
    }

    let signing = SigningKey::generate(&mut rand::rngs::OsRng);
    let secret = hex(&signing.to_bytes());
    let publicly = hex(&signing.verifying_key().to_bytes());

    // Written as a file, not printed, so it does not end up in a terminal
    // history or a screenshot.
    std::fs::write(path, format!("{name}\n{secret}\n"))
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    restrict(path);

    println!();
    println!("  A signing key called '{name}' is in {}.", path.display());
    println!();
    println!("  This is the only copy. Back it up somewhere only you can reach —");
    println!("  a password manager or a safe, not the repository and not a shared");
    println!("  drive. Anyone with this file can ship software to every machine");
    println!("  running Excalibur View, and losing it means no future release can be");
    println!("  signed by this key.");
    println!();
    println!("  The public half goes in the program, in crates/hyperview/src/trust.rs:");
    println!();
    println!("      (\"{name}\", \"{publicly}\"),");
    println!();
    Ok(())
}

/// Locks a key file down to its owner where the platform has a way to.
#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {}

fn public(args: &[String]) -> Result<(), String> {
    let (name, signing) = load_key(args)?;
    println!("(\"{name}\", \"{}\"),", hex(&signing.verifying_key().to_bytes()));
    Ok(())
}

fn load_key(args: &[String]) -> Result<(String, SigningKey), String> {
    let path = value(args, "--key").unwrap_or_else(|| "hyperview-signing.key".into());
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("could not read {path}: {e}"))?;
    let mut lines = text.lines();
    let name = lines
        .next()
        .ok_or("that key file is empty")?
        .trim()
        .to_string();
    let secret = lines.next().ok_or("that key file has no key in it")?.trim();
    let bytes = unhex(secret).ok_or("that key file's key is not readable")?;
    if bytes.len() != 32 {
        return Err("that key file's key is the wrong length".into());
    }
    let mut fixed = [0u8; 32];
    fixed.copy_from_slice(&bytes);
    Ok((name, SigningKey::from_bytes(&fixed)))
}

// ---- signing --------------------------------------------------------------

/// What `sign` and `prepare` are asked to sign, read off the command line.
struct ToSign {
    version: String,
    platform: String,
    channel: Channel,
    notes: String,
    installer: String,
    bytes: Vec<u8>,
}

fn what_to_sign(args: &[String]) -> Result<ToSign, String> {
    let version = value(args, "--version")
        .ok_or("say which version this is: --version 1.5.0")?;
    if hub::update::compare(&version, "0.0.0") == std::cmp::Ordering::Equal
        && version != "0.0.0"
    {
        return Err(format!(
            "'{version}' is not a version a seat can compare. Use major.minor.patch."
        ));
    }
    let platform = value(args, "--platform").unwrap_or_else(|| "windows-x64".into());
    let channel = match value(args, "--channel").as_deref() {
        Some("preview") => Channel::Preview,
        Some("stable") | None => Channel::Stable,
        Some(other) => return Err(format!("'{other}' is not a channel. Use stable or preview.")),
    };
    let notes = value(args, "--notes").unwrap_or_default();

    let installer = positional(args)
        .first()
        .cloned()
        .ok_or("say which installer to sign")?;
    let bytes = std::fs::read(&installer)
        .map_err(|e| format!("could not read {installer}: {e}"))?;
    if bytes.is_empty() {
        return Err(format!("{installer} is empty"));
    }
    Ok(ToSign { version, platform, channel, notes, installer, bytes })
}

/// The manifest for an installer, complete but for its signature.
fn describe(asked: &ToSign, published: String, key: &str) -> Release {
    let (version, platform) = (&asked.version, &asked.platform);
    Release {
        version: version.clone(),
        channel: asked.channel,
        platform: platform.clone(),
        published,
        notes: asked.notes.clone(),
        download: format!("/api/v1/update/{version}/download?platform={platform}"),
        bytes: asked.bytes.len() as u64,
        digest: hex(&Sha256::digest(&asked.bytes)),
        signature: String::new(),
        key: key.to_string(),
        minimum_api_version: hub::API_VERSION,
    }
}

/// How a signed manifest is written, whichever way it was signed.
fn manifest_text(release: &Release) -> Result<String, String> {
    serde_json::to_string_pretty(release).map_err(|e| e.to_string())
}

fn sign(args: &[String]) -> Result<(), String> {
    let (name, signing) = load_key(args)?;
    let asked = what_to_sign(args)?;
    let out = value(args, "--out").unwrap_or_else(|| "manifest.json".into());

    let mut release = describe(&asked, stamp(), &name);
    release.signature = hex(&signing.sign(&release.signing_payload()).to_bytes());

    // Check our own work before anybody else has to.
    let trusted = Trusted {
        keys: vec![(name.clone(), signing.verifying_key().to_bytes())],
    };
    trusted
        .check(&release, &asked.bytes)
        .map_err(|e| format!("the signature this just made does not check out: {e}"))?;

    let text = manifest_text(&release)?;
    std::fs::write(&out, text).map_err(|e| format!("could not write {out}: {e}"))?;

    println!();
    println!("  Signed {} for {} with '{name}'.", release.version, release.platform);
    how_to_publish(&release, &out, &asked.installer);
    Ok(())
}

fn how_to_publish(release: &Release, out: &str, installer: &str) {
    let (version, platform) = (&release.version, &release.platform);
    println!("    {} bytes", release.bytes);
    println!("    sha256 {}", release.digest);
    println!("    manifest {out}");
    println!();
    println!("  Publish both to a server:");
    println!();
    println!("      curl -X POST https://your-server/api/v1/admin/releases \\");
    println!("           -H \"Authorization: Bearer $TOKEN\" \\");
    println!("           -F manifest=@{out} -F file=@{installer}");
    println!();
    println!("  Nobody is offered it until it is marked ready. Try it on one seat first:");
    println!();
    println!("      curl -X POST https://your-server/api/v1/admin/releases/{version}/ready \\");
    println!("           -H \"Authorization: Bearer $TOKEN\" \\");
    println!("           -H 'Content-Type: application/json' \\");
    println!("           -d '{{\"platform\":\"{platform}\",\"ready\":true}}'");
    println!();
}

/// `sign`, stopped where the key file would sign. See [`hardware_key`].
fn prepare(args: &[String]) -> Result<(), String> {
    let asked = what_to_sign(args)?;
    let slot = value(args, "--slot").ok_or(
        "say which slot on the hardware key holds the release key: --slot, as two hex digits",
    )?;
    let commit = value(args, "--commit").unwrap_or_else(commit_here);
    let out_dir = value(args, "--out-dir").unwrap_or_else(|| ".".into());
    let installer_name = Path::new(&asked.installer)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("that installer has no usable file name")?
        .to_string();
    let trusted = trusted_keys()?;
    let made = stamp();
    let key_name = value(args, "--key-name");
    let prepared = hardware_key::prepare(
        &trusted,
        &hardware_key::How {
            slot: &slot,
            key_name: key_name.as_deref(),
            commit: &commit,
            made: &made,
            out_dir: Path::new(&out_dir),
        },
        describe(&asked, made.clone(), ""),
        &installer_name,
        &asked.bytes,
    )?;

    let request_name = prepared
        .request
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    println!();
    println!(
        "  Prepared {} for {}. Nothing is signed yet.",
        asked.version, asked.platform
    );
    println!("    kept in  {}", prepared.folder.display());
    println!("    request  {request_name}");
    println!();
    println!("  Copy the request to the computer the hardware key is plugged into and");
    println!("  sign it there. Then, with the signatures file that comes back:");
    println!();
    println!(
        "      hyperview-release finish --request \"{}\" --signatures {}",
        prepared.request.display(),
        request_name.replace(".sign-request.json", ".signatures.json")
    );
    println!();
    Ok(())
}

/// The manifest `sign` would have written, from a kept request and the
/// signatures that came back for it.
fn finish(args: &[String]) -> Result<(), String> {
    let request = value(args, "--request")
        .ok_or("say which signing request prepare kept: --request")?;
    let signatures = value(args, "--signatures")
        .ok_or("say which signatures file came back: --signatures")?;
    let trusted = trusted_keys()?;
    let finished =
        hardware_key::finish(&trusted, Path::new(&request), Path::new(&signatures))?;
    let out = value(args, "--out")
        .unwrap_or_else(|| finished.folder.join("manifest.json").display().to_string());
    let text = manifest_text(&finished.release)?;
    std::fs::write(&out, text).map_err(|e| format!("could not write {out}: {e}"))?;

    println!();
    println!(
        "  Signed {} for {} with '{}', on the hardware key.",
        finished.release.version, finished.release.platform, finished.release.key
    );
    how_to_publish(&finished.release, &out, &finished.installer.display().to_string());
    Ok(())
}

/// The commit being built, in full, or "" when git cannot say.
fn commit_here() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_lowercase())
        .unwrap_or_default()
}

/// Wraps a signed release as a package the Excalibur Fleet can deliver.
///
/// The Fleet hands the `payload` to a shop's server verbatim and never looks
/// inside it — that is the whole point of the arrangement. The server checks
/// the signature against its own pinned keys, and so does every seat, so a
/// Fleet with a compromised shelf can offer only builds that were already
/// signed by the key held here.
///
/// The installer travels inside the package rather than being fetched later,
/// so what was signed and what arrives are the same journey.
fn bundle(args: &[String]) -> Result<(), String> {
    let version = value(args, "--version")
        .ok_or("say which version this is: --version 1.5.0")?;
    let files = positional(args);
    let installer = files
        .first()
        .ok_or("say which installer to bundle")?;
    let manifest = files
        .get(1)
        .cloned()
        .unwrap_or_else(|| "manifest.json".into());

    let bytes = std::fs::read(installer)
        .map_err(|e| format!("could not read {installer}: {e}"))?;
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("could not read {manifest}: {e}"))?;
    let release: Release = serde_json::from_str(&text)
        .map_err(|e| format!("{manifest} is not a release manifest: {e}"))?;

    // Checked here rather than taken on trust: a package that cannot be
    // installed is worse on a shelf than not on one, because it looks ready.
    let trusted = trusted_keys()?;
    trusted
        .check(&release, &bytes)
        .map_err(|e| format!("{manifest} does not check out against {installer}: {e}"))?;
    if release.version != version {
        return Err(format!(
            "{manifest} says version {}, and you asked to bundle {version}. \
             One of them is wrong.",
            release.version
        ));
    }

    let wave = value(args, "--wave").unwrap_or_else(|| release.channel_name().to_string());
    let note = value(args, "--note").unwrap_or_else(|| {
        if release.notes.trim().is_empty() {
            format!("Excalibur View {version}")
        } else {
            release.notes.clone()
        }
    });
    // The platform is in the name because one version is now two builds, and
    // two packages landing on the shelf under one name is the second one
    // quietly replacing the first.
    let out = value(args, "--out")
        .unwrap_or_else(|| format!("hyperview-{version}-{}-{wave}.avpkg", release.platform));

    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let package = serde_json::json!({
        "fleet": {
            "product": "hyperview",
            "version": version,
            // On the envelope rather than only inside the payload, so the
            // Fleet can tell a Windows package from a Mac one without opening
            // a payload it is meant to pass along unread.
            "platform": release.platform,
            "wave": wave,
            "note": note,
            "files": 1,
            "createdAt": stamp(),
        },
        "payload": {
            "release": release,
            "installer": encoded,
            // Staged rather than offered. One seat tries it, and somebody
            // decides. A shop of six does not find out about a new version by
            // all six of them restarting into it at once.
            "ready": false,
        }
    });
    let text = serde_json::to_string(&package).map_err(|e| e.to_string())?;
    std::fs::write(&out, &text).map_err(|e| format!("could not write {out}: {e}"))?;

    println!();
    println!("  Bundled {version} as {out}.");
    println!("    {} bytes of installer, {} bytes of package", bytes.len(), text.len());
    println!("    signed by '{}'", release.key);
    println!();
    println!("  Put it on the Fleet shelf:");
    println!();
    println!("      copy \"{out}\" \"%USERPROFILE%\\dev\\ExcaliburFleet\\native\\dist\\releases\\\"");
    println!();
    println!("  Then deliver it to one install from the Fleet, and try it on one seat");
    println!("  before the rest of the office is offered it.");
    println!();
    Ok(())
}

// ---- checking -------------------------------------------------------------

fn check(args: &[String]) -> Result<(), String> {
    let files = positional(args);
    let installer = files.first().ok_or("say which installer to check")?;
    let manifest = files
        .get(1)
        .cloned()
        .unwrap_or_else(|| "manifest.json".into());

    let bytes = std::fs::read(installer).map_err(|e| format!("could not read {installer}: {e}"))?;
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| format!("could not read {manifest}: {e}"))?;
    let release: Release =
        serde_json::from_str(&text).map_err(|e| format!("{manifest} is not readable: {e}"))?;

    // Checked against the keys the program itself trusts, so this says what a
    // seat will say and not merely that the file is self-consistent.
    let trusted = trusted_keys()?;
    match trusted.check(&release, &bytes) {
        Ok(()) => {
            println!();
            println!("  {} {} for {} is good.", release.version, release.channel_name(), release.platform);
            println!("    signed by '{}'", release.key);
            println!("    sha256 {}", release.digest);
            println!();
            Ok(())
        }
        Err(refusal) => Err(refusal.to_string()),
    }
}

/// The keys the shipping program trusts, read from the same file it compiles in
/// so this command cannot approve something a seat would refuse.
fn trusted_keys() -> Result<Trusted, String> {
    let mut keys = Vec::new();
    for (name, publicly) in hyperview_trust::KEYS {
        let bytes = unhex(publicly).ok_or_else(|| format!("the key '{name}' is not readable"))?;
        if bytes.len() != 32 {
            return Err(format!("the key '{name}' is the wrong length"));
        }
        let mut fixed = [0u8; 32];
        fixed.copy_from_slice(&bytes);
        keys.push((name.to_string(), fixed));
    }
    if keys.is_empty() {
        return Err(
            "This build trusts no signing keys, so it would refuse every update. Run \
             `hyperview-release keygen <name>` and paste the public half into \
             crates/hyperview/src/trust.rs."
                .into(),
        );
    }
    Ok(Trusted { keys })
}

/// The trusted key list, shared with the program itself.
mod hyperview_trust {
    #![allow(dead_code)]
    include!("../../hyperview/src/trust.rs");
}

fn stamp() -> String {
    // No date library here; a release is stamped by the build, and seconds do
    // not matter. Falls back to the epoch rather than failing a release.
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = seconds / 86_400;
    let (year, month, day) = civil(days as i64);
    let rest = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// Days since the epoch to a calendar date. Howard Hinnant's algorithm.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

trait ChannelName {
    fn channel_name(&self) -> &'static str;
}

impl ChannelName for Release {
    fn channel_name(&self) -> &'static str {
        match self.channel {
            Channel::Stable => "stable",
            Channel::Preview => "preview",
        }
    }
}
