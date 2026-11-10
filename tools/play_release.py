#!/usr/bin/env python3
"""Sends the Android App Bundle to Google Play and puts it on a track.

    python3 tools/play_release.py --account-file FILE --aab FILE --version 0.6.30 \
        [--symbols FILE] [--notes-file FILE] [--track production] [--status completed] [--dry-run]

Run by the Release workflow, never by hand on a desk: the service account file
is written there from the PLAY_SERVICE_ACCOUNT_JSON secret, 0600, and removed
when the step ends.

It uses Google's Play Developer Publishing API, the same steps the Play
Console does by hand: open an edit, upload the bundle (and the native debug
symbols beside it), put that version on the track, and commit the edit. Until
the commit nothing anybody can see has changed; a failure on the way leaves an
edit Google throws away by itself.

--dry-run checks everything this side of the network (the account file's
shape, the bundle is a signed App Bundle, the notes fit, the token can be
signed) and lists the calls it would make, without making any.
"""
import argparse  # WHAT: the command line. WHY: the workflow passes everything as options.
import json  # WHAT: reads the service account file and writes request bodies. WHY: both are JSON.
import os  # WHAT: file sizes and paths. WHY: checks on the bundle.
import re  # WHAT: patterns for accepted values. WHY: refuse anything odd before a store sees it.
import shutil  # WHAT: removes the private folder. WHY: the key copy is gone the moment we are done.
import sys  # WHAT: the exit code. WHY: the job goes red on failure.
import tempfile  # WHAT: a private folder for the key. WHY: openssl reads the key from a file.
import time  # WHAT: the clock. WHY: the token's issue and expiry times.
import urllib.parse  # WHAT: form encoding and URL quoting. WHY: the token request is a form; names go into addresses.
import zipfile  # WHAT: reads the bundle. WHY: an .aab is a zip; its contents prove what it is.

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))  # WHAT: finds stores.py beside this. WHY: run as a plain script.
import stores  # noqa: E402  WHAT: the shared signing and HTTP pieces. WHY: one copy for both stores.

TOKEN_URI = "https://oauth2.googleapis.com/token"  # WHAT: Google's token address. WHY: the only one accepted; a file naming another would send our signed assertion elsewhere.
SCOPE = "https://www.googleapis.com/auth/androidpublisher"  # WHAT: the Play publishing permission. WHY: the token can do nothing else.
API = "https://androidpublisher.googleapis.com/androidpublisher/v3/applications"  # WHAT: the API's base. WHY: every call starts here.
UPLOAD = "https://androidpublisher.googleapis.com/upload/androidpublisher/v3/applications"  # WHAT: the upload base. WHY: files go to the upload address.
ACCOUNT_TYPE = "service_account"  # WHAT: the "type" of a Google service account key file. WHY: other Google JSON files look similar and are refused.
TRACKS = ("production", "beta", "alpha", "internal")  # WHAT: Play's standard tracks. WHY: anything else is refused.
STATUSES = ("completed", "draft")  # WHAT: out to everybody, or a draft to press release on. WHY: the two this release makes.
NOTES_LIMIT = 500  # WHAT: Play's limit on release notes per language. WHY: longer notes are refused by Play.
PACKAGE_RE = re.compile(r"^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$")  # WHAT: an Android package name. WHY: it goes into URLs.
VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")  # WHAT: MAJOR.MINOR.PATCH. WHY: the release's name on Play.


def load_account(path):  # WHAT: reads and checks the service account file. WHY: a wrong file should fail here, not halfway through.
    stores.check_private_file(path)  # WHAT: it is 0600. WHY: it holds a private key.
    try:  # WHAT: parse it. WHY: a pasted secret can be broken.
        with open(path, encoding="utf-8") as f:  # WHAT: the file. WHY: as above.
            account = json.load(f)  # WHAT: the JSON. WHY: Google's key file format.
    except (OSError, ValueError):  # WHAT: unreadable or not JSON. WHY: say so without quoting it.
        raise stores.StoreError("the Play service account secret is not a JSON key file") from None  # WHAT: stop. WHY: never echo the secret.
    if not isinstance(account, dict) or account.get("type") != ACCOUNT_TYPE:  # WHAT: must be a service account key. WHY: other Google JSON files look similar.
        raise stores.StoreError("the Play secret is not a service account key (type is not service_account)")  # WHAT: stop. WHY: as above.
    for field in ("client_email", "private_key"):  # WHAT: the two fields used. WHY: both are needed to sign.
        if not isinstance(account.get(field), str) or not account[field]:  # WHAT: present and text. WHY: as above.
            raise stores.StoreError(f"the Play service account key has no {field}")  # WHAT: stop, naming only the field. WHY: never the value.
    if "-----BEGIN PRIVATE KEY-----" not in account["private_key"]:  # WHAT: a PKCS#8 key. WHY: what Google issues; anything else is a wrong paste.
        raise stores.StoreError("the Play service account's private_key is not a PEM private key")  # WHAT: stop. WHY: as above.
    if account.get("token_uri", TOKEN_URI) != TOKEN_URI:  # WHAT: the token address is Google's. WHY: see TOKEN_URI.
        raise stores.StoreError("the Play service account names a token address that is not Google's")  # WHAT: stop. WHY: as above.
    return account  # WHAT: the checked account. WHY: used to sign.


def check_bundle(path, version):  # WHAT: the bundle is a signed Android App Bundle of this version. WHY: Play refuses an unsigned one after the upload; better before.
    if not os.path.isfile(path) or os.path.getsize(path) == 0:  # WHAT: a real, non-empty file. WHY: the build may have made nothing.
        raise stores.StoreError(f"no bundle at {path}")  # WHAT: stop. WHY: nothing to send.
    if not os.path.basename(path) == f"excalibur-view-{version}.aab":  # WHAT: named for this version. WHY: the build names it so; another name means another version.
        raise stores.StoreError(f"{os.path.basename(path)} is not the bundle for {version}")  # WHAT: stop. WHY: never send the wrong version.
    try:  # WHAT: open it as a zip. WHY: an .aab is one.
        with zipfile.ZipFile(path) as z:  # WHAT: the bundle. WHY: to list it.
            names = set(z.namelist())  # WHAT: every entry. WHY: checked below.
    except zipfile.BadZipFile:  # WHAT: not a zip. WHY: a broken download.
        raise stores.StoreError(f"{path} is not an App Bundle") from None  # WHAT: stop. WHY: as above.
    for needed in ("BundleConfig.pb", "base/manifest/AndroidManifest.xml"):  # WHAT: what every bundle holds. WHY: proves it is a bundle and not an APK.
        if needed not in names:  # WHAT: missing. WHY: not a bundle.
            raise stores.StoreError(f"{path} is not an App Bundle (no {needed})")  # WHAT: stop. WHY: as above.
    signed = any(re.fullmatch(r"META-INF/[^/]+\.(RSA|EC|DSA)", n) for n in names)  # WHAT: a signature block. WHY: jarsigner leaves one; Play needs the upload key's.
    if not signed or not any(re.fullmatch(r"META-INF/[^/]+\.SF", n) for n in names):  # WHAT: and its signature file. WHY: the pair is the signature.
        raise stores.StoreError("the bundle is not signed: the upload key secrets are missing or wrong")  # WHAT: stop. WHY: the build makes an unsigned bundle without the key.


def plan(package, track, status, version, notes):  # WHAT: checks the release settings and returns the track body. WHY: one place builds what Play is told.
    if not PACKAGE_RE.fullmatch(package):  # WHAT: a package name. WHY: it goes into URLs.
        raise stores.StoreError(f"{package!r} is not an Android package name")  # WHAT: stop. WHY: as above.
    if track not in TRACKS:  # WHAT: a known track. WHY: as above.
        raise stores.StoreError(f"track must be one of {', '.join(TRACKS)}")  # WHAT: stop. WHY: as above.
    if status not in STATUSES:  # WHAT: a known status. WHY: as above.
        raise stores.StoreError(f"status must be one of {', '.join(STATUSES)}")  # WHAT: stop. WHY: as above.
    if not VERSION_RE.fullmatch(version):  # WHAT: MAJOR.MINOR.PATCH. WHY: as above.
        raise stores.StoreError(f"{version!r} is not a version")  # WHAT: stop. WHY: as above.
    release = {"name": version, "versionCodes": [], "status": status}  # WHAT: the release, its version code filled in after the upload. WHY: Play gives the code back.
    if notes:  # WHAT: notes when there are some. WHY: "What's new" on Play.
        release["releaseNotes"] = [{"language": "en-US", "text": stores.shorten(notes, NOTES_LIMIT)}]  # WHAT: English notes, cut to fit. WHY: Play's limit.
    return {"track": track, "releases": [release]}  # WHAT: the track body. WHY: edits.tracks.update takes this.


def assertion(account, now):  # WHAT: the signed request for a token. WHY: Google's service account flow (RFC 7523).
    header = {"alg": "RS256", "typ": "JWT"}  # WHAT: RSA with SHA-256. WHY: what Google's keys are.
    if isinstance(account.get("private_key_id"), str):  # WHAT: the key's id when present. WHY: lets Google pick the right public key.
        header["kid"] = account["private_key_id"]  # WHAT: added. WHY: as above.
    claims = {"iss": account["client_email"], "scope": SCOPE, "aud": TOKEN_URI,  # WHAT: who asks, for what, from whom. WHY: Google's required claims.
              "iat": int(now), "exp": int(now) + 3600}  # WHAT: valid for an hour. WHY: Google's maximum.
    return header, claims  # WHAT: both. WHY: signed by the caller.


class Play:  # WHAT: the calls to Play, through an Http that tests replace. WHY: the request building is tested offline.
    def __init__(self, http, package):  # WHAT: a client for one app. WHY: every address names the package.
        self.http, self.package, self.token = http, package, None  # WHAT: the door, the app, no token yet. WHY: the token comes from sign_in.

    def sign_in(self, signed_assertion):  # WHAT: trades the signed assertion for an access token. WHY: every call needs one.
        body = urllib.parse.urlencode({"grant_type": "urn:ietf:params:oauth:grant-type:jwt-bearer",  # WHAT: the JWT bearer grant. WHY: Google's flow for service accounts.
                                       "assertion": signed_assertion}).encode()  # WHAT: the signed assertion. WHY: proves who asks.
        _, answer = self.http.send("POST", TOKEN_URI, {"Content-Type": "application/x-www-form-urlencoded"}, body)  # WHAT: ask. WHY: as above.
        token = (answer or {}).get("access_token")  # WHAT: the token. WHY: used on every call.
        if not isinstance(token, str) or not token:  # WHAT: none came. WHY: the account is wrong.
            raise stores.StoreError("Google gave no access token")  # WHAT: stop. WHY: nothing can be done without one.
        stores.mask(token)  # WHAT: hidden from the log. WHY: it is a secret for an hour.
        self.token = token  # WHAT: kept. WHY: for the calls below.

    def _auth(self, extra=None):  # WHAT: the headers for an API call. WHY: the token on each.
        headers = {"Authorization": f"Bearer {self.token}"}  # WHAT: the bearer token. WHY: Google's way.
        headers.update(extra or {})  # WHAT: and any others. WHY: content types.
        return headers  # WHAT: done. WHY: used by the caller.

    def _app(self, base):  # WHAT: the app's address under a base. WHY: every call uses one.
        return f"{base}/{urllib.parse.quote(self.package, safe='.')}"  # WHAT: base and package. WHY: quoted, though the pattern already keeps it plain.

    def open_edit(self):  # WHAT: edits.insert. WHY: every change happens inside an edit.
        _, answer = self.http.send("POST", f"{self._app(API)}/edits", self._auth({"Content-Type": "application/json"}), b"{}")  # WHAT: a new edit. WHY: as above.
        edit = (answer or {}).get("id")  # WHAT: its id. WHY: named in every call that follows.
        if not isinstance(edit, str) or not re.fullmatch(r"[A-Za-z0-9_-]+", edit):  # WHAT: a sane id. WHY: it goes into addresses.
            raise stores.StoreError("Play gave no usable edit id")  # WHAT: stop. WHY: as above.
        return edit  # WHAT: the id. WHY: as above.

    def upload_bundle(self, edit, path):  # WHAT: edits.bundles.upload. WHY: sends the .aab.
        with open(path, "rb") as f:  # WHAT: the bundle. WHY: its bytes are the body.
            data = f.read()  # WHAT: read whole. WHY: urllib sends a body it holds; a bundle is tens of MB.
        _, answer = self.http.send("POST", f"{self._app(UPLOAD)}/edits/{edit}/bundles?uploadType=media",  # WHAT: a media upload. WHY: the file itself is the body.
                                   self._auth({"Content-Type": "application/octet-stream"}), data, timeout=900)  # WHAT: up to 15 minutes. WHY: a large file on a slow day.
        code = (answer or {}).get("versionCode")  # WHAT: the bundle's version code as Play read it. WHY: the track names the code.
        if not isinstance(code, int) or code <= 0:  # WHAT: a real code. WHY: anything else is a broken answer.
            raise stores.StoreError("Play did not say which version code it received")  # WHAT: stop. WHY: as above.
        return code  # WHAT: the code. WHY: as above.

    def upload_symbols(self, edit, code, path):  # WHAT: the native debug symbols for that version. WHY: crash reports on Play then name our functions.
        with open(path, "rb") as f:  # WHAT: the zip build.sh makes. WHY: as above.
            data = f.read()  # WHAT: read whole. WHY: as above.
        self.http.send("POST", f"{self._app(UPLOAD)}/edits/{edit}/apks/{code}/deobfuscationFiles/nativeCode?uploadType=media",  # WHAT: edits.deobfuscationfiles.upload. WHY: native code symbols.
                       self._auth({"Content-Type": "application/octet-stream"}), data, timeout=900)  # WHAT: as the bundle. WHY: as above.

    def set_track(self, edit, track_body):  # WHAT: edits.tracks.update. WHY: puts the version on the track.
        url = f"{self._app(API)}/edits/{edit}/tracks/{urllib.parse.quote(track_body['track'])}"  # WHAT: the track's address. WHY: Google's layout.
        self.http.send("PUT", url, self._auth({"Content-Type": "application/json"}), json.dumps(track_body).encode())  # WHAT: replace the track's releases. WHY: as above.

    def commit(self, edit):  # WHAT: edits.commit. WHY: until now nothing is visible; this makes it real.
        self.http.send("POST", f"{self._app(API)}/edits/{edit}:commit", self._auth())  # WHAT: commit. WHY: as above.


def release(play, track_body, aab, symbols):  # WHAT: the whole run, in order. WHY: one place, tested with a recorder.
    edit = play.open_edit()  # WHAT: an edit. WHY: every change happens in one.
    stores.say("Uploading the bundle")  # WHAT: progress. WHY: the upload is the slow part.
    code = play.upload_bundle(edit, aab)  # WHAT: the bundle. WHY: as above.
    stores.say(f"Play received version code {code}")  # WHAT: which code. WHY: matches the build's.
    if symbols:  # WHAT: the symbols when there are some. WHY: nice to have, not needed to ship.
        try:  # WHAT: try them. WHY: a failure here should not stop the release.
            play.upload_symbols(edit, code, symbols)  # WHAT: send them. WHY: crash reports.
        except stores.StoreError as e:  # WHAT: Play refused them. WHY: said, then carried on.
            stores.say(f"warning: the debug symbols were not taken: {e}")  # WHAT: a warning. WHY: as above.
    track_body["releases"][0]["versionCodes"] = [str(code)]  # WHAT: the release names that code. WHY: Play wants it as text.
    play.set_track(edit, track_body)  # WHAT: onto the track. WHY: as above.
    play.commit(edit)  # WHAT: made real. WHY: as above.
    return code  # WHAT: the code. WHY: reported.


def main(argv=None):  # WHAT: the command line. WHY: the workflow runs this.
    p = argparse.ArgumentParser(description="Send the Android App Bundle to Google Play.")  # WHAT: the options. WHY: as the docstring.
    p.add_argument("--account-file", required=True)  # WHAT: the service account key file. WHY: written 0600 by the workflow.
    p.add_argument("--aab", required=True)  # WHAT: the bundle. WHY: what is sent.
    p.add_argument("--version", required=True)  # WHAT: the version. WHY: the release's name and a check on the bundle.
    p.add_argument("--package", default="com.excaliburct.view")  # WHAT: the app. WHY: applicationId in mobile/android/app/build.gradle.kts.
    p.add_argument("--symbols")  # WHAT: the native debug symbols zip. WHY: optional.
    p.add_argument("--notes-file")  # WHAT: the release notes. WHY: optional; "What's new" on Play.
    p.add_argument("--track", default="production")  # WHAT: the track. WHY: production unless told otherwise.
    p.add_argument("--status", default="completed")  # WHAT: completed or draft. WHY: as above.
    p.add_argument("--dry-run", action="store_true")  # WHAT: check, do not send. WHY: proves the release without touching Play.
    args = p.parse_args(argv)  # WHAT: read them. WHY: as above.
    work = None  # WHAT: the private folder, once made. WHY: removed in finally.
    try:  # WHAT: everything that can fail. WHY: a plain error, and the key copy always removed.
        notes = stores.read_notes(args.notes_file) if args.notes_file else ""  # WHAT: the notes. WHY: as above.
        track_body = plan(args.package, args.track, args.status, args.version, notes)  # WHAT: the settings checked. WHY: before anything else.
        check_bundle(args.aab, args.version)  # WHAT: the bundle checked. WHY: as above.
        if args.symbols and not os.path.isfile(args.symbols):  # WHAT: the symbols named but missing. WHY: a typo in the workflow.
            raise stores.StoreError(f"no symbols file at {args.symbols}")  # WHAT: stop. WHY: as above.
        account = load_account(args.account_file)  # WHAT: the account checked. WHY: as above.
        work = tempfile.mkdtemp(prefix="play-")  # WHAT: a folder only we can open (0700). WHY: the key copy goes in it.
        key = os.path.join(work, "key.pem")  # WHAT: where the key copy goes. WHY: openssl reads a file.
        fd = os.open(key, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)  # WHAT: created 0600, never there before. WHY: private from the first byte.
        with os.fdopen(fd, "w", encoding="utf-8") as f:  # WHAT: write it. WHY: as above.
            f.write(account["private_key"])  # WHAT: the PEM key. WHY: for openssl only.
        header, claims = assertion(account, time.time())  # WHAT: the token request. WHY: as above.
        signed = stores.make_jwt(header, claims, key, "RS256")  # WHAT: signed. WHY: proves the key works, in a dry run too.
        stores.mask(signed)  # WHAT: hidden. WHY: it can be traded for a token for an hour.
        stores.say(f"Excalibur View {args.version} to Google Play: {args.track} track, {args.status}")  # WHAT: what is about to happen. WHY: the log says it plainly.
        if args.dry_run:  # WHAT: a dry run. WHY: stops here.
            for line in ("POST oauth2 token", "POST edits", "POST upload bundle", "POST upload native symbols" if args.symbols else "",  # WHAT: the calls a real run makes. WHY: a dry run shows them.
                         f"PUT edits/…/tracks/{args.track}", "POST edits/…:commit"):  # WHAT: and the last two. WHY: as above.
                if line:  # WHAT: skip the empty one. WHY: no symbols, no call.
                    stores.say(f"  would {line}")  # WHAT: one call. WHY: as above.
            stores.say(f"  track body: {json.dumps(track_body)}")  # WHAT: what the track would be told. WHY: notes and status can be checked by eye.
            stores.say("Dry run: nothing was sent to Google Play.")  # WHAT: said plainly. WHY: nobody should think it shipped.
            return 0  # WHAT: success. WHY: everything checked.
        play = Play(stores.Http(), args.package)  # WHAT: the real client. WHY: not a dry run.
        play.sign_in(signed)  # WHAT: a token. WHY: as above.
        code = release(play, track_body, args.aab, args.symbols)  # WHAT: the release. WHY: as above.
        stores.say(f"Done: version code {code} is on the {args.track} track ({args.status}).")  # WHAT: the result. WHY: the log says it plainly.
        return 0  # WHAT: success. WHY: as above.
    except stores.StoreError as e:  # WHAT: a plain failure. WHY: no stack trace.
        return stores.fail(str(e))  # WHAT: said and exit 1. WHY: as above.
    finally:  # WHAT: always. WHY: the key copy never outlives the run.
        if work:  # WHAT: if the folder was made. WHY: as above.
            shutil.rmtree(work, ignore_errors=True)  # WHAT: removed. WHY: as above.


if __name__ == "__main__":  # WHAT: run as a script. WHY: the workflow runs it so.
    sys.exit(main())  # WHAT: its exit code. WHY: the job reads it.
