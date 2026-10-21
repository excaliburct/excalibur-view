# Excalibur View on iPad and Android tablets

The tablet apps are the same program as the desktop one. `crates/hyperview` is
built into them unchanged, in its store edition (see `src/edition.rs`), with
a thin app around it on each system: `crates/mobile` is the door each system
comes in through, `mobile/android` and `mobile/ios` are the apps.

## What a tablet has, and what it doesn't

Everything the desktop program does is there: drawing sets, every markup
tool, measurement and takeoff, the markups list and its exports, Edit Text,
Cloud+, signatures, document and batch jobs, compare and overlay, sealing,
the 3D model and its tonnage, tool chests, and the office server.

What changes on a tablet:

- **Files.** A tablet has no system dialog to borrow, so the program shows its
  own list of the drawings in the app's folder (`src/files.rs`), with
  **Bring in from this device…** for anything elsewhere, and **Send a copy…**
  (press and hold a file) to send one out. On an iPad the folder is the app's
  Documents, which the Files app shows as Excalibur View's. A PDF opened
  *with* Excalibur View from Mail, Files or a browser opens as a tab.
- **Fingers.** Two fingers move and zoom the sheet with any tool in hand; a
  touch that becomes two fingers takes back whatever its first finger started.
  One finger on bare paper with the Select tool moves the sheet. Press and
  hold is right-click. Sheets are picked in Thumbnails by pressing and holding
  one, then tapping others. Controls come a size up, in three sizes chosen
  in Preferences.
- **The system's own.** Printing, the share sheet (Email attaches the file for
  real), links, and OCR go through the tablet's own system: Vision on an iPad,
  ML Kit on Android, both on the device (`src/platform.rs`).
- **Left out.** Updating itself (the store does it), buying or entering a
  license (an office's license lives on its server), running plugins on the
  device (the office server runs them), hosting the office server (a tablet
  joins one), and connecting a desktop assistant app to the program.

`HYPERVIEW_TABLET=1` makes a desktop build behave like a tablet — the list of
files, the larger controls, the tablet's hints — which is how the tablet side
is seen and tested without a tablet.

## Android

```
mobile/android/build.sh
```

Needs Rust with `aarch64-linux-android` and `x86_64-linux-android`, the NDK
r28 or later, the Android SDK with platform 36 and build tools 36.1.0, and
JDK 17 or later. The script's header says where each is looked for.

It leaves in `dist/android`:

- `excalibur-view-<version>-debug.apk` — install on a tablet with
  `adb install`, or copy it over and open it
- `excalibur-view-<version>.aab` — the bundle Play takes
- `native-debug-symbols-<version>.zip` — upload beside the bundle

Or on GitHub: **Actions → Android build → Run workflow**.

Release signing uses Play's upload key, from `EXV_KEYSTORE`,
`EXV_KEYSTORE_PASSWORD`, `EXV_KEY_ALIAS` and `EXV_KEY_PASSWORD`, or the
matching `exv.*` Gradle properties; the workflow reads them from secrets of
the same names. Without them the bundle is built unsigned. Play re-signs
what it delivers with a key of its own.

Every native library is linked for 16 KB pages, which Play requires of apps
that target Android 15 and later; `llvm-readelf -lW` shows `Align 0x4000` on
every LOAD segment.

## iPad

On a Mac with Xcode 26 or later, Rust with `aarch64-apple-ios` and
`aarch64-apple-ios-sim`, and XcodeGen (`brew install xcodegen`):

```
mobile/ios/build.sh
```

That builds the Rust library for an iPad and for the Simulator, wraps the PDF
engine as a framework, writes `mobile/ios/ExcaliburView.xcodeproj`, and
makes a Simulator build. Open the project in Xcode to run it on the Simulator
or on an iPad on a cable.

`STEPS="native project simulator archive upload"` also makes the App Store
archive and sends it to App Store Connect, where it appears in TestFlight. An
archive is signed by Xcode's automatic signing, which needs `TEAM_ID` and an
App Store Connect API key (`API_KEY_PATH`, `API_KEY_ID`, `API_ISSUER`) with
the App Manager or Admin role.

Or on GitHub: **Actions → iPad build → Run workflow**, with **archive** and
**upload** ticked to send it to TestFlight. It uses the App Store Connect key
the Mac build already has (`APPLE_API_KEY_P8`, `APPLE_API_KEY_ID`,
`APPLE_API_ISSUER`) and reads the team from `MACOS_SIGN_IDENTITY`. A key made
for notarizing alone may have the Developer role; if the archive step says it
is not allowed to make certificates or profiles, make a new key with App
Manager in App Store Connect → Users and Access → Integrations → Keys and
replace those three secrets.

The app is iPad-only, iPadOS 17 or later (the PDF engine's minimum).

## What has been checked, and what hasn't

- Android: builds to an APK and a Play bundle, alignment and packaging
  checked. Not yet run on a device.
- iPad: the Rust side type-checks for `aarch64-apple-ios`. The Swift, the
  Xcode project and the build script have not yet run on a Mac.
- The tablet list of files, the larger controls and the tablet hints run and
  have been looked at on a desktop in `HYPERVIEW_TABLET=1`. Touch itself —
  pinching, press and hold, the soft keyboard — can only be tried on a
  tablet.

## Known gaps

- **The iPad and the iOS 27 SDK.** From April 2027 the App Store takes only
  apps built with the iOS 27 SDK, and an app built with it must use UIKit's
  scene life cycle or it will not launch. winit 0.30, which eframe draws the
  window with, does not use scenes yet (winit issue 4224, planned for 0.31).
  Before then: move to a winit and eframe that do, or host the program in a
  scene-based view of our own.
- **Apple Pencil.** Pencil strokes arrive as touches with pressure. Hover,
  tilt and double-tap need winit 0.31, which egui has not moved to.
- **Hardware keyboards on Android** type through winit as on a desktop; if a
  keyboard also feeds the soft keyboard's box, a letter could arrive twice.
  Worth trying first on a tablet with a keyboard cover.
- **egui 0.32.** 0.36 is current. It adds safe-area insets on iOS and saves
  battery by not redrawing when nothing changed. The upgrade moves wgpu from
  25 to 30, which the 3D view's own device has to follow.
