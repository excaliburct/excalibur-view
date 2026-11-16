"""The Citadel cell the Office Server runs: taken from a pinned LatCon
Estimating release (private), checked, and packed for Fleet and the IT kit.
It is never part of Excalibur View's public release, which carries only the
pin (Creede, 2026-11-13).

    python3 tools/citadel_cell.py bundle --out ExcaliburOffice-Citadel.tar.gz [--from-zip FILE]
    python3 tools/citadel_cell.py kit --out KIT/                      [--from-zip FILE]
    python3 tools/citadel_cell.py check ExcaliburOffice-Citadel.tar.gz

Everything comes from packaging/citadel-cell.json (the pin): the LatCon release
zip's address and SHA-256, and each cell file's SHA-256. The zip is fetched (or
read from --from-zip, the same file fetched by hand), checked against the pin,
and every pinned file is taken out of it by its exact name and checked again.
Nothing else in the zip is ever used.

`bundle` writes the cell part Fleet delivers: a gzipped tar of names.json
(SHA-256 to file name) and each file named by its own SHA-256, the shape the
server's crate::cellbundle::stage_bundle reads. hyperview-release signs it as
the windows-x64-citadel platform of the same version as the server, with the
same key, and bundles it for Fleet's shelf; Fleet pushes it only to installs
licensed for citadel.

`kit` copies the pinned zip itself, checked, into OUT/ for the office's
private IT kit; an administrator installs it with
hyperview-server --install-citadel-cell "<zip>", which checks it against the
pin the server was built with and refuses anything else.

`check` says whether a cell part holds exactly the pinned files.
"""

import argparse  # WHAT: the command line. WHY: three commands
import hashlib  # WHAT: SHA-256. WHY: every file is checked by it
import io  # WHAT: bytes as files. WHY: the tar is written from memory
import json  # WHAT: the pin and names.json. WHY: both are JSON
import os  # WHAT: paths. WHY: where things go
import sys  # WHAT: the exit. WHY: a refusal ends the run
import tarfile  # WHAT: the plain tar. WHY: the server reads ustar
import urllib.request  # WHAT: the download. WHY: the pinned release zip
import zipfile  # WHAT: the LatCon release zip. WHY: the cell's files are in it

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))  # WHAT: the repository. WHY: the pin is found from it
PIN = os.path.join(ROOT, "packaging", "citadel-cell.json")  # WHAT: the pin. WHY: the one source of truth
PROGRAM = "Citadel Server.exe"  # WHAT: the cell's program. WHY: it must be pinned
LARGEST = 400 * 1024 * 1024  # WHAT: the largest file taken. WHY: as the server's own cap
PLAIN = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 -_.()")  # WHAT: the letters a cell file's name may have. WHY: the server's plain_name


def sha256(data):
    return hashlib.sha256(data).hexdigest()  # WHAT: lower-case hex. WHY: as the pin writes it


def read_pin(path=None):
    """The pin, checked as strictly as the server checks it."""
    with open(path or PIN, encoding="utf-8") as f:  # WHAT: the pin given, or the repository's. WHY: read when called, so a test can point it elsewhere
        pin = json.load(f)  # WHAT: read. WHY: below
    files = pin.get("files") or {}  # WHAT: the pinned files. WHY: nothing pinned carries nothing
    if not files:
        raise SystemExit("packaging/citadel-cell.json pins no files: this build carries no Citadel cell")  # WHAT: refused. WHY: never a cell nobody pinned
    for name, digest in files.items():
        if not name or name in (".", "..") or len(name) > 100 or not set(name) <= PLAIN:
            raise SystemExit(f"the pin lists {name!r}, which isn't a plain file name")  # WHAT: refused. WHY: never outside citadel\
        if len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
            raise SystemExit(f"the pin's SHA-256 for {name} isn't 64 lower-case hex digits")  # WHAT: refused. WHY: compared as text
    if PROGRAM not in files:
        raise SystemExit(f"the pin doesn't list {PROGRAM}")  # WHAT: refused. WHY: the program is what runs
    return pin


def source_zip(pin, from_zip=None):
    """The pinned LatCon release zip's bytes, checked against the pin."""
    want = (pin.get("source") or {}).get("sha256", "")  # WHAT: the zip's pinned SHA-256. WHY: checked before anything is taken out
    if from_zip:
        with open(from_zip, "rb") as f:
            data = f.read()  # WHAT: the zip fetched by hand. WHY: the same file, checked the same way
    else:
        url = (pin.get("source") or {}).get("url", "")  # WHAT: where it is. WHY: fetched
        if not url.startswith("https://"):
            raise SystemExit("the pin's source.url isn't an https address")  # WHAT: refused. WHY: never fetched in the clear
        with urllib.request.urlopen(url, timeout=120) as r:  # nosemgrep: python.lang.security.audit.dynamic-urllib-use-detected.dynamic-urllib-use-detected -- the pinned https address, checked against its SHA-256 below
            data = r.read(LARGEST * 2 + 1)  # WHAT: read, bounded. WHY: a giant is refused
    if sha256(data) != want:
        raise SystemExit(f"the LatCon release zip's SHA-256 is {sha256(data)}, not the pinned {want}")  # WHAT: refused. WHY: only the pinned zip
    return data


def pinned_files(pin, data):
    """Each pinned file out of the zip, by its exact name, checked."""
    out = {}  # WHAT: name to bytes. WHY: returned
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        for name, want in sorted(pin["files"].items()):
            info = z.getinfo(name)  # WHAT: the member by its exact name. WHY: nothing else in the zip is used; a missing one raises
            if info.file_size > LARGEST:
                raise SystemExit(f"{name} is {info.file_size} bytes, over the {LARGEST} allowed")  # WHAT: refused. WHY: never a giant
            body = z.read(info)  # WHAT: its bytes. WHY: checked
            if sha256(body) != want:
                raise SystemExit(f"{name} in the zip isn't the pinned one ({sha256(body)}, not {want})")  # WHAT: refused. WHY: only the pinned file
            out[name] = body
    return out


def pack(files):
    """The cell part: names.json and each file named by its SHA-256, gzipped ustar."""
    buf = io.BytesIO()  # WHAT: the archive. WHY: written in memory, then out
    names = {sha256(body): name for name, body in files.items()}  # WHAT: fingerprint to name. WHY: names.json
    with tarfile.open(fileobj=buf, mode="w:gz", format=tarfile.USTAR_FORMAT) as tar:
        for name, body in [("names.json", json.dumps(names, sort_keys=True).encode("utf-8"))] + [(sha256(b), b) for b in files.values()]:
            info = tarfile.TarInfo(name)  # WHAT: a plain file. WHY: the server reads nothing else
            info.size, info.mode, info.mtime, info.type = len(body), 0o600, 0, tarfile.REGTYPE  # WHAT: size, owner-only, no date, a file. WHY: as the server's own tar writer
            tar.addfile(info, io.BytesIO(body))  # WHAT: added. WHY: in the part
    return buf.getvalue()


def entry_bytes(tar, member):
    """The bytes of one regular file in a cell part. Raises if it isn't one."""
    f = tar.extractfile(member)  # WHAT: the entry, opened. WHY: read below
    if f is None:
        raise SystemExit(f"{member.name} in a cell part isn't a plain file")  # WHAT: refused. WHY: a link or folder is never read as a file
    return f.read()  # WHAT: its bytes. WHY: checked by the caller


def check_part(path, pin):
    """Whether a cell part holds exactly the pinned files. Raises if not."""
    with tarfile.open(path, mode="r:gz") as tar:
        members = {m.name: m for m in tar.getmembers()}  # WHAT: every entry. WHY: checked
        if "names.json" not in members:
            raise SystemExit(f"{path} has no names.json")  # WHAT: refused. WHY: not a cell part
        names = json.loads(entry_bytes(tar, members["names.json"]))  # WHAT: fingerprint to name. WHY: compared with the pin
        if sorted(names.values()) != sorted(pin["files"]) or any(pin["files"][n] != s for s, n in names.items()):
            raise SystemExit(f"{path} doesn't hold exactly the pinned cell files")  # WHAT: refused. WHY: the release carries the pinned cell only
        if set(members) != {"names.json"} | set(names):
            raise SystemExit(f"{path} holds entries names.json doesn't list")  # WHAT: refused. WHY: nothing unnamed
        for digest in names:
            if sha256(entry_bytes(tar, members[digest])) != digest:
                raise SystemExit(f"{path}: an entry isn't the file its name says")  # WHAT: refused. WHY: every file checked
    return True


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)  # WHAT: the commands. WHY: below
    sub = p.add_subparsers(dest="command", required=True)  # WHAT: one of three. WHY: as listed above
    for name in ("bundle", "kit"):
        s = sub.add_parser(name)  # WHAT: the command. WHY: below
        s.add_argument("--out", required=True)  # WHAT: where it goes. WHY: a file for bundle, a folder for kit
        s.add_argument("--from-zip")  # WHAT: the pinned zip, fetched by hand. WHY: a machine with no route to it
    c = sub.add_parser("check")  # WHAT: the check. WHY: before publishing
    c.add_argument("part")  # WHAT: the cell part. WHY: checked
    a = p.parse_args(argv)
    pin = read_pin()  # WHAT: the pin. WHY: every command starts from it
    if a.command == "check":
        check_part(a.part, pin)
        print(f"{a.part} holds exactly the pinned Citadel cell {pin.get('version', '')}")  # WHAT: said. WHY: plain
        return 0
    data = source_zip(pin, a.from_zip)  # WHAT: the pinned zip, checked. WHY: both commands start from it
    files = pinned_files(pin, data)  # WHAT: the checked files. WHY: a zip missing one is never handed on
    if a.command == "bundle":
        with open(a.out, "wb") as f:
            f.write(pack(files))  # WHAT: the cell part. WHY: signed by publish.py
        print(f"wrote {a.out}: the Citadel cell {pin.get('version', '')}, {len(files)} file(s)")  # WHAT: said. WHY: plain
    else:
        name = os.path.basename((pin.get("source") or {}).get("url", "")) or f"CitadelServer-{pin.get('version', '')}-win-x64-cell.zip"  # WHAT: the zip's own name. WHY: as LatCon's release names it
        os.makedirs(a.out, exist_ok=True)  # WHAT: the kit's folder. WHY: the zip goes in it
        to = os.path.join(a.out, name)  # WHAT: where it goes. WHY: beside the kit's other files
        with open(to, "xb") as f:
            f.write(data)  # WHAT: the pinned zip, as checked. WHY: the install command checks it again against the server's pin
        print(f"wrote {to}: the Citadel cell {pin.get('version', '')}")  # WHAT: said. WHY: plain
    return 0


if __name__ == "__main__":
    sys.exit(main())
