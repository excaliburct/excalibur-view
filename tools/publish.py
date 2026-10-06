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
"""

import argparse
import datetime
import hashlib
import json
import os
import re
import sys
import time
import urllib.error
import subprocess
import urllib.request

APP_PLATFORM = "windows-x64"
SERVER_PLATFORM = "windows-x64-server"
MAC_PLATFORM = "macos-universal"
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
FILES = {APP_PLATFORM: APP_FILE, SERVER_PLATFORM: SERVER_FILE, MAC_PLATFORM: MAC_FILE}
MANIFEST = "release.json"

# Under every release on the page, for whoever downloads from it.
HOW_TO = """

---

**ExcaliburView.exe**: the desktop app. Double-click it on each computer. It installs itself for that person, opens, and finds the office's server by itself.

**ExcaliburView-Server.exe**: the Office server. Double-click it once on the computer that should hold the company's drawings. Windows asks for permission once. After that it runs by itself, and keeps itself and every copy of Excalibur View in the office up to date.

**ExcaliburView-mac.dmg**: the Mac version. Open it and drag Excalibur View into Applications. It is the whole program, not a viewer — Apple Silicon and Intel in one download.

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


def cmd_sign(a):
    name, key = load_key(a.key)
    notes = open(a.notes_file, encoding="utf-8").read().strip() if a.notes_file else ""
    published = a.published or datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    signed = lambda path, platform: sign_release(
        describe(path, a.version, a.channel, platform, notes, published, a.min_api), name, key)

    apps = [signed(a.app, APP_PLATFORM)]
    if a.mac:
        apps.append(signed(a.mac, MAC_PLATFORM))

    # `app` is the Windows build and stays exactly where it has always been.
    # Every copy of Excalibur View installed before there was a Mac build reads
    # that one field and knows nothing about the list beside it; move it and
    # each of those copies is stranded on the version it happens to have, with
    # no way of being told about another. `apps` is what a copy that knows to
    # look reads, and it holds every build including the Windows one.
    manifest = {"app": apps[0]}
    if a.server:
        manifest["server"] = signed(a.server, SERVER_PLATFORM)
    if len(apps) > 1:
        manifest["apps"] = apps

    for part in parts(manifest):
        verify(part, key.public_key())
    with open(a.out, "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)
    built = ", ".join(part["platform"] for part in parts(manifest))
    print(f"signed {a.version} ({a.channel}) for {built} with '{name}' -> {a.out}")


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
    """
    root = repo_root()
    version = a.version or version_in_cargo()
    if version != version_in_cargo():
        raise SystemExit(
            f"you asked for {version} and Cargo.toml says {version_in_cargo()}. "
            "One of them is wrong, and a release whose program reports a different "
            "version from its manifest is one nobody can diagnose later.")

    for name, path in (("--mac-zip", a.mac_zip), ("--mac-dmg", a.mac_dmg)):
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

    # 3. Sign every program, here, with the key on this computer.
    print("\n== signing ==")
    manifest = os.path.join(root, MANIFEST)
    cmd_sign(argparse.Namespace(
        key=a.key, version=version, channel=a.channel, notes_file=a.notes_file,
        app=app, mac=a.mac_zip, server=server, out=manifest,
        published=None, min_api=a.min_api))

    if a.sign_only:
        print(f"\nstopped after signing, as you asked. {manifest} is ready.")
        return

    # 4. Up it goes, as a draft until every file is there whole.
    print("\n== uploading ==")
    files = [manifest, app, server] + ([a.mac_zip] if a.mac_zip else [])
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


def cmd_promote(a):
    gh = GitHub(a.token_file, a.repo)
    name, key = load_key(a.key)
    tag = f"v{a.version}"
    release = gh.release(tag)
    if release is None:
        raise SystemExit(f"there is no release {tag}")
    asset = next((x for x in release["assets"] if x["name"] == MANIFEST), None)
    if asset is None:
        raise SystemExit(f"{tag} has no {MANIFEST}")
    raw = gh.call("GET", f"/releases/assets/{asset['id']}", accept="application/octet-stream")
    manifest = json.loads(raw)

    def promoted(release):
        release = dict(release)
        release["channel"] = "stable"
        release = sign_release(release, name, key)
        verify(release, key.public_key())
        return release

    for field in ("app", "server"):
        if field in manifest:
            manifest[field] = promoted(manifest[field])
    for field in ("apps", "servers"):
        if field in manifest:
            manifest[field] = [promoted(part) for part in manifest[field]]
    out = os.path.join(os.path.dirname(os.path.abspath(a.key)), f"release-{a.version}-stable.json")
    with open(out, "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)
    gh.replace_asset(release, out, MANIFEST)
    gh.call("PATCH", f"/releases/{release['id']}", {"prerelease": False, "make_latest": "true"})
    print(f"promoted {tag}: every office now takes it")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="command", required=True)
    s = sub.add_parser("sign")
    s.add_argument("--key", required=True)
    s.add_argument("--version", required=True)
    s.add_argument("--channel", choices=["preview", "stable"], default="preview")
    s.add_argument("--notes-file")
    s.add_argument("--app", required=True, help="the Windows program")
    s.add_argument("--mac", help="the Mac program, zipped by deploy/macos/build.sh")
    s.add_argument("--server")
    s.add_argument("--out", default=MANIFEST)
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
    r.add_argument("--key", required=True)
    r.add_argument("--token-file", required=True)
    r.add_argument("--repo", required=True)
    r.add_argument("--version", required=True)
    rel = sub.add_parser("release", help="build, sign, upload and publish, in one command")
    rel.add_argument("--key", required=True, help="the signing key, on this computer")
    rel.add_argument("--token-file", required=True)
    rel.add_argument("--repo", required=True)
    rel.add_argument("--version", help="defaults to the version in Cargo.toml")
    rel.add_argument("--channel", choices=["preview", "stable"], default="preview")
    rel.add_argument("--notes-file")
    rel.add_argument("--app", help="ExcaliburView.exe, already built (GitHub builds it)")
    rel.add_argument("--server", help="ExcaliburView-Server.exe, already built")
    rel.add_argument("--mac-zip", help="ExcaliburView-mac.zip from deploy/macos/build.sh")
    rel.add_argument("--mac-dmg", help="the .dmg from the same build")
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
