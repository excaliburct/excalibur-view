#!/usr/bin/env python3
"""Puts an uploaded iPad build up for App Store review.

    python3 tools/appstore_release.py --key-file FILE --version 0.6.30 --build-number 20261110.1530 \
        --notes-file notes-0.6.30.txt [--release manual|after-approval] [--wait-minutes 90] [--dry-run]

The key's id and issuer come from APPLE_API_KEY_ID and APPLE_API_ISSUER (or
--key-id and --issuer). Run by the Release workflow after the iPad build has
sent the archive to App Store Connect; the key file is written there from the
APPLE_API_KEY_P8 secret, 0600, and removed when the step ends.

What it does, through the App Store Connect API, the same steps as the
website:

  1. waits until Apple has finished processing that build (it says VALID),
  2. finds the App Store version for this version number, or makes it (or
     renames the one still being prepared, if an older number was left
     unsubmitted),
  3. picks that build for it, and sets "What's New" from the release notes,
  4. adds it to a review submission and submits that for review.

Release after approval stays a person's button unless --release
after-approval: Apple then puts it out by itself the moment it is approved.

--dry-run checks the settings, the notes and the key (by signing a token with
it), and lists the calls, without calling Apple.
"""
import argparse  # WHAT: the command line. WHY: the workflow passes everything as options.
import json  # WHAT: request bodies. WHY: the API speaks JSON.
import os  # WHAT: the environment. WHY: the key id and issuer come from it.
import re  # WHAT: patterns for accepted values. WHY: refuse anything odd before Apple sees it.
import subprocess  # WHAT: runs openssl. WHY: checks the key is a key, in a dry run too.
import sys  # WHAT: the exit code. WHY: the job goes red on failure.
import time  # WHAT: the clock and waiting. WHY: tokens expire; processing takes minutes.
import urllib.parse  # WHAT: query strings and quoting. WHY: filters go into addresses.

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))  # WHAT: finds stores.py beside this. WHY: run as a plain script.
import stores  # noqa: E402  WHAT: the shared signing and HTTP pieces. WHY: one copy for both stores.

API = "https://api.appstoreconnect.apple.com/v1"  # WHAT: the API's base. WHY: every call starts here.
BUNDLE_ID = "com.excaliburct.view"  # WHAT: the iPad app. WHY: PRODUCT_BUNDLE_IDENTIFIER in mobile/ios/project.yml.
NOTES_LIMIT = 4000  # WHAT: Apple's limit on "What's New". WHY: longer is refused.
RELEASES = {"manual": "MANUAL", "after-approval": "AFTER_APPROVAL"}  # WHAT: who presses release. WHY: manual unless told otherwise.
EDITABLE = {"PREPARE_FOR_SUBMISSION", "DEVELOPER_REJECTED", "REJECTED", "METADATA_REJECTED", "INVALID_BINARY"}  # WHAT: states a version can still be changed in. WHY: only those take a new build and notes.
IN_REVIEW = {"WAITING_FOR_REVIEW", "IN_REVIEW", "PENDING_DEVELOPER_RELEASE", "PROCESSING_FOR_DISTRIBUTION",  # WHAT: states where it is already with Apple or approved. WHY: nothing more to do, and changing it would be wrong.
             "PENDING_APPLE_RELEASE", "ACCEPTED", "WAITING_FOR_EXPORT_COMPLIANCE"}  # WHAT: and the rest of that kind. WHY: as above.
VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")  # WHAT: MAJOR.MINOR.PATCH. WHY: the App Store version string.
BUILD_RE = re.compile(r"^[0-9]{1,9}(\.[0-9]{1,9}){0,2}$")  # WHAT: one to three dotted numbers, as Apple's CFBundleVersion. WHY: the release workflow makes 20261110.1530; nothing else may go into the filter.
KEY_ID_RE = re.compile(r"^[A-Z0-9]{10}$")  # WHAT: an App Store Connect key id. WHY: ten letters and digits.
ISSUER_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")  # WHAT: an issuer id. WHY: a UUID.
APPLE_ID_RE = re.compile(r"^[A-Za-z0-9-]{1,64}$")  # WHAT: an id Apple hands back. WHY: it goes into addresses.


def check_settings(version, build_number, key_id, issuer, release):  # WHAT: refuses odd settings. WHY: before Apple is asked anything.
    if not VERSION_RE.fullmatch(version or ""):  # WHAT: a version. WHY: as above.
        raise stores.StoreError(f"{version!r} is not a version")  # WHAT: stop. WHY: as above.
    if not BUILD_RE.fullmatch(build_number or ""):  # WHAT: a build number. WHY: as above.
        raise stores.StoreError(f"{build_number!r} is not a build number")  # WHAT: stop. WHY: as above.
    if not KEY_ID_RE.fullmatch(key_id or ""):  # WHAT: a key id. WHY: a wrong secret is said here.
        raise stores.StoreError("APPLE_API_KEY_ID is not an App Store Connect key id")  # WHAT: stop, without the value. WHY: it is a secret.
    if not ISSUER_RE.fullmatch(issuer or ""):  # WHAT: an issuer. WHY: as above.
        raise stores.StoreError("APPLE_API_ISSUER is not an issuer id")  # WHAT: stop, without the value. WHY: as above.
    if release not in RELEASES:  # WHAT: a known choice. WHY: as above.
        raise stores.StoreError(f"release must be one of {', '.join(RELEASES)}")  # WHAT: stop. WHY: as above.


def check_key(path):  # WHAT: the key file is a private key openssl can read. WHY: a broken secret is said before waiting an hour.
    stores.check_private_file(path)  # WHAT: 0600. WHY: as in stores.
    out = subprocess.run(["openssl", "pkey", "-in", path, "-noout"], capture_output=True, check=False)  # WHAT: parse it, print nothing. WHY: -noout keeps the key off stdout.
    if out.returncode != 0:  # WHAT: not a key. WHY: wrong or broken secret.
        raise stores.StoreError("the App Store Connect key is not a private key (APPLE_API_KEY_P8 is the .p8 file, base64)")  # WHAT: stop. WHY: says which secret.


def whats_new(path):  # WHAT: the notes as "What's New". WHY: from the notes file the repo keeps for each version.
    text = stores.read_notes(path)  # WHAT: unwrapped paragraphs. WHY: as stores.read_notes.
    if not text:  # WHAT: nothing in it. WHY: Apple needs some text for an update.
        raise stores.StoreError(f"{path} is empty")  # WHAT: stop. WHY: as above.
    return stores.shorten(text, NOTES_LIMIT)  # WHAT: cut to Apple's limit. WHY: as above.


class AppStore:  # WHAT: the calls to App Store Connect, through an Http tests replace. WHY: the request building is tested offline.
    def __init__(self, http, key_path, key_id, issuer, clock=time.time, sleep=time.sleep):  # WHAT: a client. WHY: clock and sleep swapped in tests.
        self.http, self.key_path, self.key_id, self.issuer = http, key_path, key_id, issuer  # WHAT: kept. WHY: every call signs a token.
        self.clock, self.sleep = clock, sleep  # WHAT: time. WHY: as above.
        self._token, self._made = None, 0  # WHAT: no token yet. WHY: made on first use.

    def token(self):  # WHAT: a token, remade every ten minutes. WHY: Apple's last at most twenty; the processing wait is longer.
        now = self.clock()  # WHAT: now. WHY: as above.
        if self._token is None or now - self._made > 600:  # WHAT: none, or ten minutes old. WHY: as above.
            header = {"alg": "ES256", "kid": self.key_id, "typ": "JWT"}  # WHAT: ES256 with our key's id. WHY: Apple's required header.
            claims = {"iss": self.issuer, "iat": int(now), "exp": int(now) + 1200, "aud": "appstoreconnect-v1"}  # WHAT: twenty minutes. WHY: Apple's maximum.
            self._token = stores.make_jwt(header, claims, self.key_path, "ES256")  # WHAT: signed. WHY: as above.
            stores.mask(self._token)  # WHAT: hidden from the log. WHY: it is a secret while it lasts.
            self._made = now  # WHAT: when. WHY: as above.
        return self._token  # WHAT: the token. WHY: as above.

    def call(self, method, path, query=None, *, body=None):  # WHAT: one API call; the body only by name. WHY: the token and JSON in one place, and a body can never land in the query by position.
        url = f"{API}{path}" + (("?" + urllib.parse.urlencode(query)) if query else "")  # WHAT: the address. WHY: filters go in the query.
        headers = {"Authorization": f"Bearer {self.token()}"}  # WHAT: the bearer token. WHY: Apple's way.
        data = None  # WHAT: no body by default. WHY: GETs have none.
        if body is not None:  # WHAT: a body. WHY: POST and PATCH.
            headers["Content-Type"] = "application/json"  # WHAT: JSON. WHY: as above.
            data = json.dumps(body).encode()  # WHAT: encoded. WHY: as above.
        _, answer = self.http.send(method, url, headers, data)  # WHAT: the call. WHY: as above.
        return answer or {}  # WHAT: the JSON, or nothing. WHY: 204 answers have none.

    def app_id(self, bundle_id):  # WHAT: the app's id. WHY: every other call names it.
        found = [a for a in self.call("GET", "/apps", {"filter[bundleId]": bundle_id}).get("data", [])  # WHAT: apps with that bundle id. WHY: Apple's filter.
                 if (a.get("attributes") or {}).get("bundleId") == bundle_id]  # WHAT: exactly that one. WHY: the filter can match a prefix.
        if len(found) != 1:  # WHAT: none, or more than one. WHY: either is wrong.
            raise stores.StoreError(f"App Store Connect has no single app {bundle_id} for this key")  # WHAT: stop. WHY: the key may be for another team.
        return safe_id(found[0])  # WHAT: its id. WHY: as above.

    def wait_for_build(self, app, version, build_number, minutes):  # WHAT: waits until Apple has processed the build. WHY: only a VALID build can be picked.
        deadline = self.clock() + minutes * 60  # WHAT: when to give up. WHY: a stuck build must not hold the runner all day.
        while True:  # WHAT: ask until it is ready. WHY: processing takes 5 to 30 minutes.
            builds = self.call("GET", "/builds", {"filter[app]": app, "filter[version]": build_number,  # WHAT: our build number for our app. WHY: Apple's filters.
                                                  "filter[preReleaseVersion.version]": version, "limit": "5"}).get("data", [])  # WHAT: of this version. WHY: the same number could exist under another version.
            if builds:  # WHAT: Apple has it. WHY: check its state.
                attrs = builds[0].get("attributes") or {}  # WHAT: its attributes. WHY: processingState.
                state = attrs.get("processingState")  # WHAT: its state. WHY: as above.
                if state == "VALID":  # WHAT: ready. WHY: it can be picked.
                    if attrs.get("usesNonExemptEncryption") is None:  # WHAT: export compliance unanswered. WHY: Apple refuses review without it; Info.plist answers it.
                        raise stores.StoreError("the build has no export compliance answer (ITSAppUsesNonExemptEncryption missing from Info.plist?)")  # WHAT: stop. WHY: answering it is a legal statement, made in Info.plist, not here.
                    return safe_id(builds[0])  # WHAT: the build's id. WHY: attached to the version.
                if state in ("FAILED", "INVALID"):  # WHAT: Apple turned it down. WHY: there is nothing to wait for.
                    raise stores.StoreError(f"Apple marked build {build_number} {state}; its email says why")  # WHAT: stop. WHY: as above.
                stores.say(f"Build {build_number} is {state or 'arriving'} at Apple; waiting")  # WHAT: progress. WHY: the log shows the wait.
            else:  # WHAT: not there yet. WHY: uploads take a few minutes to appear.
                stores.say(f"Build {build_number} has not appeared in App Store Connect yet; waiting")  # WHAT: progress. WHY: as above.
            if self.clock() > deadline:  # WHAT: out of time. WHY: as above.
                raise stores.StoreError(f"build {build_number} was not ready after {minutes} minutes; run the submit again later")  # WHAT: stop. WHY: as above.
            self.sleep(60)  # WHAT: a minute between asks. WHY: Apple's own advice is not to hammer it.

    def version_for(self, app, version, release_type):  # WHAT: the App Store version to submit, found, renamed or made. WHY: step 2.
        listed = self.call("GET", f"/apps/{app}/appStoreVersions", {"filter[platform]": "IOS", "limit": "20"}).get("data", [])  # WHAT: the app's recent versions. WHY: to find ours or an editable one.

        def state(v):  # WHAT: a version's state. WHY: Apple renamed appStoreState to appVersionState; either may be there.
            a = v.get("attributes") or {}  # WHAT: attributes. WHY: as above.
            return a.get("appVersionState") or a.get("appStoreState") or ""  # WHAT: the newer name first. WHY: as above.

        same = [v for v in listed if (v.get("attributes") or {}).get("versionString") == version]  # WHAT: ours, if it exists. WHY: reuse it.
        if same:  # WHAT: it exists. WHY: see what state it is in.
            v = same[0]  # WHAT: that one. WHY: a version string is unique per platform.
            if state(v) == "READY_FOR_REVIEW":  # WHAT: already in a submission that was never sent. WHY: a run that stopped after step 4's first half; only the sending is left.
                return safe_id(v), False  # WHAT: its id, and nothing to change. WHY: a version in a submission takes no new build or notes.
            if state(v) in IN_REVIEW:  # WHAT: already with Apple. WHY: submitting twice is refused.
                raise stores.StoreError(f"{version} is already {state(v)} on the App Store; nothing to submit")  # WHAT: stop. WHY: as above.
            if state(v) not in EDITABLE:  # WHAT: released or something else final. WHY: a released version cannot change.
                raise stores.StoreError(f"{version} is {state(v)} on the App Store; raise the version to release again")  # WHAT: stop. WHY: as above.
            self._set(v, {"releaseType": release_type})  # WHAT: who presses release. WHY: the input decides.
            return safe_id(v), True  # WHAT: its id, still to be set up. WHY: steps 3 and 4 follow.
        editable = [v for v in listed if state(v) in EDITABLE]  # WHAT: a version still being prepared under another number. WHY: Apple allows only one, so it is renamed.
        if editable:  # WHAT: there is one. WHY: making another would be refused.
            stores.say(f"Renaming the unsubmitted App Store version {(editable[0].get('attributes') or {}).get('versionString')} to {version}")  # WHAT: said. WHY: it changes something visible.
            self._set(editable[0], {"versionString": version, "releaseType": release_type})  # WHAT: rename it. WHY: as above.
            return safe_id(editable[0]), True  # WHAT: its id, still to be set up. WHY: as above.
        made = self.call("POST", "/appStoreVersions", body={"data": {"type": "appStoreVersions",  # WHAT: a new App Store version. WHY: none to reuse.
                                                               "attributes": {"platform": "IOS", "versionString": version, "releaseType": release_type},  # WHAT: iPad app, this number. WHY: as above.
                                                               "relationships": {"app": {"data": {"type": "apps", "id": app}}}}})  # WHAT: for our app. WHY: as above.
        return safe_id(made.get("data") or {}), True  # WHAT: its id, still to be set up. WHY: as above.

    def _set(self, v, attributes):  # WHAT: changes a version's attributes. WHY: release type, or the number.
        vid = safe_id(v)  # WHAT: its id. WHY: in the address.
        self.call("PATCH", f"/appStoreVersions/{vid}", body={"data": {"type": "appStoreVersions", "id": vid, "attributes": attributes}})  # WHAT: the change. WHY: as above.

    def attach_build(self, version_id, build_id):  # WHAT: picks the build for the version. WHY: step 3.
        self.call("PATCH", f"/appStoreVersions/{version_id}/relationships/build", body={"data": {"type": "builds", "id": build_id}})  # WHAT: the relationship. WHY: Apple's way.

    def set_whats_new(self, version_id, text):  # WHAT: "What's New" in every language the app has. WHY: step 3.
        locs = self.call("GET", f"/appStoreVersions/{version_id}/appStoreVersionLocalizations").get("data", [])  # WHAT: its localizations. WHY: the notes are set on each.
        if not locs:  # WHAT: none. WHY: an app always has its primary language.
            raise stores.StoreError("the App Store version has no languages to set What's New on")  # WHAT: stop. WHY: as above.
        for loc in locs:  # WHAT: each language. WHY: our notes are English; every listing gets them rather than none.
            lid = safe_id(loc)  # WHAT: its id. WHY: in the address.
            self.call("PATCH", f"/appStoreVersionLocalizations/{lid}", body={"data": {"type": "appStoreVersionLocalizations", "id": lid,  # WHAT: the change. WHY: as above.
                                                                                "attributes": {"whatsNew": text}}})  # WHAT: the notes. WHY: as above.

    def submit(self, app, version_id):  # WHAT: a review submission holding the version, submitted. WHY: step 4.
        open_ones = self.call("GET", "/reviewSubmissions", {"filter[app]": app, "filter[platform]": "IOS",  # WHAT: a submission not yet sent. WHY: Apple allows one open at a time.
                                                            "filter[state]": "READY_FOR_REVIEW"}).get("data", [])  # WHAT: only unsent ones. WHY: as above.
        if open_ones:  # WHAT: one is open. WHY: reuse it.
            sid = safe_id(open_ones[0])  # WHAT: its id. WHY: as above.
            items = self.call("GET", f"/reviewSubmissions/{sid}/items", {"include": "appStoreVersion"}).get("data", [])  # WHAT: what is in it. WHY: our version may already be.
            inside = {(((i.get("relationships") or {}).get("appStoreVersion") or {}).get("data") or {}).get("id") for i in items}  # WHAT: the versions in it. WHY: as above.
        else:  # WHAT: none open. WHY: make one.
            made = self.call("POST", "/reviewSubmissions", body={"data": {"type": "reviewSubmissions", "attributes": {"platform": "IOS"},  # WHAT: a new submission. WHY: as above.
                                                                     "relationships": {"app": {"data": {"type": "apps", "id": app}}}}})  # WHAT: for our app. WHY: as above.
            sid, inside = safe_id(made.get("data") or {}), set()  # WHAT: its id; empty. WHY: as above.
        if version_id not in inside:  # WHAT: not in it yet. WHY: add it.
            self.call("POST", "/reviewSubmissionItems", body={"data": {"type": "reviewSubmissionItems", "relationships": {  # WHAT: the version as an item. WHY: Apple's way.
                "reviewSubmission": {"data": {"type": "reviewSubmissions", "id": sid}},  # WHAT: in this submission. WHY: as above.
                "appStoreVersion": {"data": {"type": "appStoreVersions", "id": version_id}}}}})  # WHAT: our version. WHY: as above.
        self.call("PATCH", f"/reviewSubmissions/{sid}", body={"data": {"type": "reviewSubmissions", "id": sid, "attributes": {"submitted": True}}})  # WHAT: sent for review. WHY: the last step.
        return sid  # WHAT: its id. WHY: reported.


def safe_id(resource):  # WHAT: an id Apple gave, checked. WHY: it goes into addresses.
    rid = resource.get("id") if isinstance(resource, dict) else None  # WHAT: the id. WHY: as above.
    if not isinstance(rid, str) or not APPLE_ID_RE.fullmatch(rid):  # WHAT: sane. WHY: as above.
        raise stores.StoreError("App Store Connect answered without a usable id")  # WHAT: stop. WHY: as above.
    return rid  # WHAT: the id. WHY: as above.


def release(store, version, build_number, notes, release_type, minutes, bundle_id=BUNDLE_ID):  # WHAT: the whole run, in order. WHY: one place, tested with a recorder.
    app = store.app_id(bundle_id)  # WHAT: the app. WHY: step 0.
    build = store.wait_for_build(app, version, build_number, minutes)  # WHAT: the processed build. WHY: step 1.
    vid, changeable = store.version_for(app, version, release_type)  # WHAT: the version, and whether it can still change. WHY: step 2.
    if changeable:  # WHAT: not yet in a submission. WHY: only then can its build and notes be set.
        store.attach_build(vid, build)  # WHAT: its build. WHY: step 3.
        store.set_whats_new(vid, notes)  # WHAT: its notes. WHY: step 3.
    return store.submit(app, vid)  # WHAT: sent for review. WHY: step 4.


def main(argv=None):  # WHAT: the command line. WHY: the workflow runs this.
    p = argparse.ArgumentParser(description="Submit an uploaded iPad build for App Store review.")  # WHAT: the options. WHY: as the docstring.
    p.add_argument("--key-file", required=True)  # WHAT: the .p8 key. WHY: written 0600 by the workflow.
    p.add_argument("--key-id", default=os.environ.get("APPLE_API_KEY_ID", ""))  # WHAT: its id. WHY: from the environment, so it is not on a command line.
    p.add_argument("--issuer", default=os.environ.get("APPLE_API_ISSUER", ""))  # WHAT: the issuer. WHY: as above.
    p.add_argument("--version", required=True)  # WHAT: the version. WHY: the App Store version string.
    p.add_argument("--build-number", required=True)  # WHAT: the build number the iPad build used. WHY: finds the build.
    p.add_argument("--notes-file", required=True)  # WHAT: the release notes. WHY: What's New.
    p.add_argument("--release", default="manual")  # WHAT: manual or after-approval. WHY: as the docstring.
    p.add_argument("--wait-minutes", type=int, default=90)  # WHAT: how long to wait for processing. WHY: as above.
    p.add_argument("--dry-run", action="store_true")  # WHAT: check, do not call. WHY: proves the release without touching Apple.
    args = p.parse_args(argv)  # WHAT: read them. WHY: as above.
    try:  # WHAT: everything that can fail. WHY: a plain error.
        check_settings(args.version, args.build_number, args.key_id, args.issuer, args.release)  # WHAT: settings. WHY: first.
        notes = whats_new(args.notes_file)  # WHAT: the notes. WHY: as above.
        check_key(args.key_file)  # WHAT: the key. WHY: as above.
        store = AppStore(stores.Http(), args.key_file, args.key_id, args.issuer)  # WHAT: the client. WHY: as above.
        store.token()  # WHAT: sign one token now. WHY: proves the key signs, in a dry run too.
        stores.say(f"Excalibur View {args.version} (build {args.build_number}) to App Store review; release {args.release}")  # WHAT: what is about to happen. WHY: plain log.
        if args.dry_run:  # WHAT: a dry run. WHY: stops here.
            for line in ("GET apps (find com.excaliburct.view)", f"GET builds (wait for {args.build_number} to be VALID)",  # WHAT: the calls a real run makes. WHY: a dry run shows them.
                         f"GET/POST/PATCH appStoreVersions ({args.version}, {RELEASES[args.release]})", "PATCH relationships/build",  # WHAT: and these. WHY: as above.
                         "PATCH appStoreVersionLocalizations (What's New)", "POST reviewSubmissions + reviewSubmissionItems, PATCH submitted"):  # WHAT: and these. WHY: as above.
                stores.say(f"  would {line}")  # WHAT: one call. WHY: as above.
            stores.say(f"  What's New ({len(notes)} characters):\n{notes}")  # WHAT: the notes as Apple would get them. WHY: checked by eye.
            stores.say("Dry run: nothing was sent to Apple.")  # WHAT: said plainly. WHY: nobody should think it was submitted.
            return 0  # WHAT: success. WHY: everything checked.
        sid = release(store, args.version, args.build_number, notes, RELEASES[args.release], args.wait_minutes)  # WHAT: the submission. WHY: as above.
        stores.say(f"Done: {args.version} is submitted for review (submission {sid}).")  # WHAT: the result. WHY: plain log.
        return 0  # WHAT: success. WHY: as above.
    except stores.StoreError as e:  # WHAT: a plain failure. WHY: no stack trace.
        return stores.fail(str(e))  # WHAT: said and exit 1. WHY: as above.


if __name__ == "__main__":  # WHAT: run as a script. WHY: the workflow runs it so.
    sys.exit(main())  # WHAT: its exit code. WHY: the job reads it.
