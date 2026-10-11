# Releasing Excalibur View

## The short version

1. **github.com/excaliburct/excalibur-view** → Actions. Run **Windows build**,
   **Mac build** and **Linux build**, all on the commit you are releasing. About twenty
   minutes, most of the Mac's share being Apple.
2. On the PC, double-click **Release Excalibur View**.
3. Try it on one seat.
4. Double-click **Give it to everyone**.

That is the whole job. The rest of this file is what those four steps are
doing and why, for whoever has to change one of them later.

**The shape of it: GitHub builds both halves. The PC signs and publishes.**

GitHub builds because building is not the thing that has to happen on one
machine. It compiles and tests Windows on Windows and the Mac on a Mac, which
no single computer can do, and it needs nothing installed on anybody's desk.
Apple's credentials live there too, because if one leaked Apple revokes it and
issues another.

The PC signs because `hyperview-signing.key` cannot be revoked. That key says
"this update is safe to install" to every copy of Excalibur View in the world,
and every seat installs what it vouches for by itself. It lives on one
computer and it is used there. **That key, and nothing else, is the reason a
release happens on a particular machine.** It is emphatically not a compiler:
there is no Rust on that PC, there never was, and for most of a day the
release script assumed otherwise and stopped a release with a stack trace.

Building on the PC still works for anybody who does have the toolchain --
leave `--app` and `--server` off and it compiles. That is how a developer cuts
a test release without waiting on CI.

Setting the Mac half up for the first time: `docs/SETUP-THE-MAC-BUILD.md`.

## What each half produces

| Workflow | Runner | Artifact | Holds |
|---|---|---|---|
| Windows build | `windows-latest` | `windows-<version>` | `ExcaliburView.exe`, `ExcaliburView-Server.exe` |
| Mac build | `macos-14` | `mac-<version>` | `ExcaliburView-<version>.dmg`, `ExcaliburView-mac.zip` |
| Linux build | `ubuntu-22.04` | `linux-<version>` | `ExcaliburView-linux` |

Both run the whole test suite first. A release that does not pass its own
tests is not a release, and the build somebody is about to sign and hand to
every seat in a shop is the one worth testing twice.

**Both are checked against the commit being released.** Not by commit id --
that is the easy question and the wrong one, because it refuses a release
whose only change since the build was a line in the release notes. The check
asks whether the two commits build the same thing, comparing `crates`,
`Cargo.toml`, `Cargo.lock` and `third_party`. A change anywhere else cannot
make two different programs.




What is built, and stays built whatever ships the bytes, is the part that makes
an update safe to install: a signed manifest, and a seat that checks it against
keys compiled into itself. A fleet app is another way of getting the file to a
machine. It is not a reason to trust the file.

## The contract a fleet app has to satisfy

Whatever ends up distributing builds, a seat will install one only if all of
this holds.

**The manifest.** JSON, as `hub::update::Release`:

```json
{
  "version": "1.5.0",
  "channel": "stable",
  "platform": "windows-x64",
  "published": "2026-09-18T12:00:00Z",
  "notes": "Dynamic Fill.",
  "download": "/api/v1/update/1.5.0/download?platform=windows-x64",
  "bytes": 48234112,
  "digest": "f42562cf…",
  "signature": "a41c…",
  "key": "mesafab-2026",
  "minimum_api_version": 1
}
```

**What is signed** is not the JSON. Two encoders will not agree on key order or
number formatting, and a signature that depends on how something was printed is
not a signature. It is exactly these bytes:

```
hyperview-release-v1\n
version=<version>\n
channel=<stable|preview>\n
platform=<platform>\n
bytes=<length>\n
digest=<sha256, lower-case hex>\n
minimum_api_version=<n>\n
```

Ed25519 over that, hex-encoded. Release notes and the published date are
deliberately **not** signed: they are presentation, and changing them should not
invalidate a build.

**What a seat checks**, in this order, before anything runs:

1. The key named in `key` is in the list compiled into that build.
2. The SHA-256 of what actually arrived matches `digest`.
3. The length matches `bytes`.
4. The signature verifies against the signing payload above.

Both halves matter. The digest says the download is whole; the signature says
the publisher meant to publish it. A download that matches its digest and
nothing else proves only that whoever wrote the digest also wrote the file.

**When it installs:** on the next launch. Never mid-session. Somebody three
hours into a takeoff does not get restarted.

## The tool

`hyperview-release` does the signing and the checking, and a fleet app can call
it or reimplement it against the contract above.

```
hyperview-release keygen mesafab-2026
hyperview-release public --key hyperview-signing.key
hyperview-release sign   --key hyperview-signing.key --version 1.5.0 \
                         --platform windows-x64 --notes "Dynamic Fill." \
                         Hyperview-1.5.0-Setup.exe
hyperview-release check  Hyperview-1.5.0-Setup.exe manifest.json
```

`check` verifies against the keys **the program** trusts, not against whatever
signed it, so it says what a seat will say.

## The key

Generated on the publisher's own machine, by the publisher. It is the only copy.
Back it up somewhere only they can reach — a password manager or a safe, not the
repository and not a shared drive.

Anyone holding it can ship software to every machine running Hyperview. Losing
it means no future release can be signed by that key; seats go on running what
they have, and a new key is added alongside the old one rather than replacing
it.

The public half goes in `crates/hyperview/src/trust.rs`. An empty list there
means the build refuses every update and says so, which is the right way round:
a program that installs anything when it has been told to trust nothing is worse
than one that installs nothing.

## What the server already does

The Hyperview server has the distribution end of this, and a fleet app can drive
it or replace it:

| | |
|---|---|
| `POST /api/v1/admin/releases` | installer + signed manifest. Refuses one that does not match its manifest, or one with no signature at all. Holds no signing key and signs nothing. |
| `POST /api/v1/admin/releases/{version}/ready` | offer it to the office, or take it back. A release arrives unoffered so one seat can try it first. |
| `POST /api/v1/admin/pin` | hold every seat on one version. A fabricator halfway through a bid package should not have the tool change under them. |
| `GET /api/v1/update/latest` | what a seat asks. |

## The installer

`installer/hyperview.iss` builds it with Inno Setup. It puts the program, the
pdfium library and a `profiles` folder on a machine, and registers Hyperview as
*a* program that opens PDFs rather than taking the association away from
whatever the person already uses.

## Two platforms in one release

A release is one version built twice. `release.json` says so in two shapes at
once, and both have to stay:

```json
{
  "app":  { "platform": "windows-x64", ... },
  "apps": [ { "platform": "windows-x64", ... },
            { "platform": "macos-universal", ... } ],
  "server": { "platform": "windows-x64-server", ... }
}
```

`app` is the Windows build and is exactly where it always was. Every copy of
Excalibur View installed before there was a Mac build reads that one field and
knows nothing about the list beside it. Move it and each of those copies is
stranded on the version it has, with no way of being told about another —
there is no mechanism to reach them except the field they already read. So
`app` is repeated inside `apps` rather than replaced by it, and
`hub::feed::Published::app_for` is what code asks instead of reading a field.

A seat that asks for an update without naming a platform is taken to be a
Windows one (`hub::feed::ASSUMED_PLATFORM`). It was built when there was only
one platform to be, so that is the only thing its silence can mean.

An office server holds **every** platform in a release, not just its own. A
Windows server keeps the Mac build for the Macs in the same office, so a Mac
that turns up next month finds its version already there rather than waiting
on the next look at the feed. It costs one extra download per release in a
shop with no Macs in it.

### Making a Mac release

```
deploy/macos/build.sh
```

on the Mac, which produces two files in `target/macos`:

- `ExcaliburView-<version>.dmg` — what a person downloads and drags into
  Applications.
- `ExcaliburView-mac.zip` — the same program in the form an update unpacks.
  Made with `ditto`, which is the only thing that preserves the symbolic links
  and extended attributes an Apple signature is taken over. A zip made any
  other way unpacks into a program macOS refuses to open.

The manifest names the **zip**, because the manifest is read by the updater.
The disk image goes on the release as well, for the website.

Then, on the computer with the key:

```
python3 tools/publish.py sign --key hyperview-signing.key --version 0.7.0 \
    --app ExcaliburView.exe --server ExcaliburView-Server.exe \
    --mac ExcaliburView-mac.zip --out release.json
python3 tools/publish.py upload --token-file github-token.txt --repo owner/name \
    --version 0.7.0 release.json ExcaliburView.exe ExcaliburView-Server.exe \
    ExcaliburView-mac.zip
```

**Apple has to notarize every Mac build before anyone can install it**, and
`build.sh` waits for that. A Windows release is instant; a Mac one is a few
minutes behind. Nothing in the release path has a "signed but waiting" state,
so the two builds are simply signed together once the Mac one exists.

### Replacing a program that is a folder

Windows replaces one file. macOS replaces `Excalibur View.app` whole —
overwriting the executable inside a signed bundle breaks the signature and
macOS then refuses to open it at all. `hub::update::swap_bundle_in` unpacks the
zip with `ditto`, asks `codesign` whether what came out is really signed,
takes the quarantine mark off with `xattr`, and swaps the two folders with a
rename inside the folder the app already lives in. If the swap fails, what was
working goes back.

An app the person cannot write to — one installed by somebody else, or by an
administrator — cannot replace itself, and says so before downloading anything
rather than after.

### On the Fleet

`(version, platform)` is the identity of a release row, so both builds of one
version sit side by side and the push endpoint carries either. Two things to
know:

- A `.avpkg` is named `hyperview-<version>-<platform>-<wave>.avpkg` and carries
  `platform` on its envelope. Before that, two builds of one version wrote the
  same filename and the second quietly replaced the first on the shelf.
- `POST /api/admin/update/rollback` withdraws **every** platform of the newest
  version, chosen by version number rather than publication date. Send
  `{"platform": "macos-universal"}` to withdraw one build on its own — for the
  case this exists for, where the Mac build is bad and the Windows one is fine.

### What the App Store copy cannot do

Apple does not allow an App Store app to download and run new code, so a copy
from the Mac App Store cannot update itself at all — it updates through the
App Store. That is a second Mac channel, not a variant of this one, and the
App Store build has to hide "Check for Updates" rather than offer something it
cannot do.
