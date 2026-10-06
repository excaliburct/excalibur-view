# Getting Excalibur View onto the Mac

One-time setup, then one command per release. Do all of it on the M1.

## Where this stands

Done already:

- Team ID `C27WQYHJXB`, in the build script.
- `Developer ID Application: Your Name (TEAMID)` — installed,
  paired with its private key, chain verified, backed up as a `.p12`.
- The notary profile `excalibur-notary`, stored in the keychain and validated.
- Xcode command line tools.

Still outstanding: a **1024x1024 PNG** of the icon, so the app has one rather
than the generic blank sheet.

## 0. The source

```
cd ~/dev
git clone https://github.com/excaliburct/excalibur-view.git
cd excalibur
```

Private repo — sign in with the GitHub token, not your account password.

## 1. Tools

```
xcode-select --install
rustup target add aarch64-apple-darwin x86_64-apple-darwin
```

## 2. The certificate

developer.apple.com → Certificates → **+** → **Developer ID Application**.
(Not "Mac App Distribution" — that's the App Store one, for later.)

It'll ask for a certificate signing request. Keychain Access → Certificate
Assistant → Request a Certificate From a Certificate Authority → save to disk.
Upload that, download the certificate, double-click to install.

Check it landed:

```
security find-identity -v -p codesigning
```

You want a line reading `Developer ID Application: <name> (TEAMID)`. Send me
that line — it's public.

## 3. The notary key

App Store Connect → Users and Access → **Integrations** → App Store Connect
API → **+**. Access: Developer. Download the `.p8` — **you get one chance**,
it cannot be downloaded twice.

Put it somewhere only you can read:

```
mkdir -p ~/private && chmod 700 ~/private
mv ~/Downloads/AuthKey_*.p8 ~/private/
chmod 600 ~/private/AuthKey_*.p8
```

Note the **Key ID** and the **Issuer ID** off that page.

## 4. Store it in the keychain

So no secret is ever in a script or in your shell history:

```
xcrun notarytool store-credentials excalibur-notary \
  --key ~/private/AuthKey_XXXXXXXX.p8 \
  --key-id XXXXXXXX \
  --issuer XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX
```

From here on the build script just says `--keychain-profile excalibur-notary`.

## 5. PDFium

The Mac needs its own copy of the PDF engine, and the universal build covers
both architectures in one file. From the root of the repository:

```
mkdir -p third_party/pdfium/embedded
curl -L -o /tmp/pdfium-mac.tgz https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/8066/pdfium-mac-univ.tgz
tar xzf /tmp/pdfium-mac.tgz -C /tmp
cp /tmp/lib/libpdfium.dylib third_party/pdfium/embedded/libpdfium.dylib
lipo -info third_party/pdfium/embedded/libpdfium.dylib
```

That last line should say `arm64 x86_64`. `build.rs` looks for `pdfium.dll`,
`libpdfium.dylib` or `libpdfium.so` depending on the platform. Without it the
program still builds and simply says it needs the library beside it.

`third_party/` is deliberately not in the repository — it is somebody else's
BSD-licensed binary, not ours to redistribute, and it would bloat every clone.

## 6. Build

```
export SIGN_IDENTITY="Developer ID Application: <your name> (TEAMID)"
deploy/macos/build.sh
```

It builds both architectures, joins them into one universal binary, makes the
`.app`, signs it, builds a `.dmg`, sends it to Apple, waits, and staples the
result. Out comes `target/macos/ExcaliburView-<version>.dmg`.

## 7. Prove it works on a Mac that has never seen it

This is the step people skip and then find out from a customer:

```
xattr -w com.apple.quarantine '0081;00000000;Safari;' target/macos/ExcaliburView-*.dmg
open target/macos/ExcaliburView-*.dmg
```

That marks the file as downloaded from the internet, which is the only state
where Gatekeeper actually judges it. It should open with no warning at all —
not "unidentified developer", not "are you sure". If it warns, something in
signing or stapling didn't take, and it's better you find that than a
fabricator does.

## What's different on a Mac

- **No installer.** Drag to Applications. That's the whole thing.
- **Updates still work.** The signed-release mechanism is yours, not Apple's.
  This is the main reason we're not going through the App Store first.
- **Sealed is a managed preference**, not a registry key. Your customer's MDM
  pushes `com.excaliburct.view.plist` with `Sealed` set true, to
  `/Library/Managed Preferences/`. An administrator on one machine can do it
  by hand:

  ```
  sudo defaults write /Library/Preferences/com.excaliburct.view Sealed -bool true
  ```

  I've built and tested the reader for both the XML and binary forms.
- **The Office server** runs the same way but is started by launchd rather than
  installed as a Windows service. There's already a "run it yourself" fallback
  message; a proper `.plist` for launchd is a small job once you want it.
- **OCR is off on the Mac for now.** Windows has a recogniser built in;
  macOS's equivalent is the Vision framework and it needs writing. Everything
  else works. Worth saying on the download page rather than letting somebody
  find out.

## Later, if you want the App Store

Nothing here is wasted. You'd add: a sandboxed build, an "Apple Distribution"
certificate, entitlements for the local network and user-selected files, and a
version with the self-update path compiled out. The free one-person app is the
only part that makes sense in there — the Office server can't go in at all.
