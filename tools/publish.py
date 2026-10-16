#!/usr/bin/env python3
"""Publishing a Hyperview release: signing it, and putting it on GitHub.

Runs on the publisher's own computer, beside the signing key, because the key
never goes anywhere else. Standard library plus `cryptography` for Ed25519.

    python3 publish.py sign     --key hyperview-signing.key --version 0.5.0 \\
                                --channel preview --notes-file notes.txt \\
                                --app ExcaliburView.exe --server ExcaliburView-Server.exe \\
                                --mac ExcaliburView-mac.zip \\
                                --out release.json
    python3 publish.py upload   --token-file github-token.txt --repo owner/name \\
                                --version 0.5.0 --notes-file notes.txt \\
                                release.json Hyperview.exe Hyperview-Server.exe
    python3 publish.py release  --key hyperview-signing.key --token-file github-token.txt \\
                                --repo owner/name --notes-file notes.txt \\
                                --mac-zip ExcaliburView-mac.zip --mac-dmg ExcaliburView-mac.dmg
    python3 publish.py promote  --key hyperview-signing.key --token-file github-token.txt \\
                                --repo owner/name --version 0.5.0

`release` is the whole thing in one command: tests, build, sign, upload,
publish. The three commands above it are its parts, for when one of them has
to be done on its own.
    python3 publish.py make-key    --name excalibur-plugins-2026 --out plugin-signing.key
    python3 publish.py sign-plugin --key plugin-signing.key --id acme-estimating \\
                                --version 1.0.0 --name "Acme Estimating" \\
                                acme_estimating.wasm

A release goes up as `preview` first — a GitHub prerelease, which only an
office set to "get them early" takes — and is promoted to `stable` once it has
been tried. The channel is part of what is signed, so promoting re-signs the
manifest; the program files themselves do not change.

The signed text must match `hub::update::Release::signing_payload` byte for
byte. There is a test in the Rust code that checks a manifest this script made.

An Excalibur View Office license is signed with the same key too, into a
`.evlicense` file the customer adds under Studio -> Office -> License. Its
signed text starts with a different first line from a release's, so one can
never pass for the other, and must match `hub::license::License::signing_payload`:

    python3 publish.py sign-license --key hyperview-signing.key \\
                                --company "Acme Steel, Inc." --users 6 --years 1
    python3 publish.py sign-license --key hyperview-signing.key \\
                                --company "Example Steel, Inc." --founding
    python3 publish.py sign-license --key hyperview-signing.key --product citadel \\
                                --company "Example Builders" --founding

A plugin is signed into a `.hvplugin` file that an office administrator adds
from the Plugins menu. Its signed text must match `plugin_api::signing_payload`,
and a test in the Rust code checks a plugin this script sealed. Sign plugins
with a key made for plugins alone (`make-key`, then its public half pasted into
PLUGIN_KEYS in trust.rs), not the release key: a plugin key can seal a plugin
and nothing else, so it can be used as often as plugins need signing.

When the release key is on a hardware security key (a YubiKey) rather than in
a file, `sign`, `release` and `promote` each take `--prepare --slot SLOT` in
place of `--key`, and later `--finish REQUEST --signatures FILE`. Prepare does
everything up to the moment of signing and writes down the exact bytes to be
signed; the computer the key is plugged into signs them; finish checks the
signatures against the key the program trusts and carries on exactly as
`--key` would have. See "Signing on a hardware key" in docs/RELEASING.md.

    python3 publish.py release  --prepare --slot SLOT --notes-file notes.txt \\
                                --app ExcaliburView.exe --server ExcaliburView-Server.exe
    python3 publish.py release  --finish dist/hyperview-0.5.0-release/hyperview-0.5.0-release.sign-request.json \\
                                --signatures hyperview-0.5.0-release.signatures.json \\
                                --token-file github-token.txt --repo owner/name
"""

import argparse
import base64
import binascii
import datetime
import hashlib
import json
import os
import re
import shutil
import sys
import time
import urllib.error
import subprocess
import urllib.request

APP_PLATFORM = "windows-x64"
SERVER_PLATFORM = "windows-x64-server"
MAC_PLATFORM = "macos-universal"
LINUX_PLATFORM = "linux-x64"
# What each program is called on the release. Every installed copy finds its
# update by the name in release.json, not by a name of its own, so these are
# also the names the website's download buttons look for. (Up to 0.6.1 they
# were Hyperview.exe and Hyperview-Server.exe; the files on this computer
# can be called anything - they are matched to the manifest by fingerprint.)
APP_FILE = "ExcaliburView.exe"
SERVER_FILE = "ExcaliburView-Server.exe"
# A Mac gets two files and they are not interchangeable. The .dmg is what a
# person downloads and drags into Applications. The .zip is the same program
# in the form an update can unpack without breaking Apple's signature over it,
# and it is the one the manifest names, because the manifest is read by the
# updater and not by a person.
MAC_FILE = "ExcaliburView-mac.zip"
# The disk image is not in the manifest -- nothing updates to it, a person
# downloads it -- so it is the one file whose name nothing else decides. It
# gets a fixed one anyway, for the same reason the programs have fixed ones:
# the website looks every asset up by name, and a name with a version in it
# is a name that has to be edited every release or the download disappears.
MAC_DMG_FILE = "ExcaliburView-mac.dmg"
MAC_DISK_IMAGE = "ExcaliburView-mac.dmg"
# Linux gets the program itself, as Windows does: run once, it installs itself
# for that person, and an update replaces that one file.
LINUX_FILE = "ExcaliburView-linux"
FILES = {APP_PLATFORM: APP_FILE, SERVER_PLATFORM: SERVER_FILE, MAC_PLATFORM: MAC_FILE, LINUX_PLATFORM: LINUX_FILE}
MANIFEST = "release.json"

# Under every release on the page, for whoever downloads from it.
HOW_TO = """

---

**ExcaliburView.exe**: the desktop app. Double-click it on each computer. It installs itself for that person, opens, and finds the office's server by itself.

**ExcaliburView-Server.exe**: the Office server. Double-click it once on the computer that should hold the company's drawings. Windows asks for permission once. After that it runs by itself, and keeps itself and every copy of Excalibur View in the office up to date.

**ExcaliburView-mac.dmg**: the Mac version. Open it and drag Excalibur View into Applications. It is the whole program, not a viewer — Apple Silicon and Intel in one download.

**ExcaliburView-linux**: the Linux version (Intel and AMD, 64-bit). Mark it as a program (right-click, Properties, Allow executing; or `chmod +x ExcaliburView-linux`) and run it once. It installs itself for that person, on the applications menu, and finds the office's server by itself.

The first person to open Excalibur View after the server is up sets it up and gets a join code for everybody else."""

# ---- signing ---------------------------------------------------------------

def load_key(path):
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    lines = open(path, encoding="utf-8").read().split("\n")
    name, seed = lines[0].strip(), lines[1].strip()
    return name, Ed25519PrivateKey.from_private_bytes(bytes.fromhex(seed))


def payload(release):
    return (
        "hyperview-release-v1\n"
        f"version={release['version']}\n"
        f"channel={release['channel']}\n"
        f"platform={release['platform']}\n"
        f"bytes={release['bytes']}\n"
        f"digest={release['digest'].lower()}\n"
        f"minimum_api_version={release['minimum_api_version']}\n"
    ).encode("utf-8")


def describe(path, version, channel, platform, notes, published, min_api):
    data = open(path, "rb").read()
    return {
        "version": version,
        "channel": channel,
        "platform": platform,
        "published": published,
        "notes": notes,
        # What the file is called on the release, which is what a server
        # looks it up by. The files uploaded must carry these names.
        "download": FILES[platform],
        "bytes": len(data),
        "digest": hashlib.sha256(data).hexdigest(),
        "signature": "",
        "key": "",
        "minimum_api_version": min_api,
    }


def sign_release(release, key_name, key):
    release = dict(release)
    release["key"] = key_name
    release["signature"] = key.sign(payload(release)).hex()
    return release


def verify(release, public):
    public.verify(bytes.fromhex(release["signature"]), payload(release))


def parts(manifest):
    """Every signed release in a manifest, whichever shape it is written in.

    `app` and `server` are single releases; `apps` and `servers` are lists of
    them. Anything that walks a manifest walks this, so adding a platform
    never means remembering to update three loops.
    """
    found = []
    for key in ("app", "server"):
        if key in manifest:
            found.append(manifest[key])
    for key in ("apps", "servers"):
        found.extend(manifest.get(key, []))
    # `app` is repeated inside `apps` on purpose; count each build once.
    seen, unique = set(), []
    for part in found:
        if part["digest"] not in seen:
            seen.add(part["digest"])
            unique.append(part)
    return unique


def every_part(manifest):
    """Every release in a manifest as it is written, `app` and its copy in
    `apps` both. `parts` counts a build once; this is for checking something
    that has to be true of each copy a seat might read."""
    found = [manifest[key] for key in ("app", "server") if key in manifest]
    for key in ("apps", "servers"):
        found.extend(manifest.get(key, []))
    return found


def each_part(manifest, change):
    """A copy of `manifest` with every release in it put through `change`,
    keeping the shape and order it was written in.

    The one walk every kind of signing shares, so a manifest signed on a
    hardware key is laid out exactly as one signed with the key file."""
    out = dict(manifest)
    for key in ("app", "server"):
        if key in out:
            out[key] = change(out[key])
    for key in ("apps", "servers"):
        if key in out:
            out[key] = [change(part) for part in out[key]]
    return out


def unsigned_manifest(version, channel, notes, published, min_api, programs):
    """The manifest `sign` writes, before anything is signed. `programs` is
    {platform: file}."""
    described = lambda platform: describe(programs[platform], version, channel, platform, notes, published, min_api)
    apps = [described(APP_PLATFORM)] + [described(p) for p in (MAC_PLATFORM, LINUX_PLATFORM) if programs.get(p)]

    # `app` is the Windows build and stays exactly where it has always been.
    # Every copy of Excalibur View installed before there was a Mac build reads
    # that one field and knows nothing about the list beside it; move it and
    # each of those copies is stranded on the version it happens to have, with
    # no way of being told about another. `apps` is what a copy that knows to
    # look reads, and it holds every build including the Windows one.
    manifest = {"app": apps[0]}
    if programs.get(SERVER_PLATFORM):
        manifest["server"] = described(SERVER_PLATFORM)
    if len(apps) > 1:
        manifest["apps"] = apps
    return manifest


def write_manifest(manifest, path):
    """How every signed manifest is written, whichever way it was signed."""
    with open(path, "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)


def now():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def notes_and_date(a):
    notes = open(a.notes_file, encoding="utf-8").read().strip() if a.notes_file else ""
    return notes, a.published or now()


def programs_given(a):
    """{platform: file} for each program handed to `sign`."""
    given = {APP_PLATFORM: a.app, SERVER_PLATFORM: a.server, MAC_PLATFORM: a.mac,
             LINUX_PLATFORM: getattr(a, "linux", None)}
    return {platform: path for platform, path in given.items() if path}


def needed(a, *names):
    """Stops, in words, if an option this way of running needs was left off."""
    missing = ["--" + name.replace("_", "-") for name in names if not getattr(a, name, None)]
    if missing:
        how = ("--finish" if getattr(a, "finish", None) else
               "--prepare" if getattr(a, "prepare", False) else "--key")
        raise SystemExit(f"{a.command} {how} also needs {', '.join(missing)}")


def cmd_sign(a):
    if getattr(a, "finish", None):
        return finish_sign(a)
    needed(a, "version", "app")
    if getattr(a, "prepare", False):
        return prepare_sign(a)
    name, key = load_key(a.key)
    notes, published = notes_and_date(a)
    manifest = each_part(
        unsigned_manifest(a.version, a.channel, notes, published, a.min_api, programs_given(a)),
        lambda part: sign_release(part, name, key))

    for part in parts(manifest):
        verify(part, key.public_key())
    out = a.out or MANIFEST
    write_manifest(manifest, out)
    built = ", ".join(part["platform"] for part in parts(manifest))
    print(f"signed {a.version} ({a.channel}) for {built} with '{name}' -> {out}")


# ---- signing on a hardware key: prepare, then finish -----------------------
#
# The release key can be kept on a hardware security key (a YubiKey) instead
# of in a file. A hardware key never hands its key over. It stays plugged into
# one computer and signs short messages there, so a release is signed in two
# halves:
#
#   --prepare  does everything `--key` does up to the moment of signing, and
#              instead of signing writes down the exact bytes to be signed, in
#              a signing request. The request, the unsigned manifest and a copy
#              of every program it describes are kept together in a new folder
#              under dist/, which nothing ever writes over.
#
#   Then a copy of the request goes to the computer the key is plugged into,
#   is signed there, and a signatures file comes back.
#
#   --finish   reads the release's own kept request (never the copy that
#              travelled), checks every signature against the release key the
#              installed copies trust (KEYS in crates/hyperview/src/trust.rs),
#              checks that nothing kept has changed since prepare, and then
#              carries on exactly as `--key` would have.
#
# What is signed does not change, and nor does how a seat checks it. Ed25519
# signatures are deterministic: one key over the same bytes always makes the
# same signature. So a release finished this way is, byte for byte, the
# release the key file would have made. tools/test_yubikey_signing.py proves
# that, and proves every refusal below.

REQUEST_FORMAT = "excalibur-sign-request/1"
SIGNATURES_FORMAT = "excalibur-signatures/1"
PRODUCT_ID = "hyperview"
RELEASE_STEP = "release"
PROMOTE_STEP = "promote"
# Where the keys every installed copy trusts are compiled in from.
TRUST_FILE = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                          "crates", "hyperview", "src", "trust.rs")
KEPT_IN = "dist"
UNSIGNED = "release.unsigned.json"
AS_PUBLISHED = "release.as-published.json"
PROGRAMS = "programs"
MOST_ITEMS = 100
LARGEST_MESSAGE = 1024 * 1024
LONGEST_WHAT = 200
# The slots a hardware key signs from: the four standard ones and the twenty
# "retired key" slots, which is where an imported key usually goes.
PIV_SLOTS = ("9a", "9c", "9d", "9e") + tuple(f"{n:x}" for n in range(0x82, 0x96))
PLAIN_NAMES = {
    APP_PLATFORM: "the desktop app for Windows",
    SERVER_PLATFORM: "the office server for Windows",
    MAC_PLATFORM: "the Mac app",
    LINUX_PLATFORM: "the desktop app for Linux",
}


class Refused(SystemExit):
    """A signing step that will not go ahead. It says why in words, and
    nothing has been signed, written over or published."""


def trusted_release_keys(path=None):
    """The release keys every installed copy trusts, as [(name, 32 bytes)].

    Read out of trust.rs, the file the program compiles its list from, so a
    signature checked here is checked against exactly what a seat checks it
    against. Only `KEYS`: a licence key or a plugin key can never vouch for a
    release. Anything malformed is left out, as `Trusted::pinned` leaves it
    out, and so is anything commented out, as the compiler would."""
    path = path or TRUST_FILE
    try:
        with open(path, encoding="utf-8") as f:
            text = f.read()
    except OSError as e:
        raise Refused(f"could not read the keys the program trusts from {path}: {e}")
    found = re.search(r"pub const KEYS\s*:[^=]*=\s*&\[(.*?)\];", text, re.S)
    if not found:
        raise Refused(f"{path} has no KEYS list in it, so there is nothing to check a signature against.")
    listed = re.sub(r"//[^\n]*", "", found.group(1))
    keys = []
    for name, public in re.findall(r'\(\s*"([^"]*)"\s*,\s*"([^"]*)"\s*\)', listed):
        if re.fullmatch(r"[0-9a-fA-F]{64}", public.strip()):
            keys.append((name, bytes.fromhex(public.strip())))
    return keys


def the_trusted_key(name=None):
    """(name, public key) of the release key to sign with: the one the
    program trusts, or the one named if it trusts several."""
    keys = trusted_release_keys()
    if not keys:
        raise Refused("The program trusts no release key (KEYS in crates/hyperview/src/trust.rs is "
                      "empty), so every installed copy would refuse whatever this signed.")
    if name is None:
        if len(keys) > 1:
            raise Refused("The program trusts more than one release key "
                          f"({', '.join(n for n, _ in keys)}). Say which with --key-name.")
        return keys[0]
    for listed, public in keys:
        if listed == name:
            return listed, public
    raise Refused(f"'{name}' is not a release key the program trusts. It trusts: "
                  f"{', '.join(n for n, _ in keys)}.")


def trusted_name_for(public):
    """The name the program trusts a public key under, or a refusal."""
    for name, listed in trusted_release_keys():
        if listed == public:
            return name
    raise Refused("This release was prepared for a key the program does not trust: it is not in KEYS "
                  "in crates/hyperview/src/trust.rs. Every installed copy would refuse what it signed.")


def piv_slot(text):
    slot = (text or "").strip().lower()
    if slot not in PIV_SLOTS:
        raise Refused(f"'{text or ''}' is not a slot a hardware key signs from. Give --slot as the "
                      "two hex digits of the slot holding the release key: 9a, 9c, 9d, 9e, or 82 to 95.")
    return slot


def commit_built():
    """The commit the release is built from, in full, or "" if git cannot say."""
    try:
        out = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo_root(),
                             capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.SubprocessError):
        return ""
    commit = out.stdout.strip().lower()
    return commit if out.returncode == 0 and re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", commit) else ""


def b64(raw):
    return base64.b64encode(raw).decode("ascii")


def unb64(text, what):
    if not isinstance(text, str):
        raise Refused(f"{what} is missing.")
    try:
        return base64.b64decode(text, validate=True)
    except (binascii.Error, ValueError):
        raise Refused(f"{what} is not base64.")


def read_json(path, what):
    try:
        with open(path, "rb") as f:
            raw = f.read()
    except OSError as e:
        raise Refused(f"could not read {what} ({path}): {e.strerror or e}")
    if raw.startswith(b"\xef\xbb\xbf"):
        raise Refused(f"{what} ({path}) starts with a byte-order mark. The format is UTF-8 JSON without one.")
    try:
        return json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as e:
        raise Refused(f"{what} ({path}) is not readable JSON: {e}")


def what_it_is(part, promoting=False):
    """One line saying what a message vouches for, shown before it is signed."""
    plain = PLAIN_NAMES.get(part["platform"], part["platform"])
    channel = "moving to the stable channel" if promoting else f"{part['channel']} channel"
    text = (f"Excalibur View {part['version']}, {plain} ({channel}): {part['download']}, "
            f"{part['bytes']:,} bytes, SHA-256 {part['digest'][:16]}")
    return text[:LONGEST_WHAT]


def to_be_signed(manifest, promoting=False):
    """What a signing request asks to have signed for this manifest: one item
    per build, as (id, what, message). The message is `payload`, the very
    bytes the key file signs."""
    items, seen = [], {}
    for part in every_part(manifest):
        platform, message = part["platform"], payload(part)
        if platform in seen:
            if seen[platform] != message:
                raise Refused(f"Two different builds in this release both say they are {platform}.")
            continue
        seen[platform] = message
        items.append((platform, what_it_is(part, promoting), message))
    return items


def messages_of(manifest, promoting=False):
    return {id: message for id, _, message in to_be_signed(manifest, promoting)}


def kept_folder(parent, version, step):
    """A new folder for one signing, never one already used."""
    parent = parent or os.path.join(repo_root(), KEPT_IN)
    base = os.path.join(parent, f"{PRODUCT_ID}-{version}-{step}")
    folder, n = base, 1
    while os.path.exists(folder):
        n += 1
        folder = f"{base}-{n}"
    os.makedirs(os.path.join(folder, PROGRAMS))
    return folder


def request_name(version, step):
    return f"{PRODUCT_ID}-{version}-{step}.sign-request.json"


def check_request(request, where):
    """That a signing request is one the format allows. Returns {id: message}."""
    def need(ok, why):
        if not ok:
            raise Refused(f"{where} {why}")
    need(isinstance(request, dict), "is not a signing request.")
    need(request.get("format") == REQUEST_FORMAT, f"is not in the format this reads ({REQUEST_FORMAT}).")
    need(request.get("product") == PRODUCT_ID,
         f"is for {request.get('product')!r}, not Excalibur View ({PRODUCT_ID!r}).")
    need(isinstance(request.get("version"), str) and request["version"], "names no version.")
    need(isinstance(request.get("step"), str) and re.fullmatch(r"[a-z0-9-]{1,40}", request["step"]),
         "has no proper step name.")
    need(isinstance(request.get("commit"), str) and re.fullmatch(r"|[0-9a-f]{40}|[0-9a-f]{64}", request["commit"]),
         "has a commit that is not a full git commit id.")
    need(request.get("slot") in PIV_SLOTS, "names no slot a hardware key signs from.")
    need(len(unb64(request.get("publicKey"), f"{where}'s public key")) == 32, "has a public key that is not 32 bytes.")
    items = request.get("items")
    need(isinstance(items, list) and 1 <= len(items) <= MOST_ITEMS, f"must ask for 1 to {MOST_ITEMS} signatures.")
    messages = {}
    for item in items:
        need(isinstance(item, dict), "has an item that is not an item.")
        id = item.get("id")
        need(isinstance(id, str) and re.fullmatch(r"[a-z0-9][a-z0-9.-]{0,63}", id), f"has an item called {id!r}.")
        need(id not in messages, f"asks for {id} twice.")
        what = item.get("what")
        need(isinstance(what, str) and what.strip() and len(what) <= LONGEST_WHAT,
             f"does not say what {id} is in {LONGEST_WHAT} characters or fewer.")
        message = unb64(item.get("message"), f"{where}'s message for {id}")
        need(0 < len(message) <= LARGEST_MESSAGE, f"has a message for {id} that is empty or over 1 MiB.")
        messages[id] = message
    return messages


def write_request(folder, version, step, slot, public, items):
    """Writes the signing request into the kept folder and returns its path."""
    request = {
        "format": REQUEST_FORMAT,
        "product": PRODUCT_ID,
        "version": version,
        "step": step,
        "commit": commit_built(),
        "made": now(),
        "slot": slot,
        "publicKey": b64(public),
        "items": [{"id": id, "what": what, "message": b64(message)} for id, what, message in items],
    }
    # Held to the same rules finish holds it to, before anybody is asked to
    # sign it.
    check_request(request, "the signing request just made")
    path = os.path.join(folder, request_name(version, step))
    with open(path, "x", encoding="utf-8", newline="\n") as f:
        json.dump(request, f, indent=2, ensure_ascii=False)
        f.write("\n")
    return path


def read_kept_request(path, step):
    """The release's own kept request, checked. Returns (request, {id:
    message}, its folder, the trusted key's name, the trusted public key)."""
    request = read_json(path, "the kept signing request")
    messages = check_request(request, "the kept signing request")
    if request["step"] != step:
        raise Refused(f"That request is for the '{request['step']}' step, not '{step}'. "
                      "Finish it with the command that prepared it.")
    public = unb64(request["publicKey"], "the kept request's public key")
    return request, messages, os.path.dirname(os.path.abspath(path)), trusted_name_for(public), public


def check_signatures(request, messages, signatures, public):
    """Every check a signatures file has to pass before anything it signed is
    used. Returns {id: 64-byte signature}.

    `request` is the release's own kept request, `messages` what it asks to
    have signed, and `public` the key the program trusts. Nothing in the signatures file is believed because it says
    so: the public key it names has to be the trusted one, and each signature
    is checked against the trusted one."""
    from cryptography.exceptions import InvalidSignature
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

    where = "The signatures file"

    def need(ok, why):
        if not ok:
            raise Refused(f"{where} {why} Nothing was signed or published.")
    need(isinstance(signatures, dict), "is not a signatures file.")
    need(signatures.get("format") == SIGNATURES_FORMAT, f"is not in the format this reads ({SIGNATURES_FORMAT}).")
    for field in ("product", "version", "step", "commit"):
        need(signatures.get(field) == request[field],
             f"is for {field} {signatures.get(field)!r}, and this release's own request says {request[field]!r}.")
    need(unb64(signatures.get("publicKey"), f"{where}'s public key") == public,
         "names a public key that is not the release key the program trusts.")
    entries = signatures.get("signatures")
    need(isinstance(entries, list), "has no list of signatures.")
    got = {}
    for entry in entries:
        need(isinstance(entry, dict) and isinstance(entry.get("id"), str), "has a signature with no id.")
        need(entry["id"] not in got, f"has two signatures for {entry['id']}.")
        got[entry["id"]] = entry
    missing = sorted(set(messages) - set(got))
    need(not missing, f"has no signature for {', '.join(missing)}.")
    extra = sorted(set(got) - set(messages))
    need(not extra, f"has signatures for {', '.join(extra)}, which this release did not ask for.")

    trusted = Ed25519PublicKey.from_public_bytes(public)
    out = {}
    for id, message in messages.items():
        entry = got[id]
        need(entry.get("messageSha256") == hashlib.sha256(message).hexdigest(),
             f"was signed over something other than {id}'s message: its SHA-256 is not this release's.")
        signature = unb64(entry.get("signature"), f"{where}'s signature for {id}")
        need(len(signature) == 64, f"has a signature for {id} that is not 64 bytes.")
        try:
            trusted.verify(signature, message)
        except InvalidSignature:
            raise Refused(f"{where}'s signature for {id} does not check out against the release key the "
                          "program trusts. Every installed copy would refuse it. Nothing was signed or published.")
        out[id] = signature
    return out


def prepare_release(a, dmg=None):
    """`sign`, stopped where the key file would sign. Returns (kept folder,
    request, unsigned manifest)."""
    version = a.version
    if not re.fullmatch(r"\d+\.\d+\.\d+", version or ""):
        raise Refused(f"'{version}' is not a version a seat can compare. Use major.minor.patch, like 0.6.16.")
    slot = piv_slot(a.slot)
    name, public = the_trusted_key(getattr(a, "key_name", None))
    given = programs_given(a)
    for path in list(given.values()) + ([dmg] if dmg else []):
        if not os.path.isfile(path):
            raise Refused(f"{path} is not there.")
    notes, published = notes_and_date(a)

    # The programs are copied in first and described from the copies, so the
    # request is over exactly the files kept beside it.
    folder = kept_folder(getattr(a, "out_dir", None), version, RELEASE_STEP)
    kept = {}
    for platform, path in given.items():
        kept[platform] = os.path.join(folder, PROGRAMS, FILES[platform])
        shutil.copyfile(path, kept[platform])
    if dmg:
        shutil.copyfile(dmg, os.path.join(folder, PROGRAMS, MAC_DMG_FILE))
    manifest = each_part(unsigned_manifest(version, a.channel, notes, published, a.min_api, kept),
                         lambda part: dict(part, key=name))
    write_manifest(manifest, os.path.join(folder, UNSIGNED))
    request = write_request(folder, version, RELEASE_STEP, slot, public, to_be_signed(manifest))
    return folder, request, manifest


def finish_release(request_path, signatures_path):
    """The manifest `sign --key` would have written, from a kept request and
    the signatures that came back. Returns (manifest, kept folder, request)."""
    request, messages, folder, name, public = read_kept_request(request_path, RELEASE_STEP)
    manifest = read_json(os.path.join(folder, UNSIGNED), "the release kept with the request")

    # Nothing built has changed since it was prepared.
    for part in every_part(manifest):
        path = os.path.join(folder, PROGRAMS, os.path.basename(part["download"]))
        try:
            with open(path, "rb") as f:
                data = f.read()
        except OSError:
            raise Refused(f"{part['download']} is no longer in {folder}. Prepare the release again.")
        if len(data) != part["bytes"] or hashlib.sha256(data).hexdigest() != part["digest"]:
            raise Refused(f"{part['download']} has changed since this release was prepared, so the signatures "
                          "are over a build that is not the one here. Prepare the release again.")
    if messages_of(manifest) != messages:
        raise Refused("The kept request no longer matches the release kept beside it. Prepare the release again.")

    signatures = check_signatures(request, messages, read_json(signatures_path, "the signatures file"), public)
    finished = each_part(manifest, lambda part: dict(part, key=name, signature=signatures[part["platform"]].hex()))
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    for part in every_part(finished):
        verify(part, Ed25519PublicKey.from_public_bytes(public))
    return finished, folder, request


def prepare_promote(manifest, raw, version, slot, key_name=None, out_dir=None):
    """`promote`, stopped where the key file would sign. `raw` is release.json
    exactly as it was downloaded. Returns (kept folder, request)."""
    slot = piv_slot(slot)
    name, public = the_trusted_key(key_name)
    unsigned = each_part(manifest, lambda part: dict(part, channel="stable", key=name, signature=""))
    items = to_be_signed(unsigned, promoting=True)
    folder = kept_folder(out_dir, version, PROMOTE_STEP)
    with open(os.path.join(folder, AS_PUBLISHED), "wb") as f:
        f.write(raw)
    return folder, write_request(folder, version, PROMOTE_STEP, slot, public, items)


def finish_promote(request_path, signatures_path, current):
    """The stable manifest `promote --key` would have written. `current` is
    release.json as the release holds it now. Returns (manifest, kept folder,
    request)."""
    request, messages, folder, name, public = read_kept_request(request_path, PROMOTE_STEP)
    kept = read_json(os.path.join(folder, AS_PUBLISHED), "the release.json kept with the request")
    try:
        now_there = json.loads(current)
    except ValueError:
        now_there = None
    if now_there != kept:
        raise Refused("release.json on the release has changed since this promotion was prepared, so the "
                      "signatures are over something no longer there. Prepare the promotion again.")
    promoted = lambda part: dict(part, channel="stable", key=name)
    if messages_of(each_part(kept, promoted), promoting=True) != messages:
        raise Refused("The kept request no longer matches the release it was prepared from. "
                      "Prepare the promotion again.")

    signatures = check_signatures(request, messages, read_json(signatures_path, "the signatures file"), public)
    finished = each_part(kept, lambda part: dict(promoted(part), signature=signatures[part["platform"]].hex()))
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    for part in every_part(finished):
        verify(part, Ed25519PublicKey.from_public_bytes(public))
    return finished, folder, request


def shown(path):
    """A path as somebody would type it from here, quoted."""
    try:
        relative = os.path.relpath(path)
    except ValueError:  # another drive, on Windows
        relative = path
    shorter = relative if not relative.startswith("..") else path
    return f'"{shorter}"'


def say_what_next(folder, request, finish_command):
    signatures = os.path.basename(request).replace(".sign-request.json", ".signatures.json")
    print()
    print("  Nothing is signed yet and nothing has been published.")
    print()
    print(f"  kept in          {shown(folder)}")
    print(f"  signing request  {os.path.basename(request)}")
    print()
    print("  Next:")
    print("  1. Copy the signing request to the computer the hardware key is plugged")
    print(f"     into, and sign it there. That writes {signatures}.")
    print("  2. Bring that signatures file back to this computer, and finish:")
    print()
    print(f"     {finish_command(shown(request), signatures)}")
    print()


def prepare_sign(a):
    folder, request, manifest = prepare_release(a)
    built = ", ".join(part["platform"] for part in parts(manifest))
    print(f"prepared {a.version} ({a.channel}) for {built}: {len(parts(manifest))} to sign "
          f"with the key in slot {piv_slot(a.slot)}")
    say_what_next(folder, request, lambda req, sigs:
                  f"python3 tools/publish.py sign --finish {req} --signatures {sigs}")


def finish_sign(a):
    needed(a, "signatures")
    manifest, folder, request = finish_release(a.finish, a.signatures)
    out = a.out or os.path.join(folder, MANIFEST)
    write_manifest(manifest, out)
    first = parts(manifest)[0]
    built = ", ".join(part["platform"] for part in parts(manifest))
    print(f"signed {first['version']} ({first['channel']}) for {built} with '{first['key']}' "
          f"on the hardware key -> {out}")


# ---- licenses --------------------------------------------------------------

LICENSE_FIELDS = ("id", "company", "edition", "users", "updates_through", "issued", "note")

# Each product's licenses start their signed text with a line of their own, so
# a license for one can never be taken for a license for another - even though
# the same key signs them all. Excalibur View's was first and keeps its line.
PRODUCTS = {
    "view": {"domain": "excalibur-license-v1", "extension": "evlicense", "id": "evl_",
             "editions": ("office", "sealed")},
    "citadel": {"domain": "excalibur-citadel-license-v1", "extension": "ctlicense", "id": "ctl_",
                "editions": ("standard",)},
}


def license_payload(lic, product="view"):
    lines = [PRODUCTS[product]["domain"]] + [f"{k}={lic[k]}" for k in LICENSE_FIELDS]
    return ("\n".join(lines) + "\n").encode("utf-8")


def make_license(key_path, company, product="view", edition=None, users=0, updates_through=None,
                 years=1, founding=False, note=None, id=None, issued=None):
    """Signs a license and hands it back. The one place a license is made:
    `sign-license` and the Square side both come through here."""
    import secrets
    name, key = load_key(key_path)
    today = datetime.date.today()
    kind = PRODUCTS[product]
    edition = edition or kind["editions"][0]
    if edition not in kind["editions"]:
        raise SystemExit(f"{product} has no edition called {edition}: {', '.join(kind['editions'])}")
    if founding:
        users, through, note = 0, FOREVER, note or "Founding customer"
    else:
        note = note or ""
        if updates_through:
            through = updates_through
        else:
            through = a_year_on(today, years)
    if through != FOREVER:
        datetime.date.fromisoformat(through)
    for text in (company, note):
        if "\n" in text or "\r" in text:
            raise SystemExit("a license cannot have a line break in it")
    if users < 0:
        raise SystemExit("--users is a number; 0 means no limit")
    lic = {
        "id": id or kind["id"] + secrets.token_hex(6),
        "company": company.strip(),
        "edition": edition,
        "users": users,
        "updates_through": through,
        "issued": issued or today.isoformat(),
        "note": note,
        "key": name,
    }
    signed = license_payload(lic, product)
    signature = key.sign(signed)
    key.public_key().verify(signature, signed)
    lic["signature"] = signature.hex()
    return lic


FOREVER = "forever"


def a_year_on(day, years=1):
    """The same date `years` later, as text. 29 February lands on the 28th."""
    try:
        return day.replace(year=day.year + years).isoformat()
    except ValueError:
        return day.replace(day=28).replace(year=day.year + years).isoformat()


def long_date(day):
    """2027-10-01 as "Oct 1, 2027", the way the program writes it."""
    months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
    try:
        d = datetime.date.fromisoformat(day)
    except ValueError:
        return day
    return f"{months[d.month - 1]} {d.day}, {d.year}"


def license_file_name(lic, product="view"):
    """`Example Steel, Inc.evlicense` - the company, as a file name."""
    safe = "".join(c for c in lic["company"] if c not in '\\/:*?"<>|').strip().rstrip(".") or lic["id"]
    return f"{safe}.{PRODUCTS[product]['extension']}"


def write_license(lic, path):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(lic, f, indent=2)
        f.write("\n")
    return path


def license_filename_for(license_id):
    """The name a license is published under, so a server can fetch its own.

    The id put through SHA-256. The address cannot be worked out from a
    company's name, and the list of customers is not something anybody can
    walk. Must match `crate::license::name_for` in the server, which a test
    in the Rust code checks.
    """
    return hashlib.sha256(license_id.strip().encode("utf-8")).hexdigest() + ".evlicense"


def cmd_publish_license(a):
    """Puts a signed license where the customer's server will find it.

    This is the whole of "they bought three more seats": sign a new license
    with the same id and the higher count, put it here, and their server picks
    it up within the half hour. Nobody emails anybody a file.

    The server refuses a renewal that gives fewer seats or ends sooner, so a
    stale file cannot take seats off a shop in the middle of a bid.
    """
    lic = json.load(open(a.license, encoding="utf-8"))
    name = license_filename_for(lic["id"])
    out = os.path.join(a.into, name)
    os.makedirs(a.into, exist_ok=True)
    with open(out, "w", encoding="utf-8") as f:
        json.dump(lic, f, indent=2)
    print(f"{lic['company']}: {lic['users'] or 'every'} seats, updates through {lic['updates_through']}")
    print(f"  -> {out}")
    print()
    print("  Publish that folder, then the customer's server finds it by itself.")
    print("  Nothing else has to reach them.")


def cmd_sign_license(a):
    lic = make_license(
        a.key, a.company, product=a.product, edition=a.edition, users=a.users,
        updates_through=a.updates_through, years=a.years, founding=a.founding,
        note=a.note, id=a.id, issued=a.issued,
    )
    out = a.out or license_file_name(lic, a.product)
    write_license(lic, out)
    users_said = "every user" if lic["users"] == 0 else f"{lic['users']} users"
    print(f"licensed {lic['company']}: {lic['edition']}, {users_said}, "
          f"updates through {lic['updates_through']} -> {out}")


# ---- plugins ---------------------------------------------------------------

PLUGIN_MAGIC = b"HVPLUGIN1"


def plugin_payload(plugin_id, version, sha256):
    return f"hyperview-plugin/1\n{plugin_id}\n{version}\n{sha256.lower()}\n".encode("utf-8")


def cmd_make_key(a):
    """A new signing key, made here and kept here. Prints the line for trust.rs."""
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{2,60}", a.name):
        raise SystemExit("a key name is lower-case letters, digits and dashes, like excalibur-plugins-2026")
    if os.path.exists(a.out):
        raise SystemExit(f"{a.out} is already there. A key is never written over; pick another name.")
    key = Ed25519PrivateKey.generate()
    seed = key.private_bytes(serialization.Encoding.Raw, serialization.PrivateFormat.Raw,
                             serialization.NoEncryption()).hex()
    public = key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex()
    with open(a.out, "x", encoding="utf-8") as f:
        f.write(f"{a.name}\n{seed}\n")
    print(f"made {a.out}. Keep it on this computer and in your password manager, nowhere else.")
    print("paste this into the list it's for in crates/hyperview/src/trust.rs:")
    print(f'    ("{a.name}", "{public}"),')


def cmd_sign_plugin(a):
    name, key = load_key(a.key)
    if os.path.basename(a.key) == "hyperview-signing.key":
        print("note: this is the release key. Plugins signed with it still load, but a plugin key "
              "(make-key) keeps the release key put away.")
    wasm = open(a.wasm, "rb").read()
    if not wasm.startswith(b"\0asm"):
        raise SystemExit(f"{a.wasm} is not a WebAssembly file")
    if not re.fullmatch(r"[A-Za-z0-9_-]+", a.id):
        raise SystemExit("a plugin id is letters, digits and dashes")
    sha = hashlib.sha256(wasm).hexdigest()
    signed = plugin_payload(a.id, a.version, sha)
    signature = key.sign(signed)
    key.public_key().verify(signature, signed)
    header = {
        "id": a.id, "version": a.version, "name": a.name or a.id,
        "sha256": sha, "bytes": len(wasm), "key": name, "signature": signature.hex(),
    }
    out = a.out or f"{a.id}-{a.version}.hvplugin"
    with open(out, "wb") as f:
        f.write(PLUGIN_MAGIC + b"\n" + json.dumps(header, separators=(",", ":")).encode("utf-8") + b"\n" + wasm)
    print(f"sealed {a.id} {a.version} with '{name}' -> {out}")


# ---- GitHub ----------------------------------------------------------------

class GitHub:
    """GitHub's API, patient with a home internet connection.

    A connection that drops, times out or is reset is tried again, a few
    times, with a pause that grows; so is anything GitHub answers with a
    5xx. What cannot be fixed by waiting - a bad token, a refused request -
    stops at once, in words."""

    TRIES = 5

    def __init__(self, token_file, repo):
        self.token = open(token_file, encoding="utf-8-sig").read().strip()
        self.repo = repo

    def call(self, method, url, body=None, data=None, kind="application/json", accept="application/vnd.github+json",
             timeout=600):
        if not url.startswith("http"):
            url = f"https://api.github.com/repos/{self.repo}{url}"
        headers = {"Authorization": "Bearer " + self.token, "Accept": accept,
                   "User-Agent": "excalibur-publish", "X-GitHub-Api-Version": "2022-11-28"}
        if body is not None:
            data = json.dumps(body).encode("utf-8")
        if data is not None:
            headers["Content-Type"] = kind
        for attempt in range(1, self.TRIES + 1):
            request = urllib.request.Request(url, data=data, method=method, headers=headers)
            try:
                with urllib.request.urlopen(request, timeout=timeout) as response:
                    raw = response.read()
                    if accept.startswith("application/vnd.github") and raw:
                        return json.loads(raw)
                    return raw
            except urllib.error.HTTPError as e:
                if e.code == 404 and method == "GET":
                    return None
                said = e.read()[:400]
                if e.code >= 500 and attempt < self.TRIES:
                    self.wait(attempt, f"GitHub answered {e.code}")
                    continue
                raise GitHubRefused(e.code, f"GitHub said {e.code} to {method} {url}: {said!r}")
            except (urllib.error.URLError, TimeoutError, ConnectionError, OSError) as e:
                if attempt < self.TRIES:
                    self.wait(attempt, f"the connection to GitHub dropped ({e})")
                    continue
                raise SystemExit(f"Could not reach GitHub after {self.TRIES} tries: {e}")

    @staticmethod
    def wait(attempt, why):
        pause = 5 * attempt
        print(f"  {why}; trying again in {pause} seconds...")
        sys.stdout.flush()
        time.sleep(pause)

    def release(self, tag):
        """The release for a tag, drafts included: a draft left by an upload
        that did not finish is picked up and finished rather than a second
        one made beside it. (GitHub's look-up by tag does not see drafts.)"""
        found = self.call("GET", f"/releases/tags/{tag}")
        if found:
            return found
        for page in range(1, 6):
            listed = self.call("GET", f"/releases?per_page=100&page={page}") or []
            for release in listed:
                if release.get("tag_name") == tag:
                    return release
            if len(listed) < 100:
                break
        return None

    def assets(self, release):
        return self.call("GET", f"/releases/{release['id']}/assets?per_page=100") or []

    def replace_asset(self, release, path, name=None):
        """Uploads a file as `name`, replacing one already there. A try cut
        off part way can leave a half-made asset of that name behind, which
        GitHub then refuses to upload over; it is cleared first each time."""
        name = name or os.path.basename(path)
        data = open(path, "rb").read()
        upload = release["upload_url"].split("{")[0] + "?name=" + urllib.request.quote(name)
        for attempt in range(1, self.TRIES + 1):
            for asset in self.assets(release):
                if asset["name"] == name:
                    self.call("DELETE", f"/releases/assets/{asset['id']}")
            print(f"  uploading {name} ({len(data) / 1048576:.1f} MB)...")
            sys.stdout.flush()
            try:
                self.call("POST", upload, data=data, kind="application/octet-stream", timeout=1800)
            except GitHubRefused as e:
                if e.code == 422 and attempt < self.TRIES:
                    self.wait(attempt, "GitHub still had part of an earlier try")
                    continue
                raise SystemExit(str(e))
            print(f"  uploaded {name}")
            return
        raise SystemExit(f"{name} did not upload after {self.TRIES} tries.")


class GitHubRefused(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


def cmd_upload(a):
    gh = GitHub(a.token_file, a.repo)
    manifest = json.load(open(a.files[0], encoding="utf-8"))
    channel = parts(manifest)[0]["channel"]
    notes = open(a.notes_file, encoding="utf-8").read().strip() if a.notes_file else parts(manifest)[0]["notes"]
    tag = f"v{a.version}"

    # Each program goes up under the name release.json gives it, matched by
    # fingerprint, so a file on this computer can be called anything and the
    # manifest and the release can never disagree.
    wanted = {part["digest"]: part["download"] for part in parts(manifest)}
    uploads = []
    for path in a.files[1:]:
        digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
        if digest not in wanted:
            raise SystemExit(f"{path} is not one of the programs in {a.files[0]} - nothing was uploaded.")
        uploads.append((path, wanted.pop(digest)))
    if wanted:
        raise SystemExit(f"{a.files[0]} also lists {', '.join(wanted.values())}, which was not given - nothing was uploaded.")

    # Files that go on the release but are not programs anybody updates to.
    # The Mac disk image is the one of these: a person opens it and drags the
    # program into Applications, while the updater takes the zip that the
    # manifest names. It is on the release because the website links to it.
    for path in getattr(a, "extra", None) or []:
        # Under a fixed name, not the one it happens to have on disk. The Mac
        # build writes ExcaliburView-<version>.dmg, and 0.6.4 went up under
        # that name and the website's Mac card stayed hidden, because it looks
        # for an exact name and every other asset has a version-free one.
        name = MAC_DMG_FILE if path.lower().endswith(".dmg") else os.path.basename(path)
        uploads.append((path, name))

    release = gh.release(tag)
    if release is None:
        # A draft first, so no server sees a release with half its files.
        release = gh.call("POST", "/releases", {
            "tag_name": tag, "target_commitish": "main", "name": f"Excalibur View {a.version}",
            "body": notes + HOW_TO, "draft": True, "prerelease": channel != "stable",
        })
    elif release.get("draft"):
        print(f"finishing {tag}, which an earlier try left as a draft")
    for path, name in uploads:
        gh.replace_asset(release, path, name)
    gh.replace_asset(release, a.files[0], MANIFEST)

    # Every file there, whole, before anybody can see the release.
    there = {x["name"]: x for x in gh.assets(release)}
    for path, name in uploads + [(a.files[0], MANIFEST)]:
        asset = there.get(name)
        if not asset or asset.get("state") != "uploaded" or asset.get("size") != os.path.getsize(path):
            raise SystemExit(f"{name} is not on the release in full - it stays a draft. Running this again is safe.")
    gh.call("PATCH", f"/releases/{release['id']}", {
        "name": f"Excalibur View {a.version}", "draft": False, "prerelease": channel != "stable",
        "body": notes + HOW_TO, "make_latest": "true" if channel == "stable" else "false",
    })
    print(f"published {tag} as {'a full release' if channel == 'stable' else 'an early release'}: "
          f"{', '.join(name for _, name in uploads)}")


# ---- a whole release, in one command ---------------------------------------

def run(command, where=None):
    """Runs a command and lets it print as it goes, so a long build is not
    silence. Stops everything if it fails."""
    print("  " + " ".join(command))
    try:
        code = subprocess.call(command, cwd=where)
    except FileNotFoundError:
        # The commonest case by far: cargo, on a computer that has no Rust on
        # it. A stack trace is a poor way to say "that program is not here",
        # and this one cost a release its Windows half.
        raise SystemExit(
            f"{command[0]} is not on this computer's PATH, so this cannot run:\n"
            f"    {' '.join(command)}\n"
            f"If that is cargo: GitHub builds both programs now. Run the "
            f"Windows build workflow and pass --app and --server instead.")
    if code != 0:
        raise SystemExit(f"that failed ({code}) - nothing has been published.")


def repo_root():
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def version_in_cargo():
    with open(os.path.join(repo_root(), "Cargo.toml"), encoding="utf-8") as f:
        for line in f:
            if line.startswith("version"):
                return line.split('"')[1]
    raise SystemExit("could not read the version out of Cargo.toml")


def built(name):
    """Where cargo leaves a program, whichever kind of computer this is."""
    root = repo_root()
    for candidate in (f"{name}.exe", name):
        path = os.path.join(root, "target", "release", candidate)
        if os.path.exists(path):
            return path
    raise SystemExit(f"target/release/{name} is not there - did the build run?")


def cmd_release(a):
    """Builds, signs, uploads and publishes, in that order, stopping at the
    first thing that goes wrong.

    The signing key is read on this computer and used here. It is not sent
    anywhere and nothing about this command changes that: the build and the
    upload happen either side of it, and the key stays where it is.

    The Mac half is built on a Mac, by deploy/macos/build.sh, because only a
    Mac can sign and notarize one. Its two files are handed to this with
    --mac-zip and --mac-dmg. Without them this publishes a Windows-only
    release, which is a perfectly good release and what every release before
    0.6.4 was.

    With the release key on a hardware key, `--prepare` does all of this up
    to the signing and stops there, and `--finish` checks the signatures that
    came back and does the rest: the upload and the publishing.
    """
    if a.finish:
        return finish_and_publish(a)
    if not a.prepare:
        needed(a, "token_file", "repo")
    root = repo_root()
    version = a.version or version_in_cargo()
    if version != version_in_cargo():
        raise SystemExit(
            f"you asked for {version} and Cargo.toml says {version_in_cargo()}. "
            "One of them is wrong, and a release whose program reports a different "
            "version from its manifest is one nobody can diagnose later.")

    for name, path in (("--mac-zip", a.mac_zip), ("--mac-dmg", a.mac_dmg), ("--linux", a.linux)):
        if path and not os.path.exists(path):
            raise SystemExit(f"{name} {path} is not there.")
    if bool(a.mac_zip) != bool(a.mac_dmg):
        raise SystemExit(
            "give both --mac-zip and --mac-dmg or neither. The zip is what an "
            "update installs and the disk image is what a person downloads; a "
            "release with one and not the other is broken for somebody.")

    # 1 and 2. The two Windows programs.
    #
    # Handed in, normally. GitHub builds and tests both halves of a release --
    # the Mac on a Mac, the Windows on Windows -- and this computer's job is
    # the one thing only it can do, which is sign them with the key that lives
    # here. That key is the reason releases happen on one machine; a compiler
    # is not, and requiring one meant a release could only be cut from a
    # machine with six gigabytes of build tools on it.
    #
    # Building here still works for anybody who has the toolchain, which is
    # how a developer cuts a test release without waiting on CI.
    handed_in = bool(a.app or a.server)
    if handed_in:
        for what, path in (("--app", a.app), ("--server", a.server)):
            if not path:
                raise SystemExit(
                    f"{what} is missing. Give both programs or neither: a release "
                    "with one built here and one built somewhere else is two "
                    "different builds wearing one version number.")
            if not os.path.exists(path):
                raise SystemExit(f"{what} is not there: {path}")
        app, server = a.app, a.server
        print("\n== the programs GitHub built ==")
        for path in (app, server):
            print(f"  {os.path.basename(path)}  {os.path.getsize(path) / 1048576:.1f} MB")
    else:
        if a.skip_tests:
            print("\n== skipping the tests, because you asked ==")
        else:
            print("\n== tests ==")
            run(["cargo", "test", "--workspace", "--quiet"], root)
        if a.skip_build:
            print("\n== using the programs already in target/release ==")
        else:
            print("\n== building ==")
            run(["cargo", "build", "--release", "-p", "hyperview", "-p", "hyperview-server"], root)
        app, server = built("hyperview"), built("hyperview-server")

    # 3. With the key on a hardware key: write down what is to be signed,
    # keep it with the programs, and stop until the signatures come back.
    if a.prepare:
        print("\n== preparing to sign ==")
        folder, request, prepared = prepare_release(argparse.Namespace(
            version=version, channel=a.channel, notes_file=a.notes_file,
            app=app, mac=a.mac_zip, linux=a.linux, server=server,
            published=None, min_api=a.min_api, slot=a.slot, key_name=a.key_name,
            out_dir=a.out_dir), dmg=a.mac_dmg)
        built = ", ".join(part["platform"] for part in parts(prepared))
        print(f"prepared {version} ({a.channel}) for {built}: {len(parts(prepared))} to sign "
              f"with the key in slot {piv_slot(a.slot)}")
        token = a.token_file or "github-token.txt"
        repo = a.repo or "excaliburct/hyperview-releases"
        say_what_next(folder, request, lambda req, sigs:
                      f"python3 tools/publish.py release --finish {req} --signatures {sigs} "
                      f"--token-file {token} --repo {repo}")
        return

    # 3. Sign every program, here, with the key on this computer.
    print("\n== signing ==")
    manifest = os.path.join(root, MANIFEST)
    cmd_sign(argparse.Namespace(
        key=a.key, version=version, channel=a.channel, notes_file=a.notes_file,
        app=app, mac=a.mac_zip, linux=a.linux, server=server, out=manifest,
        published=None, min_api=a.min_api))

    if a.sign_only:
        print(f"\nstopped after signing, as you asked. {manifest} is ready.")
        return

    # 4. Up it goes, as a draft until every file is there whole.
    print("\n== uploading ==")
    files = [manifest, app, server] + ([a.mac_zip] if a.mac_zip else []) + ([a.linux] if a.linux else [])
    cmd_upload(argparse.Namespace(
        token_file=a.token_file, repo=a.repo, version=version,
        notes_file=a.notes_file, files=files,
        extra=[a.mac_dmg] if a.mac_dmg else []))

    print()
    print(f"  {version} is up as {'a full release' if a.channel == 'stable' else 'an early release'}.")
    print(f"  https://github.com/{a.repo}/releases/tag/v{version}")
    if a.channel != "stable":
        print()
        print("  Try it on one seat. When it is good, every office takes it:")
        print()
        print(f"      python3 tools/publish.py promote --key {a.key} \\")
        print(f"          --token-file {a.token_file} --repo {a.repo} --version {version}")
    print()


def finish_and_publish(a):
    """`release --finish`: the signatures checked, release.json written into
    the kept folder, and the programs kept there uploaded and published."""
    needed(a, "signatures")
    if not a.sign_only:
        needed(a, "token_file", "repo")
    manifest, folder, request = finish_release(a.finish, a.signatures)
    out = os.path.join(folder, MANIFEST)
    write_manifest(manifest, out)
    version, channel = request["version"], parts(manifest)[0]["channel"]
    built = ", ".join(part["platform"] for part in parts(manifest))
    print(f"signed {version} ({channel}) for {built} with '{parts(manifest)[0]['key']}' "
          f"on the hardware key -> {out}")
    if a.sign_only:
        print(f"\nstopped after signing, as you asked. {out} is ready.")
        return

    # Up it goes, from the copies kept at prepare: the very files signed.
    print("\n== uploading ==")
    programs = [os.path.join(folder, PROGRAMS, os.path.basename(part["download"])) for part in parts(manifest)]
    dmg = os.path.join(folder, PROGRAMS, MAC_DMG_FILE)
    cmd_upload(argparse.Namespace(
        token_file=a.token_file, repo=a.repo, version=version,
        notes_file=None, files=[out] + programs,
        extra=[dmg] if os.path.exists(dmg) else []))

    print()
    print(f"  {version} is up as {'a full release' if channel == 'stable' else 'an early release'}.")
    print(f"  https://github.com/{a.repo}/releases/tag/v{version}")
    if channel != "stable":
        print()
        print("  Try it on one seat. When it is good, every office takes it:")
        print()
        print(f"      python3 tools/publish.py promote --prepare --slot {request['slot']} \\")
        print(f"          --token-file {a.token_file} --repo {a.repo} --version {version}")
    print()


def the_published_manifest(gh, tag):
    """The release for a tag, and its release.json exactly as it is there."""
    release = gh.release(tag)
    if release is None:
        raise SystemExit(f"there is no release {tag}")
    asset = next((x for x in release["assets"] if x["name"] == MANIFEST), None)
    if asset is None:
        raise SystemExit(f"{tag} has no {MANIFEST}")
    return release, gh.call("GET", f"/releases/assets/{asset['id']}", accept="application/octet-stream")


def cmd_promote(a):
    if a.finish:
        return finish_promotion(a)
    needed(a, "version")
    gh = GitHub(a.token_file, a.repo)
    tag = f"v{a.version}"
    if a.prepare:
        release, raw = the_published_manifest(gh, tag)
        folder, request = prepare_promote(json.loads(raw), raw, a.version, a.slot, a.key_name, a.out_dir)
        print(f"prepared {tag} to be promoted to stable, with the key in slot {piv_slot(a.slot)}")
        say_what_next(folder, request, lambda req, sigs:
                      f"python3 tools/publish.py promote --finish {req} --signatures {sigs} "
                      f"--token-file {a.token_file} --repo {a.repo}")
        return
    name, key = load_key(a.key)
    release, raw = the_published_manifest(gh, tag)

    def promoted(release):
        release = sign_release(dict(release, channel="stable"), name, key)
        verify(release, key.public_key())
        return release

    manifest = each_part(json.loads(raw), promoted)
    out = os.path.join(os.path.dirname(os.path.abspath(a.key)), f"release-{a.version}-stable.json")
    write_manifest(manifest, out)
    gh.replace_asset(release, out, MANIFEST)
    gh.call("PATCH", f"/releases/{release['id']}", {"prerelease": False, "make_latest": "true"})
    print(f"promoted {tag}: every office now takes it")


def finish_promotion(a):
    """`promote --finish`: the signatures checked against what the release
    holds now, and the stable release.json put up in place of the early one."""
    needed(a, "signatures")
    request, _, _, _, _ = read_kept_request(a.finish, PROMOTE_STEP)
    tag = f"v{request['version']}"
    gh = GitHub(a.token_file, a.repo)
    release, raw = the_published_manifest(gh, tag)
    manifest, folder, request = finish_promote(a.finish, a.signatures, raw)
    out = os.path.join(folder, f"release-{request['version']}-stable.json")
    write_manifest(manifest, out)
    gh.replace_asset(release, out, MANIFEST)
    gh.call("PATCH", f"/releases/{release['id']}", {"prerelease": False, "make_latest": "true"})
    print(f"promoted {tag}, signed on the hardware key: every office now takes it")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="command", required=True)
    def ways_to_sign(parser):
        """--key, or the two halves of signing on a hardware key."""
        how = parser.add_mutually_exclusive_group(required=True)
        how.add_argument("--key", help="the key file, on this computer")
        how.add_argument("--prepare", action="store_true",
                         help="the key is on a hardware key: write a signing request instead of signing")
        how.add_argument("--finish", metavar="REQUEST",
                         help="the signing request --prepare kept, once it has been signed")
        parser.add_argument("--slot", help="with --prepare: the slot holding the release key, as two hex digits")
        parser.add_argument("--signatures", help="with --finish: the signatures file that came back")
        parser.add_argument("--key-name", help="with --prepare: which release key, if the program trusts several")
        parser.add_argument("--out-dir", help=f"with --prepare: where the kept folder goes (default {KEPT_IN}/)")

    s = sub.add_parser("sign")
    ways_to_sign(s)
    s.add_argument("--version", help="needed with --key and --prepare")
    s.add_argument("--channel", choices=["preview", "stable"], default="preview")
    s.add_argument("--notes-file")
    s.add_argument("--app", help="the Windows program; needed with --key and --prepare")
    s.add_argument("--mac", help="the Mac program, zipped by deploy/macos/build.sh")
    s.add_argument("--linux", help="the Linux program, built by the Linux build workflow")
    s.add_argument("--server")
    s.add_argument("--out", help=f"default {MANIFEST}, or with --finish, {MANIFEST} in the kept folder")
    s.add_argument("--published")
    s.add_argument("--min-api", type=int, default=1)
    u = sub.add_parser("upload")
    u.add_argument("--token-file", required=True)
    u.add_argument("--repo", required=True)
    u.add_argument("--version", required=True)
    u.add_argument("--notes-file")
    u.add_argument("--extra", action="append", default=[],
                   help="a file that goes on the release but is not in the manifest, such as the Mac disk image")
    u.add_argument("files", nargs="+", help="release.json first, then the program files")
    r = sub.add_parser("promote")
    ways_to_sign(r)
    r.add_argument("--token-file", required=True)
    r.add_argument("--repo", required=True)
    r.add_argument("--version", help="needed with --key and --prepare")
    rel = sub.add_parser("release", help="build, sign, upload and publish, in one command")
    ways_to_sign(rel)
    rel.add_argument("--token-file", help="needed with --key and --finish")
    rel.add_argument("--repo", help="needed with --key and --finish")
    rel.add_argument("--version", help="defaults to the version in Cargo.toml")
    rel.add_argument("--channel", choices=["preview", "stable"], default="preview")
    rel.add_argument("--notes-file")
    rel.add_argument("--app", help="ExcaliburView.exe, already built (GitHub builds it)")
    rel.add_argument("--server", help="ExcaliburView-Server.exe, already built")
    rel.add_argument("--mac-zip", help="ExcaliburView-mac.zip from deploy/macos/build.sh")
    rel.add_argument("--mac-dmg", help="the .dmg from the same build")
    rel.add_argument("--linux", help="ExcaliburView-linux, built by the Linux build workflow")
    rel.add_argument("--min-api", type=int, default=1)
    rel.add_argument("--skip-build", action="store_true", help="use what is in target/release already")
    rel.add_argument("--skip-tests", action="store_true")
    rel.add_argument("--sign-only", action="store_true", help="stop after writing release.json")

    l = sub.add_parser("sign-license")
    l.add_argument("--key", required=True)
    l.add_argument("--company", required=True)
    l.add_argument("--product", choices=sorted(PRODUCTS), default="view",
                   help="which program the license is for; each has its own signed first line")
    l.add_argument("--edition", help="view: office (the default) or sealed; citadel: standard")
    l.add_argument("--users", type=int, default=0, help="0 means no limit")
    l.add_argument("--years", type=int, default=1, help="years of updates from today")
    l.add_argument("--updates-through", help="YYYY-MM-DD, or forever")
    l.add_argument("--founding", action="store_true", help="every user, updates forever")
    l.add_argument("--note")
    l.add_argument("--id")
    l.add_argument("--issued")
    l.add_argument("--out")
    pl = sub.add_parser("publish-license", help="put a signed license where a server can fetch it")
    pl.add_argument("--license", required=True, help="the .evlicense file to publish")
    pl.add_argument("--into", required=True, help="the folder that gets published")

    k = sub.add_parser("make-key", help="make a new signing key on this computer")
    k.add_argument("--name", required=True, help="what the key is called in trust.rs")
    k.add_argument("--out", required=True, help="the key file to write")

    g = sub.add_parser("sign-plugin")
    g.add_argument("--key", required=True)
    g.add_argument("--id", required=True)
    g.add_argument("--version", required=True)
    g.add_argument("--name")
    g.add_argument("--out")
    g.add_argument("wasm")
    a = p.parse_args()
    try:
        {"sign": cmd_sign, "upload": cmd_upload, "promote": cmd_promote,
         "release": cmd_release, "publish-license": cmd_publish_license,
         "sign-plugin": cmd_sign_plugin, "sign-license": cmd_sign_license,
         "make-key": cmd_make_key}[a.command](a)
    except GitHubRefused as e:
        raise SystemExit(str(e))


if __name__ == "__main__":
    main()
