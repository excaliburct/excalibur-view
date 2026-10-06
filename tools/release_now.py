#!/usr/bin/env python3
"""Publishing a release, for somebody who should not have to remember how.

Double-click "Release Excalibur View" on the PC and this runs. It finds the
signing key, the GitHub token, the Mac files and the release notes by looking
where they actually live, shows you what it found, and waits for you to say go.

Then it does what `publish.py release` does — tests, build, sign, upload,
publish — and says in plain words what happened.

The signing key is read here and used here. That is the reason this runs on
this computer rather than somewhere more convenient.
"""

import io
import json
import os
import shutil
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

# Where things have actually been kept, in the order worth looking.
KEY_NAMES = ("hyperview-signing.key",)
TOKEN_NAMES = ("github-token.txt", "github-token.txt.txt")
REPO = "excaliburct/hyperview-releases"
# Where the Mac build happens, so this does not have to happen on a Mac.
SOURCE_REPO = "excaliburct/excalibur-view"


def bold(text):
    return f"\033[1m{text}\033[0m" if sys.stdout.isatty() else text


def say(text=""):
    print(text, flush=True)


def stop(why, *extra):
    say()
    say(bold(why))
    for line in extra:
        say("  " + line)
    say()
    say("Nothing was published.")
    wait_to_close()
    raise SystemExit(1)


def wait_to_close():
    say()
    try:
        input("Press return to close this window. ")
    except EOFError:
        pass


def places():
    """Folders worth looking in for the key, the token and the Mac files."""
    home = os.path.expanduser("~")
    found = [ROOT, os.path.dirname(ROOT)]
    found += [os.path.join(os.path.dirname(ROOT), name)
              for name in ("ExcaliburSigning", "ExcaliburView")]
    found += [os.path.join(home, name) for name in ("Desktop", "Downloads", "OneDrive")]
    onedrive = os.path.join(home, "OneDrive")
    if os.path.isdir(onedrive):
        found.append(os.path.join(onedrive, "Desktop"))
    return [p for p in found if os.path.isdir(p)]


def find_file(names, extra_dirs=()):
    for folder in list(extra_dirs) + places():
        for name in names:
            path = os.path.join(folder, name)
            if os.path.isfile(path):
                return path
    return None


def age_of(path):
    """How old a file is, in words, so a stale one is obvious.

    A Mac build carries no version inside it and keeps its name forever. The
    one in Downloads might be the one made ten minutes ago or the one made
    last Tuesday, and the difference is a release where Mac seats and Windows
    seats run different programs under the same number.
    """
    import datetime
    try:
        when = datetime.datetime.fromtimestamp(os.path.getmtime(path))
    except OSError:
        return "Built: not known"
    old = datetime.datetime.now() - when
    if old < datetime.timedelta(hours=1):
        how = f"{int(old.total_seconds() // 60)} minutes ago"
    elif old < datetime.timedelta(days=1):
        how = f"{int(old.total_seconds() // 3600)} hours ago"
    else:
        how = f"{old.days} days ago -- check this is the one you meant"
    return f"Built {when:%d %b %H:%M}, {how}."


def find_by_suffix(suffix, contains=""):
    """The newest file ending in `suffix`, searched one level deep so a folder
    dropped on the Desktop by the Mac is found without being unpacked."""
    hits = []
    for folder in places():
        try:
            entries = os.listdir(folder)
        except OSError:
            continue
        for entry in entries:
            path = os.path.join(folder, entry)
            if os.path.isfile(path) and entry.endswith(suffix) and contains in entry:
                hits.append(path)
            elif os.path.isdir(path) and "Excalibur" in entry:
                try:
                    for inner in os.listdir(path):
                        deeper = os.path.join(path, inner)
                        if os.path.isfile(deeper) and inner.endswith(suffix) and contains in inner:
                            hits.append(deeper)
                except OSError:
                    pass
    hits.sort(key=lambda p: os.path.getmtime(p), reverse=True)
    return hits[0] if hits else None


class _DropAuthOnHop(urllib.request.HTTPRedirectHandler):
    """Takes our credentials off a redirect that leaves GitHub.

    An artifact download is answered with a redirect to storage, and that
    address is already signed inside the URL. Sending our Authorization
    header along behind it gives the storage host two credentials, and it
    refuses the one it did not issue -- a 401 whose wording ("Server failed
    to authenticate the request") reads like a bad token and is nothing of
    the kind.
    """

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        fresh = super().redirect_request(req, fp, code, msg, headers, newurl)
        if fresh is None:
            return None
        here = urllib.parse.urlparse(req.full_url).netloc
        there = urllib.parse.urlparse(newurl).netloc
        if here != there:
            for name in ("Authorization", "authorization",
                         "X-GitHub-Api-Version", "X-github-api-version"):
                fresh.headers.pop(name, None)
                fresh.unredirected_hdrs.pop(name, None)
        return fresh


_OPENER = urllib.request.build_opener(_DropAuthOnHop)


def github(url, token):
    """Asks GitHub something, and follows it to storage if it points there.

    The Accept header is always GitHub's own. Asking for application/zip on
    the artifact endpoint is refused outright -- 415, Unsupported Media Type
    -- which is a confusing answer to a request for a zip file, and cost a
    release the Mac half of itself.
    """
    request = urllib.request.Request(url, headers={
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {token}",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "excalibur-release",
    })
    return _OPENER.open(request, timeout=180)


def commit_here():
    """The commit this release is being cut from, or None if git will not say."""
    try:
        out = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT,
                             capture_output=True, text=True, timeout=30)
        return out.stdout.strip() or None if out.returncode == 0 else None
    except Exception:
        return None


# What actually ends up inside the two programs. A change anywhere else --
# release notes, this script, a workflow, a document -- cannot make the Mac
# build and the Windows build different programs, and refusing a release over
# one is crying wolf.
WHAT_SHIPS = ["crates", "Cargo.toml", "Cargo.lock", "third_party"]


def same_program(older, newer):
    """Whether two commits build the same thing.

    Asked of the paths that get compiled, not of the commit id. The commit id
    is the easy question and the wrong one: it says no to a release whose only
    change since the Mac build was a line in the release notes.
    """
    try:
        out = subprocess.run(
            ["git", "diff", "--quiet", older, newer, "--"] + WHAT_SHIPS,
            cwd=ROOT, capture_output=True, timeout=60,
        )
        return out.returncode == 0
    except Exception:
        # Git would not say, so fall back to the strict question.
        return older == newer


def commits_between(older, newer):
    """The one-line log between two commits, for showing somebody what differs."""
    try:
        out = subprocess.run(["git", "log", "--oneline", f"{older}..{newer}"],
                             cwd=ROOT, capture_output=True, text=True, timeout=30)
        return [l for l in out.stdout.splitlines() if l.strip()] if out.returncode == 0 else []
    except Exception:
        return []


def artifact_from_github(want, token, any_commit=False, into_name="from-github"):
    """Downloads a named artifact GitHub built for this release, unpacked.

    One function for both halves, because they are the same job: find the
    artifact by name, refuse it if it was built from code that differs from
    what is being released, download it, unpack it. Returns the folder, or
    None with a reason printed.
    """
    try:
        url = f"https://api.github.com/repos/{SOURCE_REPO}/actions/artifacts?per_page=100"
        with github(url, token) as answer:
            artifacts = json.load(answer).get("artifacts", [])
    except urllib.error.HTTPError as e:
        say(f"  GitHub said {e.code} when asked for {want}.")
        return None
    except Exception as e:  # a laptop on a hotel network, most likely
        say(f"  Could not reach GitHub: {e}")
        return None

    mine = [a for a in artifacts if a.get("name") == want and not a.get("expired")]
    if not mine:
        stem = want.rsplit("-", 1)[0]
        names = sorted({a.get("name", "") for a in artifacts
                        if str(a.get("name", "")).startswith(stem)})
        say(f"  GitHub has no {want}.")
        if names:
            say(f"  It has: {', '.join(names[:6])}")
        return None

    here = commit_here()
    if here and not any_commit:
        matching = [
            a for a in mine
            if same_program((a.get("workflow_run") or {}).get("head_sha") or "", here)
        ]
        if not matching:
            newest = max(mine, key=lambda a: a.get("created_at", ""))
            made_from = (newest.get("workflow_run") or {}).get("head_sha") or "?"
            say(f"  {want} was made from {made_from[:7]}, and this release is")
            say(f"  {here[:7]}. The code that gets compiled is not the same, so")
            say("  those would be two different programs.")
            ahead = commits_between(made_from, here) if made_from != "?" else []
            if ahead:
                say(f"  {len(ahead)} commit(s) it has not got:")
                for line in ahead[:8]:
                    say(f"    {line}")
                if len(ahead) > 8:
                    say(f"    ...and {len(ahead) - 8} more")
            say("  Run that build workflow on this commit and then run this again.")
            return None
        mine = matching

    newest = max(mine, key=lambda a: a.get("created_at", ""))
    into = os.path.join(ROOT, "target", into_name)
    os.makedirs(into, exist_ok=True)
    say(f"  Downloading {want}, built {newest.get('created_at', '')[:10]}...")
    try:
        with github(newest["archive_download_url"], token) as answer:
            blob = answer.read()
    except Exception as e:
        say(f"  The download did not finish: {e}")
        return None
    with zipfile.ZipFile(io.BytesIO(blob)) as bundle:
        bundle.extractall(into)
    return into


def windows_build_from_github(version, token, any_commit=False):
    """The two Windows programs, built and tested by GitHub.

    There is no compiler on the computer that holds the signing key, and
    there is no reason there should be: the key is the only thing that has
    to live there. Returns (app, server), or (None, None).
    """
    into = artifact_from_github(f"windows-{version}", token, any_commit,
                                into_name="windows-from-github")
    if not into:
        return None, None
    app = server = None
    for name in os.listdir(into):
        full = os.path.join(into, name)
        if name.lower().endswith("-server.exe"):
            server = full
        elif name.lower().endswith(".exe"):
            app = full
    if not (app and server):
        say(f"  That artifact did not hold both programs: {os.listdir(into)}")
        return None, None
    return app, server


def mac_build_from_github(version, token, any_commit=False):
    """The Mac build GitHub made for this version.

    A Mac is the only machine that can sign and notarize a Mac program, so
    GitHub keeps one for exactly that. What comes back is the same two files a
    Mac would have produced, and they are checked against the signing key here
    like anything else.

    Returns (zip, dmg) as paths, or (None, None) with a reason printed.
    """
    into = artifact_from_github(f"mac-{version}", token, any_commit,
                               into_name="mac-from-github")
    if not into:
        return None, None
    zip_path = dmg_path = None
    for name in os.listdir(into):
        full = os.path.join(into, name)
        if name.endswith("ExcaliburView-mac.zip"):
            zip_path = full
        elif name.endswith(".dmg"):
            dmg_path = full
    return zip_path, dmg_path


def version_in_cargo():
    with open(os.path.join(ROOT, "Cargo.toml"), encoding="utf-8") as f:
        for line in f:
            if line.startswith("version"):
                return line.split('"')[1]
    stop("Could not read the version out of Cargo.toml.")


def shorten(path, keep=3):
    if not path:
        return "not found"
    parts = os.path.abspath(path).replace("\\", "/").split("/")
    return os.sep.join(parts[-keep:]) if len(parts) > keep else path


def run(args):
    return subprocess.call(args, cwd=ROOT)


def main():
    promote = "--promote" in sys.argv[1:]
    # For the case where the Mac build is a commit or two behind and every
    # one of them is a note or a workflow. It has to be typed out, because
    # the point of the check is that nobody talks themselves past it by
    # accident.
    any_commit = "--mac-from-any-commit" in sys.argv[1:]

    say()
    say(bold("   Excalibur View — publishing a release"))
    say()

    # The code first. Publishing something older than what is on GitHub is a
    # mistake that is invisible until somebody reports a fixed bug.
    say(bold("Getting the latest code"))
    if subprocess.call(["git", "pull", "--ff-only"], cwd=ROOT) != 0:
        stop("Could not get the latest code.",
             "If you have local edits here, commit them or put them aside first.")

    version = version_in_cargo()
    key = find_file(KEY_NAMES)
    token = find_file(TOKEN_NAMES)
    notes = find_file((f"notes-{version}.txt", "notes.txt"), extra_dirs=[ROOT])
    if not key:
        stop("The signing key is not where this expects it.",
             "It is looked for by name, hyperview-signing.key, beside this",
             "folder or in an ExcaliburSigning folder next to it — so it",
             "follows you to another computer without anything being edited.",
             "Without it nothing can be signed, and an unsigned build is one",
             "no copy of Excalibur View anywhere will install.")
    if not token:
        stop("The GitHub token is not where this expects it.",
             "It should be github-token.txt, in dev\\ExcaliburSigning.")
    the_token = open(token).read().strip() if token else None

    say()
    say(bold("Getting the Windows programs"))
    app = server = None
    if the_token:
        app, server = windows_build_from_github(version, the_token, any_commit)
    if not app:
        if shutil.which("cargo"):
            say("  Not on GitHub. This computer will build them itself.")
        else:
            stop(
                "The Windows programs are not on GitHub, and this computer has no Rust.",
                "",
                "  On GitHub, open the Windows build workflow and press Run",
                "  workflow. It builds both programs and runs the tests, the same",
                "  way the Mac build does. Then run this again.",
                "",
                "  Rust is not needed on this computer and never was. The reason",
                "  releases happen here is the signing key, which cannot leave.",
            )

    say()
    say(bold("Getting the Mac build"))
    mac_zip, mac_dmg = (None, None)
    if the_token:
        mac_zip, mac_dmg = mac_build_from_github(version, the_token, any_commit)
    # Failing that, anything a Mac left lying about on this computer. This is
    # the path when GitHub cannot build -- no minutes, no network, no account
    # -- and somebody has sat in front of the Mac and run deploy/macos/build.sh
    # themselves. It is also the path where a file from last week is easiest to
    # ship by accident, so how old it is gets said out loud.
    from_disk = False
    if not mac_zip:
        mac_zip = find_by_suffix("ExcaliburView-mac.zip")
        mac_dmg = find_by_suffix(".dmg", contains=version)
        if mac_zip:
            from_disk = True
            say("  Using the Mac files found on this computer instead.")
            say(f"  {age_of(mac_zip)}")

    say()
    say(bold(f"Excalibur View {version}"))
    say()
    say(f"  signing key    {shorten(key)}")
    say(f"  GitHub token   {shorten(token)}")
    say(f"  release notes  {shorten(notes)}")
    say(f"  Windows app    {shorten(app)}")
    say(f"  Windows server {shorten(server)}")
    say(f"  Mac update     {shorten(mac_zip)}{'  (from this computer)' if from_disk else ''}")
    say(f"  Mac download   {shorten(mac_dmg)}")
    say()

    if promote:
        say(bold("This hands " + version + " to every office, not just the early ones."))
        say("Only do this once you have run it on a seat and it was fine.")
        if input("\nType yes to go ahead: ").strip().lower() not in ("y", "yes"):
            say("\nLeft as it was.")
            wait_to_close()
            return
        code = run([sys.executable, os.path.join("tools", "publish.py"), "promote",
                    "--key", key, "--token-file", token, "--repo", REPO,
                    "--version", version])
        if code != 0:
            stop("That did not go through. The message above says why.")
        say()
        say(bold(f"{version} is now the version every office takes."))
        wait_to_close()
        return

    if not mac_zip or not mac_dmg:
        say()
        say(bold("No Mac build found, so this would be a Windows-only release."))
        say("  That is a perfectly good release and every one before 0.6.4 was one.")
        say("  If you meant to include the Mac: on GitHub, open the Mac build")
        say("  workflow and press Run workflow. It takes about twenty minutes,")
        say("  most of it Apple. Then run this again.")
        say()
        if input("Publish Windows only? Type yes, or anything else to stop: ").strip().lower() not in ("y", "yes"):
            say("\nStopped. Nothing was published.")
            wait_to_close()
            return
        mac_zip = mac_dmg = None

    if app:
        say("This will sign everything with your key and publish it as an early")
        say("release. GitHub built and tested both halves; the signing is the")
        say("part only this computer can do.")
    else:
        say("This will run the tests, build both Windows programs, sign everything")
        say("with your key, and publish it as an early release.")
    say("Offices set to try new versions first will take it; everybody else")
    say("waits until you say so.")
    say()
    if input("Press return to go ahead, or type no to stop: ").strip().lower() in ("n", "no"):
        say("\nStopped. Nothing was published.")
        wait_to_close()
        return

    args = [sys.executable, os.path.join("tools", "publish.py"), "release",
            "--key", key, "--token-file", token, "--repo", REPO,
            "--version", version]
    if notes:
        args += ["--notes-file", notes]
    if app and server:
        args += ["--app", app, "--server", server]
    if mac_zip:
        args += ["--mac-zip", mac_zip, "--mac-dmg", mac_dmg]

    if run(args) != 0:
        stop("The release stopped before publishing.",
             "Whatever went wrong is in the message above. Running this again",
             "is safe — it finishes what it started rather than starting over.")

    say()
    say(bold("Published."))
    say()
    say(f"  https://github.com/{REPO}/releases/tag/v{version}")
    say()
    say("  The website's download page will show it within a few minutes.")
    say("  Offices on the early channel will be offered it within half an hour.")
    say()
    say("  When you have run it on one seat and it was fine, double-click")
    say("  'Give it to everyone' to hand it to the rest.")
    wait_to_close()


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        say("\n\nStopped. Nothing was published.")
