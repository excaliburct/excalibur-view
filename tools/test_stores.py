#!/usr/bin/env python3
"""The store uploads, offline: what Google Play and App Store Connect are told.

Run it: python3 tools/test_stores.py

No store, no network and no real key. The keys are made fresh by openssl for
each run and thrown away with it. A recorder stands in for the network, so the
tests see every call the scripts would make, in order, with its body, and
answer the way the stores answer.
"""
import base64  # WHAT: decodes token parts. WHY: to read back what was signed.
import json  # WHAT: bodies. WHY: the stores speak JSON.
import os  # WHAT: paths and modes. WHY: key files.
import subprocess  # WHAT: runs openssl. WHY: makes test keys and checks signatures.
import sys  # WHAT: the module path. WHY: imports the scripts beside this.
import tempfile  # WHAT: a folder per test. WHY: nothing left behind.
import unittest  # WHAT: the test runner. WHY: as the repo's other tool tests.
import urllib.parse  # WHAT: reads form bodies and queries. WHY: the token request is a form.
import zipfile  # WHAT: makes fake bundles. WHY: an .aab is a zip.

HERE = os.path.dirname(os.path.abspath(__file__))  # WHAT: this folder. WHY: the scripts are beside it.
sys.path.insert(0, HERE)  # WHAT: importable. WHY: as above.

import appstore_release  # noqa: E402  WHAT: the App Store script. WHY: under test.
import play_release  # noqa: E402  WHAT: the Play script. WHY: under test.
import stores  # noqa: E402  WHAT: the shared pieces. WHY: under test.


def openssl(*args, data=None):  # WHAT: runs openssl, failing the test if it fails. WHY: makes and checks test keys.
    return subprocess.run(["openssl", *args], input=data, capture_output=True, check=True).stdout  # WHAT: its output. WHY: as above.


def unb64(part):  # WHAT: base64url back to bytes. WHY: reads token parts.
    return base64.urlsafe_b64decode(part + "=" * (-len(part) % 4))  # WHAT: padding restored. WHY: JWTs drop it.


def raw_to_der(raw):  # WHAT: r||s back to DER. WHY: openssl verifies DER ECDSA signatures.
    def integer(b):  # WHAT: one DER INTEGER. WHY: r and s.
        b = b.lstrip(b"\x00") or b"\x00"  # WHAT: minimal form. WHY: DER's rule.
        if b[0] & 0x80:  # WHAT: top bit set. WHY: would read as negative.
            b = b"\x00" + b  # WHAT: a zero in front. WHY: as above.
        return b"\x02" + bytes([len(b)]) + b  # WHAT: tag, length, value. WHY: DER.
    body = integer(raw[:32]) + integer(raw[32:])  # WHAT: both. WHY: the SEQUENCE's content.
    return b"\x30" + bytes([len(body)]) + body  # WHAT: the SEQUENCE. WHY: DER.


class Recorder:  # WHAT: stands in for stores.Http. WHY: every call is seen, none leaves the machine.
    def __init__(self, answers):  # WHAT: a list of (matcher, answer). WHY: answers in the stores' shape.
        self.answers, self.calls = list(answers), []  # WHAT: kept. WHY: as above.

    def send(self, method, url, headers=None, body=None, timeout=None):  # WHAT: the same signature as Http.send. WHY: a drop-in.
        self.calls.append((method, url, headers or {}, body))  # WHAT: recorded. WHY: the tests read it.
        for i, (want, answer) in enumerate(self.answers):  # WHAT: the first answer that matches. WHY: in order.
            if want(method, url):  # WHAT: matches. WHY: as above.
                self.answers.pop(i)  # WHAT: used once. WHY: a repeated call needs its own answer.
                if isinstance(answer, Exception):  # WHAT: a scripted failure. WHY: refusals are tested too.
                    raise answer  # WHAT: raised. WHY: as above.
                return 200, answer  # WHAT: the answer. WHY: as above.
        raise AssertionError(f"unexpected call {method} {url}")  # WHAT: a call nobody planned. WHY: the test fails loudly.


def on(method, fragment):  # WHAT: a matcher for method and part of the address. WHY: short tests.
    return lambda m, u: m == method and fragment in u  # WHAT: the check. WHY: as above.


class Keys(unittest.TestCase):  # WHAT: a fresh folder and keys per test. WHY: shared set-up.
    def setUp(self):  # WHAT: make them. WHY: as above.
        self.dir = tempfile.mkdtemp()  # WHAT: the folder. WHY: as above.
        self.rsa = os.path.join(self.dir, "rsa.pem")  # WHAT: an RSA key path. WHY: Google's kind.
        self.ec = os.path.join(self.dir, "ec.p8")  # WHAT: an EC key path. WHY: Apple's kind.
        openssl("genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", self.rsa)  # WHAT: a throwaway RSA key. WHY: never a real one.
        openssl("genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256", "-out", self.ec)  # WHAT: a throwaway P-256 key, PKCS#8 like a .p8. WHY: as above.
        os.chmod(self.rsa, 0o600)  # WHAT: private. WHY: the scripts refuse anything else.
        os.chmod(self.ec, 0o600)  # WHAT: private. WHY: as above.

    def tearDown(self):  # WHAT: clean up. WHY: nothing left behind.
        subprocess.run(["rm", "-rf", self.dir], check=False)  # WHAT: the folder. WHY: as above.

    def verify(self, token, key, alg):  # WHAT: checks a JWT's signature with openssl. WHY: proves the token is really signed by that key.
        head, claims, sig = token.split(".")  # WHAT: the three parts. WHY: as above.
        pub = os.path.join(self.dir, "pub.pem")  # WHAT: the public half. WHY: openssl verifies with it.
        openssl("pkey", "-in", key, "-pubout", "-out", pub)  # WHAT: made. WHY: as above.
        signature = unb64(sig)  # WHAT: the signature. WHY: as above.
        if alg == "ES256":  # WHAT: JOSE's r||s. WHY: openssl wants DER.
            self.assertEqual(len(signature), 64)  # WHAT: exactly 64 bytes. WHY: the JOSE form.
            signature = raw_to_der(signature)  # WHAT: back to DER. WHY: as above.
        sigfile = os.path.join(self.dir, "sig")  # WHAT: where openssl reads it. WHY: as above.
        with open(sigfile, "wb") as f:  # WHAT: write it. WHY: as above.
            f.write(signature)  # WHAT: the bytes. WHY: as above.
        out = subprocess.run(["openssl", "dgst", "-sha256", "-verify", pub, "-signature", sigfile],  # WHAT: verify. WHY: as above.
                             input=f"{head}.{claims}".encode(), capture_output=True, check=False)  # WHAT: the signed part. WHY: as above.
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)  # WHAT: verified. WHY: as above.
        return json.loads(unb64(head)), json.loads(unb64(claims))  # WHAT: header and claims. WHY: the tests read them.


class Signing(Keys):  # WHAT: the tokens. WHY: a wrong token is refused by the store only at release time.
    def test_rs256_verifies(self):  # WHAT: an RS256 token checks out. WHY: Google's.
        token = stores.make_jwt({"alg": "RS256", "typ": "JWT"}, {"a": 1}, self.rsa, "RS256")  # WHAT: made. WHY: as above.
        header, claims = self.verify(token, self.rsa, "RS256")  # WHAT: checked. WHY: as above.
        self.assertEqual(claims, {"a": 1})  # WHAT: the claims came through. WHY: as above.

    def test_es256_verifies_many_times(self):  # WHAT: ES256 tokens check out, including signatures whose r or s is short. WHY: DER drops leading zeros; JOSE must not.
        for n in range(40):  # WHAT: forty tokens. WHY: one in about 128 has a short r or s; forty gives the reshaping a fair workout.
            token = stores.make_jwt({"alg": "ES256", "typ": "JWT"}, {"n": n}, self.ec, "ES256")  # WHAT: made. WHY: as above.
            self.verify(token, self.ec, "ES256")  # WHAT: checked. WHY: as above.

    def test_der_with_short_and_padded_integers(self):  # WHAT: the reshaping on hand-made DER. WHY: covers the short cases directly.
        r, s = b"\x00" * 31 + b"\x05", b"\xff" * 32  # WHAT: r tiny, s with the top bit set. WHY: DER writes these one byte and 33 bytes long.
        self.assertEqual(stores.der_ecdsa_to_raw(raw_to_der(r + s)), r + s)  # WHAT: back to the same 64 bytes. WHY: as above.
        with self.assertRaises(stores.StoreError):  # WHAT: garbage. WHY: refused.
            stores.der_ecdsa_to_raw(b"\x31\x06\x02\x01\x01\x02\x01\x01")  # WHAT: not a SEQUENCE. WHY: as above.

    def test_open_key_file_refused(self):  # WHAT: a key others can read is refused. WHY: the guard in check_private_file.
        os.chmod(self.rsa, 0o644)  # WHAT: opened up. WHY: as above.
        with self.assertRaises(stores.StoreError):  # WHAT: refused. WHY: as above.
            stores.make_jwt({"alg": "RS256"}, {}, self.rsa, "RS256")  # WHAT: tried. WHY: as above.

    def test_non_https_refused(self):  # WHAT: the network door refuses plain HTTP. WHY: a token must never go out in the clear.
        with self.assertRaisesRegex(stores.StoreError, "non-HTTPS"):  # WHAT: refused by the HTTPS check itself. WHY: not by a failed connection.
            stores.Http().send("GET", "http://example.invalid/")  # WHAT: tried. WHY: as above; refused before any connection.


class Notes(unittest.TestCase):  # WHAT: the notes as the stores get them. WHY: the files are wrapped for a terminal.
    def test_unwraps_paragraphs(self):  # WHAT: wrapped lines joined, paragraphs kept. WHY: a store shows hard breaks.
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as f:  # WHAT: a notes file. WHY: read_notes reads a path.
            f.write("TITLE\n\nOne line\nand its wrap.\n\n\nSecond.\n")  # WHAT: a heading, a wrapped paragraph, extra blank lines. WHY: the shapes the real files have.
        try:  # WHAT: read it. WHY: as above.
            self.assertEqual(stores.read_notes(f.name), "TITLE\n\nOne line and its wrap.\n\nSecond.")  # WHAT: as a store wants. WHY: as above.
        finally:  # WHAT: clean up. WHY: as above.
            os.unlink(f.name)  # WHAT: removed. WHY: as above.

    def test_real_notes_fit_or_are_cut(self):  # WHAT: every notes file in the repo comes out within both limits. WHY: a release must never be refused for its notes.
        root = os.path.dirname(HERE)  # WHAT: the repo. WHY: the notes are at its root.
        names = [n for n in os.listdir(root) if n.startswith("notes-") and n.endswith(".txt")]  # WHAT: every notes file. WHY: as above.
        self.assertTrue(names)  # WHAT: there are some. WHY: otherwise this proves nothing.
        for name in names:  # WHAT: each. WHY: as above.
            text = stores.read_notes(os.path.join(root, name))  # WHAT: read. WHY: as above.
            self.assertLessEqual(len(stores.shorten(text, 500)), 500, name)  # WHAT: fits Play. WHY: as above.
            self.assertLessEqual(len(stores.shorten(text, 4000)), 4000, name)  # WHAT: fits Apple. WHY: as above.

    def test_shorten_cuts_at_a_paragraph_or_word(self):  # WHAT: the cut lands somewhere sensible. WHY: notes read by customers.
        text = ("word " * 30).strip() + "\n\n" + "more " * 200  # WHAT: a 149-character paragraph, then a long one. WHY: the paragraph cut applies.
        self.assertEqual(stores.shorten(text, 200), ("word " * 30).strip())  # WHAT: just the first paragraph. WHY: as above.
        cut = stores.shorten("x" * 10 + " " + "y" * 600, 100)  # WHAT: no paragraph to cut at. WHY: the word cut applies.
        self.assertEqual(cut, "x" * 10 + "…")  # WHAT: cut at the space, with an ellipsis. WHY: never half a word.


class PlayChecks(Keys):  # WHAT: what play_release refuses before calling Google. WHY: each guard proven.
    def account(self, **change):  # WHAT: writes a service account file. WHY: shaped like Google's.
        with open(self.rsa, encoding="utf-8") as f:  # WHAT: the test key. WHY: the account's private_key.
            data = {"type": play_release.ACCOUNT_TYPE, "client_email": "release@example.iam.gserviceaccount.com",  # WHAT: the usual fields. WHY: as Google's files.
                    "private_key": f.read(), "private_key_id": "abc123", "token_uri": play_release.TOKEN_URI}  # WHAT: and the key. WHY: as above.
        data.update(change)  # WHAT: a field changed for the test. WHY: as above.
        path = os.path.join(self.dir, "account.json")  # WHAT: its path. WHY: as above.
        with open(path, "w", encoding="utf-8") as f:  # WHAT: written. WHY: as above.
            json.dump(data, f)  # WHAT: the JSON. WHY: as above.
        os.chmod(path, 0o600)  # WHAT: private. WHY: as the workflow writes it.
        return path  # WHAT: the path. WHY: as above.

    def bundle(self, version="0.6.30", signed=True):  # WHAT: a fake .aab. WHY: the shape check reads only names.
        path = os.path.join(self.dir, f"excalibur-view-{version}.aab")  # WHAT: named as the build names it. WHY: the name is checked.
        with zipfile.ZipFile(path, "w") as z:  # WHAT: the zip. WHY: as above.
            z.writestr("BundleConfig.pb", b"x")  # WHAT: a bundle's config. WHY: as above.
            z.writestr("base/manifest/AndroidManifest.xml", b"x")  # WHAT: its manifest. WHY: as above.
            if signed:  # WHAT: with a signature. WHY: the upload key's.
                z.writestr("META-INF/UPLOAD.SF", b"x")  # WHAT: the signature file. WHY: as above.
                z.writestr("META-INF/UPLOAD.RSA", b"x")  # WHAT: the signature block. WHY: as above.
        return path  # WHAT: the path. WHY: as above.

    def test_good_account_and_bundle_pass(self):  # WHAT: the happy path is not refused. WHY: guards that refuse everything are no use.
        play_release.load_account(self.account())  # WHAT: accepted. WHY: as above.
        play_release.check_bundle(self.bundle(), "0.6.30")  # WHAT: accepted. WHY: as above.

    def test_foreign_token_address_refused(self):  # WHAT: an account naming another token address. WHY: our signed assertion must only go to Google.
        with self.assertRaises(stores.StoreError):  # WHAT: refused. WHY: as above.
            play_release.load_account(self.account(token_uri="https://evil.example/token"))  # WHAT: tried. WHY: as above.

    def test_wrong_kind_of_file_refused(self):  # WHAT: not a service account. WHY: an OAuth client file looks similar.
        with self.assertRaises(stores.StoreError):  # WHAT: refused. WHY: as above.
            play_release.load_account(self.account(type="authorized_user"))  # WHAT: tried. WHY: as above.

    def test_unsigned_bundle_refused(self):  # WHAT: a bundle without the upload key's signature. WHY: Play would refuse it after the upload.
        with self.assertRaises(stores.StoreError):  # WHAT: refused. WHY: as above.
            play_release.check_bundle(self.bundle(signed=False), "0.6.30")  # WHAT: tried. WHY: as above.

    def test_other_version_refused(self):  # WHAT: a bundle named for another version. WHY: never ship the wrong one.
        with self.assertRaises(stores.StoreError):  # WHAT: refused. WHY: as above.
            play_release.check_bundle(self.bundle("0.6.29"), "0.6.30")  # WHAT: tried. WHY: as above.

    def test_bad_settings_refused(self):  # WHAT: odd track, status, package or version. WHY: each goes into an address or body.
        for args in (("com.excaliburct.view", "nightly", "completed", "0.6.30"),  # WHAT: no such track. WHY: as above.
                     ("com.excaliburct.view", "production", "inProgress", "0.6.30"),  # WHAT: a status we do not make. WHY: needs a fraction.
                     ("com.excaliburct.view/../x", "production", "completed", "0.6.30"),  # WHAT: a path in the package. WHY: as above.
                     ("com.excaliburct.view", "production", "completed", "0.6.30; rm"),  # WHAT: junk in the version. WHY: as above.
                     ("com.excaliburct.view", "production", "completed", "0.6.30\n")):  # WHAT: a trailing newline. WHY: a pattern ending in $ alone would let it through.
            with self.assertRaises(stores.StoreError, msg=str(args)):  # WHAT: refused. WHY: as above.
                play_release.plan(*args, notes="")  # WHAT: tried. WHY: as above.

    def test_dry_run_calls_nothing(self):  # WHAT: --dry-run checks all and calls nothing. WHY: the promise of a dry run.
        original = stores.Http.send  # WHAT: the real door. WHY: replaced, then put back.
        stores.Http.send = lambda *a, **k: self.fail("a dry run made a network call")  # WHAT: any call fails the test. WHY: as above.
        try:  # WHAT: run it. WHY: as above.
            code = play_release.main(["--account-file", self.account(), "--aab", self.bundle(), "--version", "0.6.30", "--dry-run"])  # WHAT: a dry run. WHY: as above.
        finally:  # WHAT: put back. WHY: other tests.
            stores.Http.send = original  # WHAT: restored. WHY: as above.
        self.assertEqual(code, 0)  # WHAT: success. WHY: everything checked.


class PlayCalls(Keys):  # WHAT: the calls a real Play release makes. WHY: the request building, proven offline.
    def test_the_whole_release(self):  # WHAT: token, edit, upload, symbols, track, commit, in that order. WHY: Google's flow.
        http = Recorder([
            (on("POST", "oauth2.googleapis.com/token"), {"access_token": "tok-123"}),  # WHAT: a token. WHY: Google's answer.
            (on("POST", "/applications/com.excaliburct.view/edits"), {"id": "edit42"}),  # WHAT: an edit. WHY: as above.
            (on("POST", "/upload/androidpublisher/v3/applications/com.excaliburct.view/edits/edit42/bundles"), {"versionCode": 630}),  # WHAT: the upload. WHY: as above.
            (on("POST", "/edits/edit42/apks/630/deobfuscationFiles/nativeCode"), {}),  # WHAT: the symbols. WHY: as above.
            (on("PUT", "/edits/edit42/tracks/production"), {}),  # WHAT: the track. WHY: as above.
            (on("POST", "/edits/edit42:commit"), {}),  # WHAT: the commit. WHY: as above.
        ])
        aab = os.path.join(self.dir, "b.aab")  # WHAT: a stand-in file. WHY: only its bytes are sent.
        with open(aab, "wb") as f:  # WHAT: written. WHY: as above.
            f.write(b"bundle-bytes")  # WHAT: the body. WHY: checked below.
        play = play_release.Play(http, "com.excaliburct.view")  # WHAT: the client. WHY: as above.
        play.sign_in("signed.assertion.here")  # WHAT: the token. WHY: as above.
        body = play_release.plan("com.excaliburct.view", "production", "completed", "0.6.30", "What changed.")  # WHAT: the track body. WHY: as above.
        self.assertEqual(play_release.release(play, body, aab, aab), 630)  # WHAT: the whole run. WHY: as above.
        self.assertFalse(http.answers)  # WHAT: every planned call was made. WHY: none skipped.
        form = urllib.parse.parse_qs(http.calls[0][3].decode())  # WHAT: the token request's form. WHY: checked.
        self.assertEqual(form["grant_type"], ["urn:ietf:params:oauth:grant-type:jwt-bearer"])  # WHAT: the JWT bearer grant. WHY: Google's.
        self.assertEqual(form["assertion"], ["signed.assertion.here"])  # WHAT: our assertion. WHY: as above.
        for method, url, headers, _ in http.calls[1:]:  # WHAT: every API call. WHY: each carries the token.
            self.assertEqual(headers["Authorization"], "Bearer tok-123")  # WHAT: the bearer. WHY: as above.
        self.assertEqual(http.calls[2][3], b"bundle-bytes")  # WHAT: the bundle went as the body. WHY: a media upload.
        self.assertIn("uploadType=media", http.calls[2][1])  # WHAT: as a media upload. WHY: Google's upload protocol.
        track = json.loads(http.calls[4][3])  # WHAT: the track body. WHY: checked.
        self.assertEqual(track, {"track": "production", "releases": [{"name": "0.6.30", "versionCodes": ["630"], "status": "completed",  # WHAT: exactly this. WHY: the version code Play gave, as text.
                                                                      "releaseNotes": [{"language": "en-US", "text": "What changed."}]}]})  # WHAT: and the notes. WHY: as above.

    def test_symbols_refused_does_not_stop_it(self):  # WHAT: a refused symbols upload is a warning. WHY: crash symbols are not worth missing a release over.
        http = Recorder([
            (on("POST", "/edits"), {"id": "e1"}),  # WHAT: an edit. WHY: as above.
            (on("POST", "/bundles"), {"versionCode": 7}),  # WHAT: the upload. WHY: as above.
            (on("POST", "deobfuscationFiles"), stores.StoreError("refused")),  # WHAT: symbols refused. WHY: the case under test.
            (on("PUT", "/tracks/internal"), {}),  # WHAT: the track. WHY: as above.
            (on("POST", ":commit"), {}),  # WHAT: the commit. WHY: still happens.
        ])
        play = play_release.Play(http, "com.excaliburct.view")  # WHAT: the client. WHY: as above.
        play.token = "t"  # WHAT: a token. WHY: sign_in is tested above.
        aab = os.path.join(self.dir, "b.aab")  # WHAT: a stand-in. WHY: as above.
        open(aab, "wb").close()  # WHAT: empty. WHY: only sent.
        body = play_release.plan("com.excaliburct.view", "internal", "draft", "0.6.30", "")  # WHAT: a draft on internal. WHY: the other choices.
        self.assertEqual(play_release.release(play, body, aab, aab), 7)  # WHAT: completed. WHY: as above.
        self.assertEqual(json.loads(http.calls[3][3])["releases"][0]["status"], "draft")  # WHAT: a draft. WHY: Creede presses release.
        self.assertNotIn("releaseNotes", json.loads(http.calls[3][3])["releases"][0])  # WHAT: no notes, no field. WHY: Play keeps the old ones out.

    def test_bad_edit_id_refused(self):  # WHAT: an edit id that is not plain. WHY: it goes into addresses.
        play = play_release.Play(Recorder([(on("POST", "/edits"), {"id": "../x"})]), "com.excaliburct.view")  # WHAT: a hostile answer. WHY: as above.
        play.token = "t"  # WHAT: a token. WHY: as above.
        with self.assertRaises(stores.StoreError):  # WHAT: refused. WHY: as above.
            play.open_edit()  # WHAT: tried. WHY: as above.


APP = {"data": [{"id": "1234567890", "attributes": {"bundleId": "com.excaliburct.view"}}]}  # WHAT: Apple's answer for our app. WHY: used by every App Store test.
VALID = {"data": [{"id": "build-1", "attributes": {"processingState": "VALID", "usesNonExemptEncryption": False}}]}  # WHAT: a processed build. WHY: as above.


class AppStoreCalls(Keys):  # WHAT: the calls a real App Store submission makes. WHY: the request building, proven offline.
    def store(self, answers, clock=None):  # WHAT: a client with a recorder. WHY: shared.
        self.http = Recorder(answers)  # WHAT: the recorder. WHY: read by the tests.
        self.slept = []  # WHAT: waits asked for. WHY: the processing wait is checked.
        ticks = iter(range(0, 10 ** 6, 30))  # WHAT: a clock moving 30 s per look. WHY: deterministic.
        return appstore_release.AppStore(self.http, self.ec, "ABCDE12345", "11111111-2222-3333-4444-555555555555",  # WHAT: the client. WHY: test key, made-up ids.
                                         clock=clock or (lambda: next(ticks)), sleep=self.slept.append)  # WHAT: fake time. WHY: no real waiting.

    def test_new_version_whole_flow(self):  # WHAT: wait, make the version, attach, notes, submit. WHY: the usual release.
        s = self.store([
            (on("GET", "/v1/apps?"), APP),  # WHAT: the app. WHY: as above.
            (on("GET", "/v1/builds?"), {"data": []}),  # WHAT: not there yet. WHY: the wait.
            (on("GET", "/v1/builds?"), {"data": [{"id": "build-1", "attributes": {"processingState": "PROCESSING"}}]}),  # WHAT: processing. WHY: the wait.
            (on("GET", "/v1/builds?"), VALID),  # WHAT: ready. WHY: as above.
            (on("GET", "/v1/apps/1234567890/appStoreVersions"), {"data": [{"id": "old", "attributes": {"versionString": "0.6.29", "appVersionState": "READY_FOR_DISTRIBUTION"}}]}),  # WHAT: only a released older one. WHY: so a new one is made.
            (on("POST", "/v1/appStoreVersions"), {"data": {"id": "ver-1"}}),  # WHAT: made. WHY: as above.
            (on("PATCH", "/v1/appStoreVersions/ver-1/relationships/build"), {}),  # WHAT: attached. WHY: as above.
            (on("GET", "/v1/appStoreVersions/ver-1/appStoreVersionLocalizations"), {"data": [{"id": "loc-en"}]}),  # WHAT: one language. WHY: as above.
            (on("PATCH", "/v1/appStoreVersionLocalizations/loc-en"), {}),  # WHAT: notes set. WHY: as above.
            (on("GET", "/v1/reviewSubmissions?"), {"data": []}),  # WHAT: none open. WHY: so one is made.
            (on("POST", "/v1/reviewSubmissions"), {"data": {"id": "sub-1"}}),  # WHAT: made. WHY: as above.
            (on("POST", "/v1/reviewSubmissionItems"), {}),  # WHAT: the item. WHY: as above.
            (on("PATCH", "/v1/reviewSubmissions/sub-1"), {}),  # WHAT: submitted. WHY: as above.
        ])
        self.assertEqual(appstore_release.release(s, "0.6.30", "20261110.1530", "What changed.", "MANUAL", 90), "sub-1")  # WHAT: the whole run. WHY: as above.
        self.assertFalse(self.http.answers)  # WHAT: every planned call made. WHY: none skipped.
        self.assertEqual(self.slept, [60, 60])  # WHAT: waited twice, a minute each. WHY: two looks before VALID.
        q = urllib.parse.parse_qs(urllib.parse.urlsplit(self.http.calls[1][1]).query)  # WHAT: the build filter. WHY: checked.
        self.assertEqual((q["filter[app]"], q["filter[version]"], q["filter[preReleaseVersion.version]"]), (["1234567890"], ["20261110.1530"], ["0.6.30"]))  # WHAT: exactly our build. WHY: as above.
        made = json.loads(self.http.calls[5][3])["data"]  # WHAT: the new version's body. WHY: checked.
        self.assertEqual(made["attributes"], {"platform": "IOS", "versionString": "0.6.30", "releaseType": "MANUAL"})  # WHAT: iPad, this number, manual release. WHY: release stays a person's button.
        self.assertEqual(json.loads(self.http.calls[6][3]), {"data": {"type": "builds", "id": "build-1"}})  # WHAT: that build. WHY: as above.
        self.assertEqual(json.loads(self.http.calls[8][3])["data"]["attributes"], {"whatsNew": "What changed."})  # WHAT: the notes. WHY: as above.
        item = json.loads(self.http.calls[11][3])["data"]["relationships"]  # WHAT: the submission item. WHY: checked.
        self.assertEqual((item["reviewSubmission"]["data"]["id"], item["appStoreVersion"]["data"]["id"]), ("sub-1", "ver-1"))  # WHAT: our version in our submission. WHY: as above.
        self.assertEqual(json.loads(self.http.calls[12][3])["data"]["attributes"], {"submitted": True})  # WHAT: sent. WHY: as above.
        for _, _, headers, _ in self.http.calls:  # WHAT: every call. WHY: each carries a token.
            self.assertTrue(headers["Authorization"].startswith("Bearer "))  # WHAT: a bearer. WHY: as above.
        _, claims = self.verify(self.http.calls[0][2]["Authorization"][7:], self.ec, "ES256")  # WHAT: the token verifies with the key. WHY: Apple would refuse anything else.
        self.assertEqual((claims["aud"], claims["exp"] - claims["iat"]), ("appstoreconnect-v1", 1200))  # WHAT: Apple's audience, twenty minutes. WHY: Apple's rules.

    def test_editable_older_version_is_renamed(self):  # WHAT: an unsubmitted 0.6.29 becomes 0.6.30. WHY: Apple allows only one version being prepared.
        s = self.store([
            (on("GET", "/v1/apps/1234567890/appStoreVersions"), {"data": [{"id": "ver-9", "attributes": {"versionString": "0.6.29", "appStoreState": "PREPARE_FOR_SUBMISSION"}}]}),  # WHAT: an old editable one. WHY: the case.
            (on("PATCH", "/v1/appStoreVersions/ver-9"), {}),  # WHAT: renamed. WHY: as above.
        ])
        self.assertEqual(s.version_for("1234567890", "0.6.30", "AFTER_APPROVAL"), ("ver-9", True))  # WHAT: that one, to be set up. WHY: as above.
        self.assertEqual(json.loads(self.http.calls[1][3])["data"]["attributes"], {"versionString": "0.6.30", "releaseType": "AFTER_APPROVAL"})  # WHAT: renamed, automatic release. WHY: the input asked for it.

    def test_already_in_review_refused(self):  # WHAT: a version already with Apple is left alone. WHY: changing it would pull it from review.
        s = self.store([(on("GET", "appStoreVersions"), {"data": [{"id": "v", "attributes": {"versionString": "0.6.30", "appVersionState": "WAITING_FOR_REVIEW"}}]})])  # WHAT: in review. WHY: the case.
        with self.assertRaisesRegex(stores.StoreError, "already WAITING_FOR_REVIEW"):  # WHAT: refused, saying it is already with Apple. WHY: the plain reason, not the generic one.  # WHAT: refused. WHY: as above.
            s.version_for("1234567890", "0.6.30", "MANUAL")  # WHAT: tried. WHY: as above.

    def test_released_version_refused(self):  # WHAT: a version already on sale. WHY: it cannot change; the version must rise.
        s = self.store([(on("GET", "appStoreVersions"), {"data": [{"id": "v", "attributes": {"versionString": "0.6.30", "appVersionState": "READY_FOR_DISTRIBUTION"}}]})])  # WHAT: released. WHY: the case.
        with self.assertRaises(stores.StoreError):  # WHAT: refused. WHY: as above.
            s.version_for("1234567890", "0.6.30", "MANUAL")  # WHAT: tried. WHY: as above.

    def test_open_submission_reused_and_item_not_doubled(self):  # WHAT: an unsent submission already holding our version is just sent. WHY: a run that stopped halfway can be run again.
        s = self.store([
            (on("GET", "/v1/reviewSubmissions?"), {"data": [{"id": "sub-7"}]}),  # WHAT: one open. WHY: the case.
            (on("GET", "/v1/reviewSubmissions/sub-7/items"), {"data": [{"relationships": {"appStoreVersion": {"data": {"id": "ver-1"}}}}]}),  # WHAT: ours in it. WHY: as above.
            (on("PATCH", "/v1/reviewSubmissions/sub-7"), {}),  # WHAT: sent. WHY: as above.
        ])
        self.assertEqual(s.submit("1234567890", "ver-1"), "sub-7")  # WHAT: reused. WHY: as above.
        self.assertFalse(self.http.answers)  # WHAT: no item added. WHY: it was already in.

    def test_failed_build_and_timeout(self):  # WHAT: a build Apple turned down, and one that never finishes. WHY: neither may hang the runner.
        s = self.store([(on("GET", "/v1/builds?"), {"data": [{"id": "b", "attributes": {"processingState": "INVALID"}}]})])  # WHAT: invalid. WHY: the case.
        with self.assertRaises(stores.StoreError):  # WHAT: stops. WHY: as above.
            s.wait_for_build("1234567890", "0.6.30", "1", 90)  # WHAT: tried. WHY: as above.
        s = self.store([(on("GET", "/v1/builds?"), {"data": []})] * 100)  # WHAT: never appears. WHY: the case.
        with self.assertRaises(stores.StoreError):  # WHAT: gives up. WHY: as above.
            s.wait_for_build("1234567890", "0.6.30", "1", 2)  # WHAT: two minutes allowed. WHY: the fake clock passes it in a few looks.

    def test_settings_refused(self):  # WHAT: odd settings refused. WHY: each goes into a filter or address.
        good = ("0.6.30", "20261110.1530", "ABCDE12345", "11111111-2222-3333-4444-555555555555", "manual")  # WHAT: good ones. WHY: changed one at a time.
        appstore_release.check_settings(*good)  # WHAT: accepted. WHY: guards that refuse everything are no use.
        for i, bad in enumerate(("0.6", "12&filter[x]=1", "abc", "not-a-uuid", "automatic")):  # WHAT: one bad value per position. WHY: each guard proven.
            args = list(good)  # WHAT: a copy. WHY: as above.
            args[i] = bad  # WHAT: one bad. WHY: as above.
            with self.assertRaises(stores.StoreError, msg=bad):  # WHAT: refused. WHY: as above.
                appstore_release.check_settings(*args)  # WHAT: tried. WHY: as above.

    def test_dry_run_calls_nothing(self):  # WHAT: --dry-run checks all and calls nothing. WHY: the promise of a dry run.
        notes = os.path.join(self.dir, "notes.txt")  # WHAT: a notes file. WHY: required.
        with open(notes, "w", encoding="utf-8") as f:  # WHAT: written. WHY: as above.
            f.write("Something new.\n")  # WHAT: text. WHY: as above.
        original = stores.Http.send  # WHAT: the real door. WHY: replaced, then put back.
        stores.Http.send = lambda *a, **k: self.fail("a dry run made a network call")  # WHAT: any call fails the test. WHY: as above.
        try:  # WHAT: run it. WHY: as above.
            code = appstore_release.main(["--key-file", self.ec, "--key-id", "ABCDE12345", "--issuer", "11111111-2222-3333-4444-555555555555",  # WHAT: a dry run. WHY: as above.
                                          "--version", "0.6.30", "--build-number", "1", "--notes-file", notes, "--dry-run"])  # WHAT: the rest. WHY: as above.
        finally:  # WHAT: put back. WHY: other tests.
            stores.Http.send = original  # WHAT: restored. WHY: as above.
        self.assertEqual(code, 0)  # WHAT: success. WHY: everything checked.

    def test_wrong_key_refused_in_dry_run(self):  # WHAT: a file that is not a key fails the dry run. WHY: a broken secret is found before a release.
        bad = os.path.join(self.dir, "bad.p8")  # WHAT: a non-key. WHY: the case.
        with open(bad, "w", encoding="utf-8") as f:  # WHAT: written. WHY: as above.
            f.write("not a key\n")  # WHAT: junk. WHY: as above.
        os.chmod(bad, 0o600)  # WHAT: private. WHY: so the key check, not the mode check, refuses it.
        notes = os.path.join(self.dir, "n.txt")  # WHAT: notes. WHY: required.
        with open(notes, "w", encoding="utf-8") as f:  # WHAT: written. WHY: as above.
            f.write("x\n")  # WHAT: text. WHY: as above.
        self.assertEqual(appstore_release.main(["--key-file", bad, "--key-id", "ABCDE12345", "--issuer", "11111111-2222-3333-4444-555555555555",  # WHAT: refused. WHY: as above.
                                                "--version", "0.6.30", "--build-number", "1", "--notes-file", notes, "--dry-run"]), 1)  # WHAT: exit 1. WHY: as above.


if __name__ == "__main__":  # WHAT: run as a script. WHY: as the repo's other tool tests.
    unittest.main()  # WHAT: run them. WHY: as above.
