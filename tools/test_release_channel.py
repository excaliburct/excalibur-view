#!/usr/bin/env python3
"""A release is for everyone unless asked otherwise (0.7.2).

Run it: python3 tools/test_release_channel.py

0.7.1 went up as an early release: a GitHub prerelease with "preview" in its
release.json. Mesa Fab's office server, set to get them early, took it; every
desktop on the stable channel skips prereleases, so none could follow, and the
device list then shut them out of the server. These tests hold the fix: a
normal release is signed "stable" and published as a full GitHub release, and
"preview" only when it is asked for. No network and no real key.
"""
import json  # WHAT: reads the manifest. WHY: its channel is checked
import os  # WHAT: paths. WHY: the manifest's place
import sys  # WHAT: the module path. WHY: imports the scripts beside this
import unittest  # WHAT: the test runner. WHY: as the repo's other tool tests
from unittest import mock  # WHAT: stands a fake in for GitHub. WHY: put back by itself, even when a test fails

HERE = os.path.dirname(os.path.abspath(__file__))  # WHAT: this folder. WHY: the scripts are beside it
sys.path.insert(0, HERE)  # WHAT: importable. WHY: as above

import publish  # noqa: E402  WHAT: the publishing script. WHY: under test
import release_now  # noqa: E402  WHAT: the double-click release. WHY: under test
from test_yubikey_signing import Bench, FakeGitHub, quietly  # noqa: E402  WHAT: the throwaway key, pretend programs and pretend GitHub. WHY: the same bench the signing tests use


class TheDefaultIsStable(unittest.TestCase):
    def test_sign_and_release_default_to_stable(self):
        for command in (["sign", "--key", "k"], ["release", "--key", "k"]):  # WHAT: both commands that pick a channel. WHY: either could publish
            self.assertEqual(publish.parser().parse_args(command).channel, "stable", command)  # WHAT: stable unless asked. WHY: the fix

    def test_preview_only_when_asked(self):
        self.assertEqual(publish.parser().parse_args(["release", "--key", "k", "--channel", "preview"]).channel, "preview")  # WHAT: asked for. WHY: early releases still work

    def test_the_double_click_release_is_stable_unless_preview_is_typed(self):
        self.assertEqual(release_now.channel_from([]), "stable")  # WHAT: a plain double-click. WHY: the fix
        self.assertEqual(release_now.channel_from(["--prepare", "--slot", "9d"]), "stable")  # WHAT: the hardware-key path. WHY: --prepare is not --preview
        self.assertEqual(release_now.channel_from(["--preview"]), "preview")  # WHAT: asked for. WHY: early releases still work

    def test_github_is_told_a_stable_release_is_full_and_the_latest(self):
        self.assertEqual(publish.github_flags("stable"), {"prerelease": False, "make_latest": "true"})  # WHAT: a full release. WHY: stable seats skip prereleases
        self.assertEqual(publish.github_flags("preview"), {"prerelease": True, "make_latest": "false"})  # WHAT: an early one. WHY: as before


class AReleaseSignedByDefault(Bench):
    def test_is_signed_stable_and_published_as_a_full_release(self):
        default = publish.parser().parse_args(["sign", "--key", "k"]).channel  # WHAT: the channel nobody chose. WHY: what a normal release gets
        out = os.path.join(self.dir, "release.json")  # WHAT: where the manifest goes. WHY: uploaded below
        quietly(publish.cmd_sign, self.sign_args(channel=default, out=out))  # WHAT: signed with the throwaway key. WHY: as a release is
        with open(out, encoding="utf-8") as f:
            manifest = json.load(f)  # WHAT: what was signed. WHY: checked
        self.assertTrue(all(part["channel"] == "stable" for part in publish.parts(manifest)), "every build signed stable")  # WHAT: stable in release.json. WHY: a stable seat skips a manifest saying preview
        github = FakeGitHub()  # WHAT: GitHub, in memory. WHY: no network
        with mock.patch.object(publish, "GitHub", github):
            quietly(publish.cmd_upload, mock.Mock(token_file="t", repo="o/r", version="9.4.0", notes_file=None,
                                                  files=[out, self.app, self.server, self.mac], extra=[]))  # WHAT: uploaded and published. WHY: as `release` does
        release = github.release("v9.4.0")  # WHAT: what GitHub now shows. WHY: checked
        self.assertIs(release["prerelease"], False, "a full release, which every stable seat sees")  # WHAT: not a prerelease. WHY: the root cause at Mesa Fab
        self.assertEqual(github.patches[-1]["make_latest"], "true", "and the latest")  # WHAT: the latest. WHY: the website's download page follows it
        self.assertIs(github.patches[-1]["draft"], False)  # WHAT: public. WHY: published


if __name__ == "__main__":
    unittest.main()  # WHAT: runs them. WHY: python3 tools/test_release_channel.py
