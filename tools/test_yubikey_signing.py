#!/usr/bin/env python3
"""Signing a release on a hardware key (a YubiKey): prepare, then finish.

Run it: python3 tools/test_yubikey_signing.py

No hardware key, no real signing key, no network. A throwaway key made here
stands in for the one on the hardware key: it signs a copy of the signing
request the way the computer the key is plugged into does, and writes a
signatures file. The keys the installed copies trust are stood in for by a
trust.rs written here, listing the throwaway key's public half. GitHub is
stood in for by a few dictionaries.

What it proves:

- A release prepared and finished on the hardware key is, byte for byte, the
  release the key file makes: `sign`, `release` and `promote` each.
- Every refusal `--finish` makes has its own test, and so does every refusal
  `--prepare` makes.
- The key-file path writes exactly what it always has.
"""

import argparse
import base64
import contextlib
import hashlib
import io
import json
import os
import shutil
import sys
import tempfile
import unittest
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)

import publish  # noqa: E402
import release_now  # noqa: E402

from cryptography.hazmat.primitives import serialization  # noqa: E402
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey  # noqa: E402

# The throwaway key the Rust tests use as well ([42u8; 32]), so what is made
# here can be checked against the fixtures there.
THROWAWAY = bytes([42]) * 32
SOMEBODY_ELSE = bytes([9]) * 32
NAME = "test-2026"
SLOT = "9d"
FIXTURES = os.path.join(ROOT, "crates", "hub", "tests", "fixtures")
# What the key file has always written for this release. The Rust side checks
# it the way a seat does (crates/hub/tests/signed_by_the_publishing_script.rs).
TWO_PLATFORMS = os.path.join(FIXTURES, "python-signed-two-platforms.json")
# The request prepare writes for that same release.
REQUEST_FIXTURE = os.path.join(FIXTURES, "python-sign-request.json")
PUBLISHED = "2026-09-23T12:00:00Z"
A_COMMIT = "0123456789abcdef0123456789abcdef01234567"


def private(seed):
    return Ed25519PrivateKey.from_private_bytes(seed)


def public_of(seed):
    return private(seed).public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)


def quietly(do, *args, **kwargs):
    with contextlib.redirect_stdout(io.StringIO()):
        return do(*args, **kwargs)


def read(path):
    with open(path, "rb") as f:
        return f.read()


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def save(path, data):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2)


class FakeGitHub:
    """GitHub, as far as publish.py uses it, kept in memory."""

    def __init__(self, releases=()):
        self.releases = [dict(r, assets=[dict(x) for x in r["assets"]]) for r in releases]
        self.patches = []
        self.ids = 100

    def __call__(self, token_file, repo):  # stands in for the class
        return self

    def next_id(self):
        self.ids += 1
        return self.ids

    def release(self, tag):
        return next((r for r in self.releases if r["tag_name"] == tag), None)

    def assets(self, release):
        return release["assets"]

    def call(self, method, url, body=None, data=None, kind=None, accept=None, timeout=None):
        if method == "POST" and url == "/releases":
            release = dict(body, id=self.next_id(), assets=[], upload_url="https://uploads/{?name}")
            self.releases.append(release)
            return release
        if method == "PATCH" and url.startswith("/releases/"):
            release = next(r for r in self.releases if r["id"] == int(url.rsplit("/", 1)[1]))
            release.update(body)
            self.patches.append(body)
            return release
        if method == "GET" and url.startswith("/releases/assets/"):
            wanted = int(url.rsplit("/", 1)[1])
            return next(x["data"] for r in self.releases for x in r["assets"] if x["id"] == wanted)
        raise AssertionError(f"publish.py asked GitHub for something unexpected: {method} {url}")

    def replace_asset(self, release, path, name=None):
        name = name or os.path.basename(path)
        data = read(path)
        release["assets"] = [x for x in release["assets"] if x["name"] != name]
        release["assets"].append({"id": self.next_id(), "name": name, "state": "uploaded",
                                  "size": len(data), "data": data})

    def files(self, tag):
        return {x["name"]: x["data"] for x in self.release(tag)["assets"]}


class Bench(unittest.TestCase):
    """A folder of pretend programs, a throwaway key file, and a trust.rs
    that trusts the throwaway key."""

    def setUp(self):
        self.dir = tempfile.mkdtemp(prefix="yubikey-signing-")
        self.addCleanup(shutil.rmtree, self.dir, True)
        self.key_file = self.file("throwaway.key", f"{NAME}\n{THROWAWAY.hex()}\n")
        self.trust((NAME, public_of(THROWAWAY)))
        # The same pretend programs the committed fixture was signed over.
        self.app = self.file("app.exe", b"MZ pretend viewer 9.4.0")
        self.server = self.file("server.exe", b"MZ pretend server 9.4.0")
        self.mac = self.file("mac.zip", b"PK pretend mac 9.4.0...")
        self.linux = self.file("linux.bin", b"\x7fELF pretend linux 9.4.0")
        self.dmg = self.file("mac.dmg", b"pretend disk image 9.4.0")
        self.notes = self.file("notes.txt", "Two platforms now.")
        self.kept = os.path.join(self.dir, "dist")
        self.travelled = os.path.join(self.dir, "travelled")
        os.makedirs(self.travelled)
        patch = mock.patch.object(publish, "commit_built", lambda: A_COMMIT)
        patch.start()
        self.addCleanup(patch.stop)

    def file(self, name, content):
        path = os.path.join(self.dir, name)
        with open(path, "wb") as f:
            f.write(content.encode("utf-8") if isinstance(content, str) else content)
        return path

    def trust(self, *keys, before=""):
        """Writes a trust.rs listing these (name, public key) pairs as KEYS."""
        listed = "".join(f'    ("{name}", "{public.hex()}"),\n' for name, public in keys)
        path = self.file("trust.rs", f"{before}pub const KEYS: &[(&str, &str)] = &[\n{listed}];\n")
        patch = mock.patch.object(publish, "TRUST_FILE", path)
        patch.start()
        self.addCleanup(patch.stop)
        return path

    # -- the ways of signing ---------------------------------------------------

    def sign_args(self, **changes):
        a = dict(command="sign", key=self.key_file, prepare=False, finish=None, slot=None, signatures=None,
                 key_name=None, out_dir=self.kept, version="9.4.0", channel="preview",
                 notes_file=self.notes, app=self.app, server=self.server, mac=self.mac, linux=None,
                 out=None, published=PUBLISHED, min_api=1)
        a.update(changes)
        return argparse.Namespace(**a)

    def with_key_file(self, **changes):
        out = os.path.join(self.dir, "signed-with-the-key-file.json")
        quietly(publish.cmd_sign, self.sign_args(out=out, **changes))
        return read(out)

    def prepare(self, **changes):
        """`sign --prepare`, returning the kept request."""
        a = self.sign_args(**dict(dict(key=None, prepare=True, slot=SLOT), **changes))
        before = set(os.listdir(self.kept)) if os.path.isdir(self.kept) else set()
        quietly(publish.cmd_sign, a)
        (made,) = set(os.listdir(self.kept)) - before
        folder = os.path.join(self.kept, made)
        (request,) = [n for n in os.listdir(folder) if n.endswith(".sign-request.json")]
        return os.path.join(folder, request)

    def hardware_key_signs(self, request, seed=THROWAWAY, change=None, change_request=None):
        """What the computer with the hardware key does: takes a COPY of the
        request, signs each message in it, writes a signatures file beside it.
        `change` and `change_request` are for breaking things on purpose."""
        copy = os.path.join(self.travelled, os.path.basename(request))
        shutil.copyfile(request, copy)
        travelled = load(copy)
        if change_request:
            change_request(travelled)
        key = private(seed)
        signatures = {
            "format": "excalibur-signatures/1",
            "product": travelled["product"], "version": travelled["version"],
            "step": travelled["step"], "commit": travelled["commit"],
            "publicKey": base64.b64encode(public_of(seed)).decode(),
            "yubikey": 12345678,
            "signed": "2026-10-16T19:05:00Z",
            "signatures": [],
        }
        for item in travelled["items"]:
            message = base64.b64decode(item["message"])
            signatures["signatures"].append({
                "id": item["id"],
                "messageSha256": hashlib.sha256(message).hexdigest(),
                "signature": base64.b64encode(key.sign(message)).decode(),
            })
        if change:
            change(signatures)
        path = copy.replace(".sign-request.json", ".signatures.json")
        with open(path, "w", encoding="utf-8") as f:
            json.dump(signatures, f, indent=2)
        return path

    def finish(self, request, signatures, **changes):
        out = os.path.join(self.dir, "signed-on-the-hardware-key.json")
        quietly(publish.cmd_sign, self.sign_args(key=None, finish=request, signatures=signatures, out=out,
                                                 **changes))
        return read(out)

    def refused(self, fragment, do, *args, **kwargs):
        with self.assertRaises(publish.Refused) as said:
            quietly(do, *args, **kwargs)
        self.assertIn(fragment, str(said.exception.code))
        return str(said.exception.code)


# ---- the key-file path ------------------------------------------------------

class TheKeyFilePathIsUnchanged(Bench):
    def test_sign_writes_exactly_what_it_always_has(self):
        self.assertEqual(self.with_key_file(), read(TWO_PLATFORMS))

    def test_sign_still_needs_a_version_and_a_program(self):
        with self.assertRaises(SystemExit) as said:
            publish.cmd_sign(self.sign_args(version=None, out=os.path.join(self.dir, "never.json")))
        self.assertIn("--version", str(said.exception.code))


# ---- round trips: the same bytes either way --------------------------------

class TheSameReleaseEitherWay(Bench):
    def test_sign_prepared_and_finished_is_byte_for_byte_what_the_key_file_signs(self):
        request = self.prepare()
        on_the_hardware_key = self.finish(request, self.hardware_key_signs(request))
        self.assertEqual(on_the_hardware_key, self.with_key_file())
        # And that is the manifest the Rust side checks the way a seat does.
        self.assertEqual(on_the_hardware_key, read(TWO_PLATFORMS))

    def test_and_with_every_platform(self):
        request = self.prepare(linux=self.linux, channel="stable")
        self.assertEqual(self.finish(request, self.hardware_key_signs(request)),
                         self.with_key_file(linux=self.linux, channel="stable"))

    def test_finish_writes_into_the_kept_folder_unless_told_otherwise(self):
        request = self.prepare()
        signatures = self.hardware_key_signs(request)
        quietly(publish.cmd_sign, self.sign_args(key=None, finish=request, signatures=signatures))
        kept = os.path.join(os.path.dirname(request), "release.json")
        self.assertEqual(read(kept), read(TWO_PLATFORMS))

    def release_args(self, **changes):
        a = dict(command="release", key=self.key_file, prepare=False, finish=None, slot=None,
                 signatures=None, key_name=None, out_dir=self.kept, token_file="github-token.txt",
                 repo="owner/name", version=None, channel="preview", notes_file=self.notes,
                 app=self.app, server=self.server, mac_zip=self.mac, mac_dmg=self.dmg, linux=self.linux,
                 min_api=1, skip_build=False, skip_tests=False, sign_only=False)
        a.update(changes)
        return argparse.Namespace(**a)

    def a_repository_at(self, version):
        """`release` reads the version out of Cargo.toml and writes release.json
        beside it, so it is given one of its own here."""
        repo = os.path.join(self.dir, "repo")
        os.makedirs(repo, exist_ok=True)
        with open(os.path.join(repo, "Cargo.toml"), "w") as f:
            f.write(f'[workspace.package]\nversion = "{version}"\n')
        patch = mock.patch.object(publish, "repo_root", lambda: repo)
        patch.start()
        self.addCleanup(patch.stop)

    def test_release_prepared_and_finished_publishes_what_the_key_file_publishes(self):
        self.a_repository_at("9.4.0")
        stamp = mock.patch.object(publish, "now", lambda: PUBLISHED)
        stamp.start()
        self.addCleanup(stamp.stop)

        with_key_file = FakeGitHub()
        with mock.patch.object(publish, "GitHub", with_key_file):
            quietly(publish.cmd_release, self.release_args())

        on_hardware_key = FakeGitHub()
        quietly(publish.cmd_release, self.release_args(key=None, prepare=True, slot=SLOT,
                                                       token_file=None, repo=None))
        (folder,) = os.listdir(self.kept)
        request = os.path.join(self.kept, folder, "hyperview-9.4.0-release.sign-request.json")
        with mock.patch.object(publish, "GitHub", on_hardware_key):
            quietly(publish.cmd_release, self.release_args(key=None, finish=request,
                                                           signatures=self.hardware_key_signs(request)))

        self.assertEqual(on_hardware_key.files("v9.4.0"), with_key_file.files("v9.4.0"))
        self.assertEqual(sorted(on_hardware_key.files("v9.4.0")),
                         ["ExcaliburView-Server.exe", "ExcaliburView-linux", "ExcaliburView-mac.dmg",
                          "ExcaliburView-mac.zip", "ExcaliburView.exe", "release.json"])
        self.assertEqual(on_hardware_key.patches, with_key_file.patches)
        manifest = json.loads(on_hardware_key.files("v9.4.0")["release.json"])
        self.assertEqual(len(publish.parts(manifest)), 4)

    def a_preview_release_on_github(self):
        raw = read(TWO_PLATFORMS)
        return [{"id": 7, "tag_name": "v9.4.0", "draft": False, "prerelease": True,
                 "upload_url": "https://uploads/{?name}",
                 "assets": [{"id": 70, "name": "release.json", "state": "uploaded", "size": len(raw), "data": raw}]}]

    def promote_args(self, **changes):
        a = dict(command="promote", key=self.key_file, prepare=False, finish=None, slot=None, signatures=None,
                 key_name=None, out_dir=self.kept, token_file="github-token.txt", repo="owner/name",
                 version="9.4.0")
        a.update(changes)
        return argparse.Namespace(**a)

    def promoted_on_the_hardware_key(self, github, change_between=None):
        with mock.patch.object(publish, "GitHub", github):
            quietly(publish.cmd_promote, self.promote_args(key=None, prepare=True, slot=SLOT))
            (folder,) = os.listdir(self.kept)
            request = os.path.join(self.kept, folder, "hyperview-9.4.0-promote.sign-request.json")
            if change_between:
                change_between(request)
            quietly(publish.cmd_promote, self.promote_args(key=None, version=None, finish=request,
                                                           signatures=self.hardware_key_signs(request)))
        return request

    def test_promote_prepared_and_finished_publishes_what_the_key_file_publishes(self):
        with_key_file = FakeGitHub(self.a_preview_release_on_github())
        with mock.patch.object(publish, "GitHub", with_key_file):
            quietly(publish.cmd_promote, self.promote_args())

        on_hardware_key = FakeGitHub(self.a_preview_release_on_github())
        self.promoted_on_the_hardware_key(on_hardware_key)

        stable = on_hardware_key.files("v9.4.0")["release.json"]
        self.assertEqual(stable, with_key_file.files("v9.4.0")["release.json"])
        self.assertEqual(on_hardware_key.patches, with_key_file.patches)
        manifest = json.loads(stable)
        self.assertEqual({part["channel"] for part in publish.every_part(manifest)}, {"stable"})
        for part in publish.every_part(manifest):
            publish.verify(part, private(THROWAWAY).public_key())

    def test_promote_refuses_when_the_release_changed_after_prepare(self):
        github = FakeGitHub(self.a_preview_release_on_github())

        def somebody_reuploads(request):
            raw = json.loads(github.release("v9.4.0")["assets"][0]["data"])
            raw["app"]["notes"] = "Different notes."
            github.release("v9.4.0")["assets"][0]["data"] = json.dumps(raw).encode()

        said = self.refused("has changed since this promotion was prepared",
                            self.promoted_on_the_hardware_key, github, somebody_reuploads)
        self.assertIn("Prepare the promotion again", said)
        self.assertEqual(github.patches, [], "nothing was published")

    def test_promote_refuses_a_kept_request_edited_after_prepare(self):
        github = FakeGitHub(self.a_preview_release_on_github())

        def edit_the_kept_request(request):
            kept = load(request)
            kept["items"][0]["message"] = base64.b64encode(b"something else entirely").decode()
            save(request, kept)

        self.refused("no longer matches the release it was prepared from",
                     self.promoted_on_the_hardware_key, github, edit_the_kept_request)
        self.assertEqual(github.patches, [])


# ---- what prepare writes and keeps ------------------------------------------

class WhatPrepareWrites(Bench):
    def test_the_request_is_what_the_contract_says(self):
        request = load(self.prepare())
        self.assertEqual(request["format"], "excalibur-sign-request/1")
        self.assertEqual(request["product"], "hyperview")
        self.assertEqual((request["version"], request["step"], request["slot"]), ("9.4.0", "release", SLOT))
        self.assertEqual(request["commit"], A_COMMIT)
        self.assertRegex(request["made"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
        # Raw 32 bytes, the key the installed copies trust.
        self.assertEqual(base64.b64decode(request["publicKey"]), public_of(THROWAWAY))
        manifest = load(TWO_PLATFORMS)
        self.assertEqual([item["id"] for item in request["items"]],
                         ["windows-x64", "windows-x64-server", "macos-universal"])
        for item in request["items"]:
            part = next(p for p in publish.parts(manifest) if p["platform"] == item["id"])
            # Exactly the bytes the key file signs, which are the bytes a seat checks.
            self.assertEqual(base64.b64decode(item["message"]), publish.payload(part))
            self.assertLessEqual(len(item["what"]), 200)
            self.assertIn(part["download"], item["what"])

    def test_the_request_has_no_byte_order_mark_and_ends_in_a_newline(self):
        raw = read(self.prepare())
        self.assertFalse(raw.startswith(b"\xef\xbb\xbf"))
        self.assertTrue(raw.endswith(b"}\n"))

    def test_the_messages_are_the_ones_the_rust_side_checks(self):
        """crates/hub/tests/signed_by_the_publishing_script.rs reads this
        fixture and checks each message is `Release::signing_payload` of the
        build it names, worked out in Rust."""
        made = load(self.prepare())
        fixture = load(REQUEST_FIXTURE)
        self.assertEqual(made["items"], fixture["items"])
        self.assertEqual(made["publicKey"], fixture["publicKey"])

    def test_the_programs_are_kept_beside_it_under_their_names_on_the_release(self):
        request = self.prepare(mac=self.mac, linux=self.linux)
        kept = os.path.join(os.path.dirname(request), "programs")
        for name, original in (("ExcaliburView.exe", self.app), ("ExcaliburView-Server.exe", self.server),
                               ("ExcaliburView-mac.zip", self.mac), ("ExcaliburView-linux", self.linux)):
            self.assertEqual(read(os.path.join(kept, name)), read(original))
        self.assertTrue(os.path.exists(os.path.join(os.path.dirname(request), "release.unsigned.json")))

    def test_preparing_again_makes_a_new_folder_and_leaves_the_first_alone(self):
        first = self.prepare()
        before = read(first)
        second = self.prepare()
        self.assertNotEqual(os.path.dirname(first), os.path.dirname(second))
        self.assertEqual(read(first), before)
        self.assertTrue(os.path.dirname(second).endswith("hyperview-9.4.0-release-2"))


class PrepareRefuses(Bench):
    def test_a_slot_that_is_not_a_slot(self):
        for slot in (None, "", "8w", "9b", "96", "slot 9d", "0x9d"):
            with self.subTest(slot=slot):
                self.refused("is not a slot a hardware key signs from", self.prepare, slot=slot)
        self.assertFalse(os.path.exists(self.kept), "nothing was kept for a refused prepare")

    def test_a_slot_is_taken_whatever_case_it_is_typed_in(self):
        request = load(self.prepare(slot="9D"))
        self.assertEqual(request["slot"], "9d")

    def test_a_version_a_seat_cannot_compare(self):
        for version in ("latest", "1.2", "1.2.3.4", "../1.2.3"):
            with self.subTest(version=version):
                self.refused("is not a version a seat can compare", self.prepare, version=version)

    def test_a_program_that_is_not_there(self):
        self.refused("is not there", self.prepare, server=os.path.join(self.dir, "no-such.exe"))
        self.assertFalse(os.path.exists(self.kept))

    def test_when_the_program_trusts_no_release_key(self):
        self.trust()
        self.refused("trusts no release key", self.prepare)

    def test_when_it_trusts_several_and_none_is_named(self):
        self.trust(("old-2025", public_of(SOMEBODY_ELSE)), (NAME, public_of(THROWAWAY)))
        self.refused("Say which with --key-name", self.prepare)
        self.refused("is not a release key the program trusts", self.prepare, key_name="nobody-2026")
        request = load(self.prepare(key_name=NAME))
        self.assertEqual(base64.b64decode(request["publicKey"]), public_of(THROWAWAY))

    def test_two_different_builds_that_say_they_are_one_platform(self):
        manifest = load(TWO_PLATFORMS)
        manifest["apps"][1]["platform"] = "windows-x64"
        self.refused("both say they are windows-x64", publish.to_be_signed, manifest)


class TheKeysTheProgramTrusts(Bench):
    def test_the_real_list_is_read_and_it_is_the_release_keys_alone(self):
        keys = publish.trusted_release_keys(os.path.join(ROOT, "crates", "hyperview", "src", "trust.rs"))
        self.assertGreaterEqual(len(keys), 1)
        names = [name for name, _ in keys]
        for name, public in keys:
            self.assertEqual(len(public), 32, name)
        # trust.rs also lists the licence keys, which can never vouch for a release.
        self.assertNotIn("excalibur-licences-2026", names)

    def test_a_commented_out_key_is_not_trusted(self):
        path = self.trust((NAME, public_of(THROWAWAY)))
        text = read(path).decode().replace("];", f'    // ("old-2025", "{public_of(SOMEBODY_ELSE).hex()}"),\n];', 1)
        self.file("trust.rs", text)
        self.assertEqual(publish.trusted_release_keys(), [(NAME, public_of(THROWAWAY))])

    def test_a_malformed_key_is_left_out_rather_than_trusted(self):
        self.trust((NAME, public_of(THROWAWAY)), ("short", b"\x01\x02"))
        self.assertEqual(publish.trusted_release_keys(), [(NAME, public_of(THROWAWAY))])

    def test_licence_and_plugin_keys_are_never_release_keys(self):
        others = (f'pub const LICENSE_KEYS: &[(&str, &str)] = &[\n    ("licences", "{public_of(SOMEBODY_ELSE).hex()}"),\n];\n'
                  f'pub const PLUGIN_KEYS: &[(&str, &str)] = &[\n    ("plugins", "{public_of(SOMEBODY_ELSE).hex()}"),\n];\n')
        # Ahead of KEYS, so only reading exactly `KEYS` gets this right.
        self.trust((NAME, public_of(THROWAWAY)), before=others)
        self.assertEqual(publish.trusted_release_keys(), [(NAME, public_of(THROWAWAY))])

    def test_a_trust_file_with_no_list_is_refused(self):
        path = self.trust()
        self.file(os.path.basename(path), "// nothing here\n")
        self.refused("has no KEYS list in it", publish.trusted_release_keys)


# ---- every refusal finish makes ---------------------------------------------

class FinishRefuses(Bench):
    """Each check `--finish` makes, each broken once. A refusal writes
    nothing: there is no manifest afterwards."""

    def setUp(self):
        super().setUp()
        self.request = self.prepare()

    def refuses(self, fragment, signatures):
        said = self.refused(fragment, self.finish, self.request, signatures)
        self.assertFalse(os.path.exists(os.path.join(self.dir, "signed-on-the-hardware-key.json")),
                         "a refused finish wrote a manifest anyway")
        return said

    def signed_with(self, change):
        return self.hardware_key_signs(self.request, change=change)

    def test_a_wrong_signature(self):
        def flip_a_bit(s):
            raw = bytearray(base64.b64decode(s["signatures"][1]["signature"]))
            raw[10] ^= 1
            s["signatures"][1]["signature"] = base64.b64encode(bytes(raw)).decode()
        said = self.refuses("signature for windows-x64-server does not check out", self.signed_with(flip_a_bit))
        self.assertIn("Nothing was signed or published", said)

    def test_a_signature_by_another_key_claiming_to_be_the_trusted_one(self):
        def claim_the_trusted_key(s):
            s["publicKey"] = base64.b64encode(public_of(THROWAWAY)).decode()
        signed = self.hardware_key_signs(self.request, seed=SOMEBODY_ELSE, change=claim_the_trusted_key)
        self.refuses("does not check out against the release key the program trusts", signed)

    def test_a_signatures_file_naming_another_key(self):
        signed = self.hardware_key_signs(self.request, seed=SOMEBODY_ELSE)
        self.refuses("names a public key that is not the release key the program trusts", signed)

    def test_an_id_missing(self):
        self.refuses("has no signature for macos-universal",
                     self.signed_with(lambda s: s["signatures"].pop(2)))

    def test_an_extra_id(self):
        def one_more(s):
            s["signatures"].append(dict(s["signatures"][0], id="linux-x64"))
        self.refuses("has signatures for linux-x64, which this release did not ask for", self.signed_with(one_more))

    def test_an_id_twice(self):
        self.refuses("has two signatures for windows-x64",
                     self.signed_with(lambda s: s["signatures"].append(dict(s["signatures"][0]))))

    def test_a_changed_message_hash(self):
        def another_hash(s):
            s["signatures"][0]["messageSha256"] = hashlib.sha256(b"something else").hexdigest()
        self.refuses("signed over something other than windows-x64's message", self.signed_with(another_hash))

    def test_a_message_hash_in_capitals(self):
        def capitals(s):
            s["signatures"][0]["messageSha256"] = s["signatures"][0]["messageSha256"].upper()
        self.refuses("signed over something other than windows-x64's message", self.signed_with(capitals))

    def test_the_copy_that_travelled_is_never_read(self):
        """Somebody changes the request on its way to the hardware key, so the
        key signs a different message. Finish reads its own kept copy and
        refuses."""
        def a_different_build(travelled):
            travelled["items"][0]["message"] = base64.b64encode(b"hyperview-release-v1\nversion=6.6.6\n").decode()
        signed = self.hardware_key_signs(self.request, change_request=a_different_build)
        self.refuses("signed over something other than windows-x64's message", signed)

    def test_a_build_changed_after_prepare(self):
        signed = self.hardware_key_signs(self.request)
        program = os.path.join(os.path.dirname(self.request), "programs", "ExcaliburView-mac.zip")
        with open(program, "ab") as f:
            f.write(b"!")
        said = self.refuses("ExcaliburView-mac.zip has changed since this release was prepared", signed)
        self.assertIn("Prepare the release again", said)

    def test_a_build_gone_after_prepare(self):
        signed = self.hardware_key_signs(self.request)
        os.remove(os.path.join(os.path.dirname(self.request), "programs", "ExcaliburView.exe"))
        self.refuses("ExcaliburView.exe is no longer in", signed)

    def test_the_kept_release_changed_after_prepare(self):
        signed = self.hardware_key_signs(self.request)
        unsigned = os.path.join(os.path.dirname(self.request), "release.unsigned.json")
        manifest = load(unsigned)
        manifest["server"]["minimum_api_version"] = 2
        save(unsigned, manifest)
        self.refuses("no longer matches the release kept beside it", signed)

    def test_the_kept_request_changed_after_prepare(self):
        signed = self.hardware_key_signs(self.request)
        kept = load(self.request)
        kept["items"][1]["message"] = base64.b64encode(b"hyperview-release-v1\nsomething else\n").decode()
        save(self.request, kept)
        self.refuses("no longer matches the release kept beside it", signed)

    def test_another_product(self):
        self.refuses("is for product 'fabwire'", self.signed_with(lambda s: s.update(product="fabwire")))

    def test_another_version(self):
        self.refuses("is for version '9.4.1'", self.signed_with(lambda s: s.update(version="9.4.1")))

    def test_another_step(self):
        self.refuses("is for step 'promote'", self.signed_with(lambda s: s.update(step="promote")))

    def test_another_commit(self):
        self.refuses("is for commit", self.signed_with(lambda s: s.update(commit="f" * 40)))

    def test_the_wrong_format(self):
        self.refuses("is not in the format this reads",
                     self.signed_with(lambda s: s.update(format="excalibur-signatures/2")))

    def test_a_malformed_signatures_file(self):
        broken = {
            "not JSON at all": (b"{ this is not json", "is not readable JSON"),
            "a byte-order mark": (b"\xef\xbb\xbf{}", "starts with a byte-order mark"),
            "a list, not an object": (b"[]", "is not a signatures file"),
            "no public key": (None, "public key is missing"),
            "no list of signatures": (None, "has no list of signatures"),
            "a signature with no id": (None, "has a signature with no id"),
            "a signature that is not base64": (None, "signature for windows-x64 is not base64"),
            "a signature 63 bytes long": (None, "signature for windows-x64 that is not 64 bytes"),
        }
        changes = {
            "no public key": lambda s: s.pop("publicKey"),
            "no list of signatures": lambda s: s.update(signatures={"windows-x64": "x"}),
            "a signature with no id": lambda s: s["signatures"][0].pop("id"),
            "a signature that is not base64": lambda s: s["signatures"][0].update(signature="not base64!"),
            "a signature 63 bytes long": lambda s: s["signatures"][0].update(
                signature=base64.b64encode(base64.b64decode(s["signatures"][0]["signature"])[:63]).decode()),
        }
        for what, (raw, fragment) in broken.items():
            with self.subTest(what):
                if raw is not None:
                    path = os.path.join(self.travelled, "broken.signatures.json")
                    with open(path, "wb") as f:
                        f.write(raw)
                else:
                    path = self.signed_with(changes[what])
                self.refuses(fragment, path)

    def test_a_signatures_file_that_is_not_there(self):
        self.refuses("could not read the signatures file", os.path.join(self.dir, "nowhere.json"))

    def test_a_request_for_a_key_the_program_no_longer_trusts(self):
        signed = self.hardware_key_signs(self.request)
        self.trust(("new-2027", public_of(SOMEBODY_ELSE)))
        self.refuses("prepared for a key the program does not trust", signed)

    def test_a_promote_request_cannot_be_finished_as_a_release(self):
        kept = load(self.request)
        kept["step"] = "promote"
        save(self.request, kept)
        self.refuses("is for the 'promote' step, not 'release'", self.hardware_key_signs(self.request))

    def test_a_malformed_kept_request(self):
        """Every rule a request is held to, each broken once."""
        def item(**changes):
            return lambda r: r["items"][0].update(changes)
        broken = {
            "the wrong format": (lambda r: r.update(format="something/1"), "is not in the format this reads"),
            "another product": (lambda r: r.update(product="arcvalley"), "is for 'arcvalley'"),
            "no version": (lambda r: r.update(version=""), "names no version"),
            "a step with capitals": (lambda r: r.update(step="Release"), "has no proper step name"),
            "a step too long": (lambda r: r.update(step="r" * 41), "has no proper step name"),
            "a short commit": (lambda r: r.update(commit="0123abc"), "not a full git commit id"),
            "no slot": (lambda r: r.pop("slot"), "names no slot"),
            "a public key of 31 bytes": (lambda r: r.update(publicKey=base64.b64encode(b"k" * 31).decode()),
                                         "public key that is not 32 bytes"),
            "nothing to sign": (lambda r: r.update(items=[]), "must ask for 1 to 100 signatures"),
            "too much to sign": (lambda r: r.update(items=[dict(r["items"][0], id=f"x{n}") for n in range(101)]),
                                 "must ask for 1 to 100 signatures"),
            "an item that is not an item": (lambda r: r["items"].append("windows-x64"), "has an item that is not an item"),
            "an id with capitals": (item(id="Windows-x64"), "has an item called 'Windows-x64'"),
            "an id twice": (lambda r: r["items"].append(dict(r["items"][0])), "asks for windows-x64 twice"),
            "nothing said about it": (item(what="  "), "does not say what windows-x64 is"),
            "too much said about it": (item(what="x" * 201), "does not say what windows-x64 is"),
            "an empty message": (item(message=""), "empty or over 1 MiB"),
            "a message over 1 MiB": (item(message=base64.b64encode(b"m" * (1024 * 1024 + 1)).decode()),
                                     "empty or over 1 MiB"),
            "a message that is not base64": (item(message="%%%"), "message for windows-x64 is not base64"),
        }
        original = read(self.request)
        for what, (change, fragment) in broken.items():
            with self.subTest(what):
                kept = json.loads(original)
                change(kept)
                with open(self.request, "w") as f:
                    json.dump(kept, f)
                self.refused(fragment, publish.finish_release, self.request, os.path.join(self.dir, "unused"))
        with open(self.request, "wb") as f:
            f.write(b"[]")
        self.refused("is not a signing request", publish.finish_release, self.request, os.path.join(self.dir, "unused"))
        with open(self.request, "wb") as f:
            f.write(b"not json")
        self.refused("is not readable JSON", publish.finish_release, self.request, os.path.join(self.dir, "unused"))

    def test_and_with_everything_right_it_finishes(self):
        """The control for every test above: the same request, signed
        properly, finishes."""
        self.assertEqual(self.finish(self.request, self.hardware_key_signs(self.request)),
                         read(TWO_PLATFORMS))


# ---- the double-click release ----------------------------------------------

class TheDoubleClickRelease(unittest.TestCase):
    """release_now.py hands prepare and finish to publish.py. What it hands
    over is checked here; publish.py's own tests above check the rest."""

    def setUp(self):
        self.ran = []
        # Nothing here may reach git; a test that needs it to say yes says so.
        self.pulled = []
        patch = mock.patch.object(release_now.subprocess, "call", lambda *a, **k: self.pulled.append(a) or 1)
        patch.start()
        self.addCleanup(patch.stop)
        for name, value in (("run", lambda args: self.ran.append(args) or 0),
                            ("wait_to_close", lambda: None),
                            ("find_file", lambda names, extra_dirs=(): "/somewhere/" + names[0]),
                            ("version_in_cargo", lambda: "9.4.0")):
            patch = mock.patch.object(release_now, name, value)
            patch.start()
            self.addCleanup(patch.stop)

    def main(self, *argv):
        with mock.patch.object(sys, "argv", ["release_now.py", *argv]):
            quietly(release_now.main)
        (args,) = self.ran
        return args[2:]

    def test_finish_hands_the_kept_request_and_the_signatures_to_publish(self):
        self.assertEqual(
            self.main("--finish", "dist/r/req.sign-request.json", "--signatures", "sigs.json"),
            ["release", "--finish", os.path.abspath("dist/r/req.sign-request.json"),
             "--signatures", os.path.abspath("sigs.json"),
             "--token-file", "/somewhere/github-token.txt", "--repo", release_now.REPO])

    def test_promote_finish_does_the_same_for_a_promotion(self):
        self.assertEqual(
            self.main("--promote", "--finish", "req.json", "--signatures", "sigs.json"),
            ["promote", "--finish", os.path.abspath("req.json"), "--signatures", os.path.abspath("sigs.json"),
             "--token-file", "/somewhere/github-token.txt", "--repo", release_now.REPO])

    def test_promote_prepare_needs_no_key_file(self):
        with mock.patch("builtins.input", lambda prompt="": "yes"), \
                mock.patch.object(release_now.subprocess, "call", lambda *a, **k: 0), \
                mock.patch.object(release_now.shutil, "which", lambda name: "/usr/bin/" + name), \
                mock.patch.object(release_now, "windows_build_from_github", lambda *a: (None, None)), \
                mock.patch.object(release_now, "mac_build_from_github", lambda *a: (None, None)), \
                mock.patch.object(release_now, "linux_build_from_github", lambda *a: None), \
                mock.patch.object(release_now, "find_by_suffix", lambda *a, **k: None), \
                mock.patch.object(release_now, "open", mock.mock_open(read_data="token"), create=True):
            handed = self.main("--promote", "--prepare", "--slot", "9d")
        self.assertEqual(handed, ["promote", "--prepare", "--slot", "9d",
                                  "--token-file", "/somewhere/github-token.txt", "--repo", release_now.REPO,
                                  "--version", "9.4.0"])

    def stops(self, *argv):
        said = io.StringIO()
        with self.assertRaises(SystemExit), mock.patch.object(sys, "argv", ["release_now.py", *argv]), \
                contextlib.redirect_stdout(said):
            release_now.main()
        self.assertEqual(self.ran, [], "publish.py was run anyway")
        self.assertEqual(self.pulled, [], "it went on to git anyway")
        return said.getvalue()

    def test_prepare_without_a_slot_stops_before_anything_else(self):
        self.assertIn("--prepare needs --slot", self.stops("--prepare"))

    def test_finish_without_signatures_stops_before_anything_else(self):
        self.assertIn("--finish needs the signing request", self.stops("--finish", "req.json"))
        self.assertIn("--finish needs the signing request", self.stops("--finish", "--signatures", "s.json"))


if __name__ == "__main__":
    unittest.main()
