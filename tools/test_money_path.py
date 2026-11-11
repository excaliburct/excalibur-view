#!/usr/bin/env python3
"""The path from a paid order to a server holding the licence.

Run it: python tools/test_money_path.py

No Square, no signing key, no network. What it covers is the part that was
missing and the part most likely to drift: where a licence is written, under
what name, and whether pushing it actually happens. Signing itself is tested
elsewhere and needs the key on the signing machine.
"""
import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import publish  # noqa: E402
import square  # noqa: E402


def a_licence(id="evl_abc123", company="Example Steel, Inc.", users=6):
    return {
        "id": id,
        "company": company,
        "users": users,
        "edition": "office",
        "updates_through": "2027-09-24",
        "signature": "not a real one, and this test does not need one",
    }


class WhereALicenceIsPublished(unittest.TestCase):
    def test_the_name_is_the_one_the_server_asks_for(self):
        # Pinned on the Rust side too, in license.rs. If these ever drift
        # apart every server quietly stops finding its own renewal and nobody
        # gets an error to read.
        self.assertEqual(
            publish.license_filename_for("evl_abc123"),
            "19431c5846060c7cb52d58f6af8352ed35243c827a01b2703ca4c850160aaebd.evlicense",
        )

    def test_a_stray_space_is_not_a_lost_licence(self):
        self.assertEqual(
            publish.license_filename_for(" evl_abc123 "),
            publish.license_filename_for("evl_abc123"),
        )

    def test_the_company_name_cannot_be_worked_out_from_the_address(self):
        one = publish.license_filename_for("evl_example_steel")
        two = publish.license_filename_for("evl_example_steel_2")
        self.assertNotIn("mesa", one)
        self.assertNotEqual(one, two)

    def test_publishing_writes_it_where_a_server_will_look(self):
        with tempfile.TemporaryDirectory() as feed:
            lic = a_licence()
            out = square.publish_into(lic, feed, quiet=True)
            self.assertTrue(os.path.exists(out))
            self.assertEqual(
                os.path.basename(out),
                publish.license_filename_for(lic["id"]),
            )
            with open(out, encoding="utf-8") as f:
                self.assertEqual(json.load(f), lic)

    def test_a_renewal_replaces_the_one_that_was_there(self):
        # Selling three more seats publishes the same id at a higher count.
        # It has to land on the same name, or the server keeps fetching the
        # old one and the seats never arrive.
        with tempfile.TemporaryDirectory() as feed:
            first = square.publish_into(a_licence(users=6), feed, quiet=True)
            second = square.publish_into(a_licence(users=9), feed, quiet=True)
            self.assertEqual(first, second)
            with open(second, encoding="utf-8") as f:
                self.assertEqual(json.load(f)["users"], 9)
            self.assertEqual(len(os.listdir(feed)), 1, "one file, not two")

    def test_the_folder_is_made_if_it_is_not_there(self):
        with tempfile.TemporaryDirectory() as parent:
            feed = os.path.join(parent, "static", "f")
            square.publish_into(a_licence(), feed, quiet=True)
            self.assertTrue(os.path.isdir(feed))


class GettingItBackWithoutAskingAnybody(unittest.TestCase):
    """A customer who lost the file has lost the licence id with it.

    What they still have is their Square receipt, so the same licence is
    published a second time under the payment id. A page in a browser can
    work that address out; nothing has to be looked up, so there is nothing
    to look somebody else's licence up with.
    """

    def test_the_receipt_address_matches_what_a_browser_would_work_out(self):
        # The page does: SHA-256("receipt:" + payment id). Pinned here,
        # because if the two drift apart the page finds nothing and says the
        # licence does not exist, which is the worst possible wrong answer.
        import hashlib
        payment = "PAYMENT123"
        want = hashlib.sha256(b"receipt:PAYMENT123").hexdigest() + ".evlicense"
        self.assertEqual(square.a_receipt_name(payment), want)

    def test_a_stray_space_off_a_receipt_still_finds_it(self):
        self.assertEqual(
            square.a_receipt_name("  PAYMENT123  "),
            square.a_receipt_name("PAYMENT123"),
        )

    def test_it_is_not_the_same_address_as_the_licence_id(self):
        # Two different things published under two different names, so
        # knowing one never gives you the other.
        self.assertNotEqual(
            square.a_receipt_name("evl_abc123"),
            publish.license_filename_for("evl_abc123"),
        )

    def test_the_receipt_copy_is_the_same_licence(self):
        with tempfile.TemporaryDirectory() as feed:
            lic = a_licence()
            square.publish_into(lic, feed, quiet=True, receipt="PAYMENT123")
            names = sorted(os.listdir(feed))
            self.assertEqual(len(names), 2, "one by id, one by receipt")
            for name in names:
                with open(os.path.join(feed, name), encoding="utf-8") as f:
                    self.assertEqual(json.load(f), lic)

    def test_nothing_is_published_by_receipt_when_there_is_no_receipt(self):
        # A licence signed by hand has no Square order behind it.
        with tempfile.TemporaryDirectory() as feed:
            square.publish_into(a_licence(), feed, quiet=True)
            self.assertEqual(len(os.listdir(feed)), 1)


class PushingIt(unittest.TestCase):
    """A licence on the signing machine has not reached anybody.

    This is the step that was missing, and the failure is silent: everything
    above succeeds, the file is there, and the customer's server never sees
    it because nothing was ever pushed.
    """

    def test_it_refuses_a_folder_that_is_not_a_checkout(self):
        with tempfile.TemporaryDirectory() as notrepo:
            with self.assertRaises(SystemExit):
                square.push_the_feed(notrepo, "nothing", quiet=True)

    def test_it_says_so_rather_than_committing_nothing(self):
        with tempfile.TemporaryDirectory() as repo:
            git = lambda *a: subprocess.run(["git", *a], cwd=repo, check=True,
                                            capture_output=True)
            git("init", "-q")
            git("config", "user.email", "t@t")
            git("config", "user.name", "t")
            os.makedirs(os.path.join(repo, "static", "f"))
            open(os.path.join(repo, "static", "f", ".keep"), "w").close()
            git("add", "-A")
            git("commit", "-q", "-m", "first")
            # Nothing has changed since, so there is nothing to push.
            self.assertFalse(square.push_the_feed(repo, "nothing new", quiet=True))

    def test_a_new_licence_is_committed(self):
        with tempfile.TemporaryDirectory() as repo:
            git = lambda *a: subprocess.run(["git", *a], cwd=repo, check=True,
                                            capture_output=True)
            git("init", "-q")
            git("config", "user.email", "t@t")
            git("config", "user.name", "t")
            feed = os.path.join(repo, "static", "f")
            os.makedirs(feed)
            open(os.path.join(feed, ".keep"), "w").close()
            git("add", "-A")
            git("commit", "-q", "-m", "first")

            square.publish_into(a_licence(), feed, quiet=True)
            # No remote here, so the push itself fails -- which is the point:
            # it must be loud rather than leaving somebody believing a licence
            # went out when it did not.
            with self.assertRaises(SystemExit):
                square.push_the_feed(repo, "Publish 1 licence", quiet=True)
            # But it was committed before the push was attempted.
            log = subprocess.run(["git", "log", "--oneline", "-1"], cwd=repo,
                                 capture_output=True, text=True).stdout
            self.assertIn("Publish 1 licence", log)

    def test_a_push_that_failed_is_sent_next_time(self):
        # A licence committed while GitHub was unreachable used to sit on the
        # PC for good: the next run saw nothing new and pushed nothing.
        with tempfile.TemporaryDirectory() as root:
            remote = os.path.join(root, "remote.git")
            repo = os.path.join(root, "site")
            subprocess.run(["git", "init", "-q", "--bare", remote], check=True)
            subprocess.run(["git", "clone", "-q", remote, repo], check=True, capture_output=True)
            git = lambda *a: subprocess.run(["git", *a], cwd=repo, check=True,
                                            capture_output=True, text=True)
            git("config", "user.email", "t@t")
            git("config", "user.name", "t")
            feed = os.path.join(repo, "static", "f")
            os.makedirs(feed)
            open(os.path.join(feed, ".keep"), "w").close()
            git("add", "-A")
            git("commit", "-q", "-m", "first")
            git("push", "-q", "-u", "origin", "HEAD")

            square.publish_into(a_licence(), feed, quiet=True)
            gone = remote + ".away"
            os.rename(remote, gone)  # GitHub is unreachable
            with self.assertRaises(SystemExit):
                square.push_the_feed(repo, "Publish 1 licence", quiet=True)
            os.rename(gone, remote)  # and back

            # Nothing new this time, but the licence from last time still goes.
            self.assertTrue(square.push_the_feed(repo, "Publish 0 licences", quiet=True))
            there = subprocess.run(["git", "--git-dir", remote, "log", "--oneline", "-1"],
                                   capture_output=True, text=True).stdout
            self.assertIn("Publish 1 licence", there)

    def test_the_commit_message_never_names_a_customer(self):
        # A commit message is forever and the repository is shared. Whatever
        # else goes wrong, who bought what is not written into git history.
        with tempfile.TemporaryDirectory() as repo:
            git = lambda *a: subprocess.run(["git", *a], cwd=repo, check=True,
                                            capture_output=True)
            git("init", "-q")
            git("config", "user.email", "t@t")
            git("config", "user.name", "t")
            feed = os.path.join(repo, "static", "f")
            os.makedirs(feed)
            open(os.path.join(feed, ".keep"), "w").close()
            git("add", "-A")
            git("commit", "-q", "-m", "first")
            square.publish_into(a_licence(company="Example Steel, Inc."), feed, quiet=True)
            try:
                square.push_the_feed(repo, "Publish 1 licence", quiet=True)
            except SystemExit:
                pass
            log = subprocess.run(["git", "log", "--format=%s%n%b"], cwd=repo,
                                 capture_output=True, text=True).stdout
            self.assertNotIn("Example Steel", log)


class WhatASaleMeansForTheLicenceHeld(unittest.TestCase):
    """square.py's own signing, with a made-up key, as the office server will read it."""

    def a_folder(self, place, held):
        key = os.path.join(place, "licence-signing.key")  # WHAT: a made-up key file. WHY: the real one never leaves the signing PC
        with open(key, "w", encoding="utf-8") as f:
            f.write("licence-test\n" + "0a" * 32 + "\n")  # WHAT: a name and a seed nobody uses. WHY: the shape publish.load_key reads
        os.makedirs(os.path.join(place, "licenses"))
        publish.write_license(held, os.path.join(place, "licenses", "held.evlicense"))  # WHAT: the licence the company has. WHY: a sale lands on it
        return square.Folder(place)

    def test_more_seats_keep_the_id_and_the_company_as_the_held_licence_spells_it(self):
        with tempfile.TemporaryDirectory() as place:
            held = dict(a_licence(company="Acme Builders, LLC", users=6), issued="2026-09-24", note="", key="licence-test")
            folder = self.a_folder(place, held)
            lic, why = square.next_license(folder, {"kind": "seats", "users": 3}, "Acme Builders", folder.key)  # WHAT: the buyer typed the name without ", LLC". WHY: may be another company; a person decides
            self.assertIsNone(lic)
            self.assertIn("Acme Builders, LLC", why)  # WHAT: both names said. WHY: so the person can tell
            self.assertIn('--company "Acme Builders, LLC"', why)  # WHAT: the exact way to finish it. WHY: one command
            lic, why = square.next_license(folder, {"kind": "seats", "users": 3}, "Acme Builders, LLC", folder.key)  # WHAT: finished with the name as held. WHY: then it is issued
            self.assertIsNone(why)
            self.assertEqual((lic["id"], lic["company"], lic["users"]), ("evl_abc123", "Acme Builders, LLC", 9))  # WHAT: the same licence, three more seats. WHY: what the server takes by itself

    def test_the_licence_spelled_exactly_as_typed_wins_over_a_loose_match(self):
        with tempfile.TemporaryDirectory() as place:
            exact = dict(a_licence(id="evl_exact", company="Acme Builders", users=4), issued="2026-09-01", note="", key="licence-test")  # WHAT: spelled exactly as the order. WHY: the one meant
            folder = self.a_folder(place, exact)
            loose = dict(a_licence(id="evl_loose", company="Acme Builders, LLC", users=6), issued="2026-10-01", note="", key="licence-test")  # WHAT: another company's licence, newer, matching loosely. WHY: used to be picked first
            publish.write_license(loose, os.path.join(place, "licenses", "loose.evlicense"))
            lic, why = square.next_license(folder, {"kind": "seats", "users": 2}, "Acme Builders", folder.key)
            self.assertIsNone(why)
            self.assertEqual((lic["id"], lic["users"]), ("evl_exact", 6))  # WHAT: seats added to the exact one. WHY: never to a look-alike

    def test_several_loose_matches_are_all_named_and_nothing_is_issued(self):
        with tempfile.TemporaryDirectory() as place:
            one = dict(a_licence(id="evl_one", company="Acme Builders, LLC", users=4), issued="2026-09-01", note="", key="licence-test")
            folder = self.a_folder(place, one)
            two = dict(a_licence(id="evl_two", company="Acme Builders Inc.", users=6), issued="2026-10-01", note="", key="licence-test")  # WHAT: a second look-alike. WHY: which one is meant can't be guessed
            publish.write_license(two, os.path.join(place, "licenses", "two.evlicense"))
            lic, why = square.next_license(folder, {"kind": "seats", "users": 2}, "Acme Builders", folder.key)
            self.assertIsNone(lic)
            self.assertIn("Acme Builders, LLC", why)  # WHAT: both named. WHY: so a person can choose
            self.assertIn("Acme Builders Inc.", why)
            self.assertIn("--company", why)

    def test_a_name_that_is_all_noise_never_matches_a_licence(self):
        with tempfile.TemporaryDirectory() as place:
            held = dict(a_licence(company="LLC", users=6), issued="2026-09-24", note="", key="licence-test")  # WHAT: a licence whose name is all noise. WHY: "Inc." would match it
            folder = self.a_folder(place, held)
            lic, why = square.next_license(folder, {"kind": "seats", "users": 3}, "Inc.", folder.key)
            self.assertIsNone(lic)
            self.assertIn("--company", why)

    def test_a_date_the_server_would_not_read_is_never_signed(self):
        with tempfile.TemporaryDirectory() as place:
            key = os.path.join(place, "k.key")
            with open(key, "w", encoding="utf-8") as f:
                f.write("licence-test\n" + "0a" * 32 + "\n")
            for bad in ("20271001", "2027-W40-1", "2027-02-30", "1999-01-01", "\u0662\u0660\u0662\u0667-10-01"):  # WHAT: shapes Python reads as dates and the server doesn't, and one in Arabic-Indic digits. WHY: a signed file every server refuses (re.ASCII keeps \d to 0-9)
                with self.assertRaises(SystemExit):
                    publish.make_license(key, "Acme", updates_through=bad)
                with self.assertRaises(SystemExit):
                    publish.make_license(key, "Acme", issued=bad)
            self.assertEqual(publish.make_license(key, "Acme", updates_through="2027-10-01")["updates_through"], "2027-10-01")


if __name__ == "__main__":
    unittest.main(verbosity=2)
