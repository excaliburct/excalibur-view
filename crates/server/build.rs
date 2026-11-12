//! Puts the Excalibur mark, and what this program is, into the server exe.
//!
//! This is the file somebody drops on the office server and double-clicks, so
//! it is the one file in the whole product that gets looked at in Explorer by
//! a person who is not sure what it is. A blank page icon there is a file
//! nobody trusts enough to run. Same mark as the viewer, from the same vector.
//!
//! The icon is generated from the same vector the program draws — see
//! `cargo run -p ui --example makeicon` — so there is one mark, not two that
//! can drift apart.

use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // The day this server was built, which a license's updates are held
    // against when somebody installs a newer server by hand. Worked out every
    // time the build script runs, which includes every change of version; a
    // date that is out of date is an earlier one, so it can only ever be
    // more generous to the customer, never less.
    println!("cargo:rustc-env=HYPERVIEW_BUILT={}", today());

    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }

    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .parent()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_default();
    let icon = root.join("assets").join("hyperview.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    if !icon.exists() {
        // Not a failure. A checkout without the generated icon should still
        // build a working program; it just gets Windows' blank page until
        // somebody runs the generator.
        println!("cargo:warning=assets/hyperview.ico is missing, so the exe will have no icon");
        return;
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let script = out.join("hyperview-server.rc");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let (a, b, c) = split(&version);

    // The version block is what Windows shows on the Details tab of the file's
    // properties, and what an administrator looks at when six machines are
    // supposed to be on the same build and one of them is not.
    let rc = format!(
        r#"#include <winver.h>
1 ICON "{icon}"

VS_VERSION_INFO VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
FILEOS VOS__WINDOWS32
FILETYPE VFT_APP
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "Excalibur Construction Technologies"
            VALUE "FileDescription", "Excalibur View Server \0"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "hyperview-server"
            VALUE "OriginalFilename", "hyperview-server.exe"
            VALUE "ProductName", "Excalibur View Server"
            VALUE "ProductVersion", "{version}"
            VALUE "LegalCopyright", "Excalibur Construction Technologies"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#,
        icon = icon.display().to_string().replace('\\', "\\\\"),
    );
    std::fs::write(&script, rc).expect("could not write the resource script");

    let object = out.join("hyperview-server-resource.o");
    if !compile(&script, &object, &target) {
        println!("cargo:warning=no Windows resource compiler found, so the exe will have no icon");
        return;
    }
    println!("cargo:rustc-link-arg-bins={}", object.display());
}

/// Tries the resource compilers that might be on this machine: the one MSVC
/// builds use, then the mingw ones a cross-build from Linux has.
fn compile(script: &Path, object: &Path, target: &str) -> bool {
    let candidates: &[&str] = if target.contains("msvc") {
        &["rc.exe", "llvm-rc"]
    } else {
        &[
            "x86_64-w64-mingw32-windres",
            "x86_64-w64-mingw32ucrt-windres",
            "windres",
            "llvm-windres",
        ]
    };
    for tool in candidates {
        let run = if tool.ends_with("rc.exe") || *tool == "llvm-rc" {
            std::process::Command::new(tool)
                .arg(format!("/fo{}", object.display()))
                .arg(script)
                .status()
        } else {
            std::process::Command::new(tool)
                .args(["-I", script.parent().unwrap_or(Path::new(".")).to_str().unwrap_or(".")])
                .arg(script)
                .arg("-O")
                .arg("coff")
                .arg("-o")
                .arg(object)
                .status()
        };
        if matches!(run, Ok(status) if status.success()) {
            return true;
        }
    }
    false
}

/// "1.4.2" as three numbers, for the fields Windows wants as numbers.
fn split(version: &str) -> (u16, u16, u16) {
    let mut parts = version.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// Today's date in UTC as YYYY-MM-DD, from the system clock alone.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs().cast_signed()); // WHAT: the seconds since 1970, signed for the day sums below, nought for a clock before it. WHY: exactly what `as i64` gave; a clock past the year 292 billion is not a concern
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2); // WHAT: the year, one more for January and February. WHY: the civil year starts in March in this algorithm
    format!("{year:04}-{month:02}-{day:02}")
}
