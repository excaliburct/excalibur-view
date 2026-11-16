#!/usr/bin/env python3
"""The Citadel cell's packing and its place in a signed release.

Run it: python3 tools/test_citadel_cell.py

No network and no real LatCon release: a zip made here stands in for the
pinned one, and a pin written here for packaging/citadel-cell.json.
"""

import hashlib  # WHAT: SHA-256. WHY: the pins
import io  # WHAT: bytes as files. WHY: zips in memory
import json  # WHAT: the pin. WHY: written here
import os  # WHAT: paths. WHY: temporary files
import sys  # WHAT: the import path. WHY: the tools beside this file
import tarfile  # WHAT: the cell part. WHY: a planted entry is added
import tempfile  # WHAT: a scratch folder. WHY: files written in the tests
import unittest  # WHAT: the test runner. WHY: as the other tool tests
import zipfile  # WHAT: a stand-in LatCon zip. WHY: the source of the cell
from unittest import mock  # WHAT: patching. WHY: the pin's path

HERE = os.path.dirname(os.path.abspath(__file__))  # WHAT: tools/. WHY: the modules under test
sys.path.insert(0, HERE)  # WHAT: importable. WHY: below

import citadel_cell  # noqa: E402  # WHAT: the module under test. WHY: its checks
import publish  # noqa: E402  # WHAT: the release script. WHY: the cell's place in a manifest

CELL = b"MZ the cell program"  # WHAT: a stand-in cell. WHY: pinned below


def sha(b):
    return hashlib.sha256(b).hexdigest()  # WHAT: lower-case hex. WHY: as pinned


class Cell(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()  # WHAT: a scratch folder. WHY: the pin and parts go in it
        buf = io.BytesIO()  # WHAT: the stand-in LatCon zip. WHY: the source
        with zipfile.ZipFile(buf, "w") as z:
            z.writestr("Citadel Server.exe", CELL)  # WHAT: the program. WHY: pinned
            z.writestr("readme.txt", b"not part of the cell")  # WHAT: something else in the zip. WHY: never used
        self.zip = buf.getvalue()  # WHAT: its bytes. WHY: pinned
        self.pin_path = os.path.join(self.dir, "citadel-cell.json")  # WHAT: the pin's place. WHY: patched in
        with open(self.pin_path, "w", encoding="utf-8") as f:
            json.dump({"version": "1.1.0", "source": {"url": "https://example.invalid/latcon.zip", "sha256": sha(self.zip)}, "files": {"Citadel Server.exe": sha(CELL)}}, f)  # WHAT: a pin. WHY: as packaging/citadel-cell.json
        self.pin = citadel_cell.read_pin(self.pin_path)  # WHAT: read. WHY: used below

    def test_an_empty_pin_carries_no_cell(self):
        empty = os.path.join(self.dir, "empty.json")  # WHAT: the pin as committed today. WHY: no cell yet
        with open(empty, "w", encoding="utf-8") as f:
            json.dump({"version": "", "source": {"url": "", "sha256": ""}, "files": {}}, f)
        with self.assertRaises(SystemExit):
            citadel_cell.read_pin(empty)  # WHAT: refused. WHY: never a cell nobody pinned

    def test_only_the_pinned_zip_and_files_are_taken(self):
        files = citadel_cell.pinned_files(self.pin, citadel_cell.source_zip(self.pin, from_zip=self._write("z.zip", self.zip)))  # WHAT: taken out. WHY: the pinned file only
        self.assertEqual(files, {"Citadel Server.exe": CELL})  # WHAT: just the program. WHY: nothing else in the zip
        with self.assertRaises(SystemExit):
            citadel_cell.source_zip(self.pin, from_zip=self._write("other.zip", self.zip + b"x"))  # WHAT: another zip refused. WHY: only the pinned one
        wrong = dict(self.pin, files={"Citadel Server.exe": sha(b"else")})  # WHAT: a pin that disagrees with the file. WHY: the file is checked too
        with self.assertRaises(SystemExit):
            citadel_cell.pinned_files(wrong, self.zip)

    def test_a_packed_part_checks_and_a_planted_entry_does_not(self):
        part = self._write("part.tar.gz", citadel_cell.pack({"Citadel Server.exe": CELL}))  # WHAT: a cell part. WHY: as published
        self.assertTrue(citadel_cell.check_part(part, self.pin))  # WHAT: exactly the pinned files. WHY: signed
        planted = os.path.join(self.dir, "planted.tar.gz")  # WHAT: a part with an extra entry. WHY: the refusal
        with tarfile.open(planted, "w:gz", format=tarfile.USTAR_FORMAT) as out, tarfile.open(part, "r:gz") as src:
            for m in src.getmembers():
                out.addfile(m, src.extractfile(m))  # WHAT: the real entries. WHY: copied
            extra = tarfile.TarInfo(sha(b"evil"))  # WHAT: an unlisted file. WHY: planted
            extra.size = 4
            out.addfile(extra, io.BytesIO(b"evil"))
        with self.assertRaises(SystemExit):
            citadel_cell.check_part(planted, self.pin)  # WHAT: refused. WHY: nothing unnamed

    def test_the_public_release_never_carries_the_cell(self):
        part = self._write(publish.CITADEL_FILE, citadel_cell.pack({"Citadel Server.exe": CELL}))  # WHAT: the cell part. WHY: offered to the release
        app, server = self._write("a.exe", b"MZ app"), self._write("s.exe", b"MZ server")  # WHAT: stand-in programs. WHY: the release
        manifest = publish.unsigned_manifest("0.7.1", "stable", "", "2026-11-13T00:00:00Z", 1, {publish.APP_PLATFORM: app, publish.SERVER_PLATFORM: server})  # WHAT: the release without the cell. WHY: needs no --citadel any more
        self.assertNotIn("servers", manifest)  # WHAT: no cell part listed. WHY: the public feed never names it
        self.assertEqual(manifest["server"]["platform"], "windows-x64-server")  # WHAT: the server where every server reads it. WHY: unchanged
        with self.assertRaises(SystemExit) as refused:
            publish.unsigned_manifest("0.7.1", "stable", "", "x", 1, {publish.APP_PLATFORM: app, publish.SERVER_PLATFORM: server, publish.CITADEL_PLATFORM: part})  # WHAT: the cell offered. WHY: refused
        self.assertIn("never goes in View's public release", str(refused.exception))  # WHAT: said. WHY: plain

    def test_the_kit_holds_the_pinned_zip_itself(self):
        out = os.path.join(self.dir, "kit")  # WHAT: the IT kit's folder. WHY: the zip goes in it
        with mock.patch.object(citadel_cell, "PIN", self.pin_path):
            citadel_cell.main(["kit", "--out", out, "--from-zip", self._write("z.zip", self.zip)])  # WHAT: the kit made. WHY: the install command takes the zip
        with open(os.path.join(out, "latcon.zip"), "rb") as f:
            self.assertEqual(f.read(), self.zip)  # WHAT: the pinned zip, as it was. WHY: the server checks its SHA-256 against the pin

    def _write(self, name, data):
        path = os.path.join(self.dir, name)  # WHAT: a file in the scratch folder. WHY: the tools read files
        with open(path, "wb") as f:
            f.write(data)
        return path


if __name__ == "__main__":
    unittest.main()
