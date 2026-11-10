#!/usr/bin/env python3
"""What the Google Play upload and the App Store submission share.

Both stores take a short-lived token signed with a private key: Google's
service account signs with RSA (RS256), Apple's App Store Connect key with an
elliptic curve (ES256). The signing is done by `openssl`, which every build
machine already has and which has been checked by more people than anything
we could add, so nothing is installed with pip and no key is ever handed to a
library downloaded at release time.

Nothing here prints a key or a token. A token is told to GitHub as a secret
(::add-mask::) the moment it exists, so even a mistake later cannot show it in
a log.
"""
import base64  # WHAT: base64url for the token's three parts. WHY: a JWT is three base64url pieces joined by dots.
import json  # WHAT: JSON in and out. WHY: both stores speak JSON.
import os  # WHAT: environment and file modes. WHY: knowing it runs on GitHub, and checking a key file is private.
import re  # WHAT: patterns for the values we accept. WHY: anything else is refused before a store hears of it.
import stat  # WHAT: reads a file's permission bits. WHY: a key file anybody can read is refused.
import subprocess  # WHAT: runs openssl. WHY: openssl does the signing, see above.
import sys  # WHAT: standard error. WHY: refusals go there.
import urllib.error  # WHAT: HTTP errors from urllib. WHY: a store's refusal is read and said plainly.
import urllib.request  # WHAT: the HTTP client in the standard library. WHY: nothing extra to install or trust.


class StoreError(Exception):  # WHAT: the one error these scripts stop with. WHY: the caller prints it and exits 1, no stack trace.
    pass  # WHAT: nothing more than the message. WHY: the message is what Creede reads.


def say(message):  # WHAT: one line of progress. WHY: the job log shows what it is doing.
    print(message, flush=True)  # WHAT: printed at once. WHY: a long wait should not look like a hang.


def mask(value):  # WHAT: tells GitHub to hide a value in every later log line. WHY: a token is a secret even though it is made here.
    if value and os.environ.get("GITHUB_ACTIONS") == "true":  # WHAT: only on GitHub, only for a real value. WHY: the command means nothing elsewhere.
        print(f"::add-mask::{value}", flush=True)  # WHAT: GitHub's masking command. WHY: the runner swallows this line and masks the value from here on.


def b64url(data):  # WHAT: base64url without padding. WHY: that is the JWT encoding.
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode("ascii")  # WHAT: encode, strip '='. WHY: RFC 7515 says no padding.


def check_private_file(path):  # WHAT: refuses a key file others can read. WHY: the workflow writes it 0600; anything else means something went wrong.
    if not os.path.isfile(path):  # WHAT: must be a plain file. WHY: a missing key is said plainly, not as an openssl error.
        raise StoreError(f"no key file at {path}")  # WHAT: stop. WHY: nothing can be signed.
    if os.name == "posix" and stat.S_IMODE(os.stat(path).st_mode) & 0o077:  # WHAT: any group or other permission. WHY: the key must be ours alone.
        raise StoreError(f"{path} can be read by others; it must be 0600")  # WHAT: stop. WHY: never sign with a key left open.


def _openssl_sign(key_path, data):  # WHAT: SHA-256 signature of data with the key, from openssl. WHY: both RS256 and ES256 are this, in different shapes.
    check_private_file(key_path)  # WHAT: the key file is private. WHY: see check_private_file.
    out = subprocess.run(["openssl", "dgst", "-sha256", "-sign", key_path],  # WHAT: sign stdin with the key, binary out. WHY: no key ever passes through argv or our memory as text.
                         input=data, capture_output=True, check=False)  # WHAT: the data on stdin, output captured. WHY: nothing reaches the log by accident.
    if out.returncode != 0:  # WHAT: openssl refused. WHY: a broken or wrong key.
        raise StoreError("openssl could not sign with that key (is it the right kind of key?)")  # WHAT: stop, without openssl's text. WHY: its text can quote the file.
    return out.stdout  # WHAT: the signature bytes. WHY: RS256 uses them as they are; ES256 reshapes them.


def der_ecdsa_to_raw(der, size=32):  # WHAT: turns openssl's DER ECDSA signature into JOSE's r||s. WHY: ES256 in a JWT is the raw 64 bytes, not DER.
    def length(buf, i):  # WHAT: reads a DER length at i. WHY: lengths over 127 use the long form.
        first = buf[i]  # WHAT: the first length byte. WHY: its top bit says short or long form.
        if first < 0x80:  # WHAT: short form. WHY: the length is the byte itself.
            return first, i + 1  # WHAT: length and the next position. WHY: the caller reads on from there.
        count = first & 0x7F  # WHAT: how many bytes the length takes. WHY: long form.
        if count == 0 or count > 2:  # WHAT: indefinite or absurd. WHY: an ECDSA signature is never that.
            raise StoreError("not an ECDSA signature")  # WHAT: stop. WHY: the bytes are not what openssl makes.
        return int.from_bytes(buf[i + 1:i + 1 + count], "big"), i + 1 + count  # WHAT: the long length. WHY: as above.

    def integer(buf, i):  # WHAT: reads one DER INTEGER at i. WHY: r and s are two of them.
        if buf[i] != 0x02:  # WHAT: the INTEGER tag. WHY: anything else is not a signature.
            raise StoreError("not an ECDSA signature")  # WHAT: stop. WHY: as above.
        n, i = length(buf, i + 1)  # WHAT: its length. WHY: to slice it out.
        value = buf[i:i + n].lstrip(b"\x00")  # WHAT: the number without DER's sign padding. WHY: JOSE wants plain fixed-width bytes.
        if len(value) > size:  # WHAT: too big for the curve. WHY: a P-256 number is 32 bytes.
            raise StoreError("not a P-256 signature")  # WHAT: stop. WHY: wrong key type.
        return value.rjust(size, b"\x00"), i + n  # WHAT: left-padded to 32 bytes. WHY: JOSE's fixed width.

    if len(der) < 8 or der[0] != 0x30:  # WHAT: must start with a SEQUENCE. WHY: that is the DER shape.
        raise StoreError("not an ECDSA signature")  # WHAT: stop. WHY: as above.
    _, i = length(der, 1)  # WHAT: skip the SEQUENCE length. WHY: the two integers follow.
    r, i = integer(der, i)  # WHAT: r. WHY: the first half.
    s, _ = integer(der, i)  # WHAT: s. WHY: the second half.
    return r + s  # WHAT: r||s. WHY: the JOSE form.


def make_jwt(header, claims, key_path, alg):  # WHAT: a signed JWT. WHY: both stores take one as the proof of who is asking.
    signing_input = (b64url(json.dumps(header, separators=(",", ":")).encode())  # WHAT: the encoded header. WHY: first part.
                     + "." + b64url(json.dumps(claims, separators=(",", ":")).encode()))  # WHAT: and the claims. WHY: second part.
    raw = _openssl_sign(key_path, signing_input.encode("ascii"))  # WHAT: signed by openssl. WHY: see the module note.
    if alg == "ES256":  # WHAT: Apple's kind. WHY: needs reshaping.
        raw = der_ecdsa_to_raw(raw)  # WHAT: DER to r||s. WHY: JOSE form.
    elif alg != "RS256":  # WHAT: only the two we use. WHY: nothing else is expected.
        raise StoreError(f"unsupported algorithm {alg}")  # WHAT: stop. WHY: a typo should not sign something odd.
    return signing_input + "." + b64url(raw)  # WHAT: the finished token. WHY: header.claims.signature.


def read_notes(path):  # WHAT: the release notes as a store wants them. WHY: the notes files are wrapped at 78 columns for a terminal.
    with open(path, encoding="utf-8") as f:  # WHAT: the notes file. WHY: notes-<version>.txt, kept in the repo.
        text = f.read()  # WHAT: all of it. WHY: it is a few hundred bytes.
    paragraphs = [" ".join(line.strip() for line in block.splitlines() if line.strip())  # WHAT: each paragraph on one line. WHY: a store shows a hard line break where the file had a wrap.
                  for block in re.split(r"\n\s*\n", text)]  # WHAT: split on blank lines. WHY: blank lines are the paragraph breaks.
    return "\n\n".join(p for p in paragraphs if p)  # WHAT: paragraphs, one blank line apart. WHY: reads the same as the file.


def shorten(text, limit):  # WHAT: text cut to fit a store's limit. WHY: Play takes 500 characters of notes, Apple 4,000.
    if len(text) <= limit:  # WHAT: it fits. WHY: nothing to do.
        return text  # WHAT: unchanged. WHY: as above.
    cut = text.rfind("\n\n", 0, limit + 1)  # WHAT: the last whole paragraph that fits. WHY: a cut at a paragraph reads naturally.
    if cut > limit // 3:  # WHAT: only if that keeps a fair amount. WHY: one tiny paragraph is worse than a cut sentence.
        return text[:cut]  # WHAT: whole paragraphs. WHY: as above.
    cut = text.rfind(" ", 0, limit)  # WHAT: else the last space before the limit. WHY: never cut a word in half.
    return text[:cut if cut > 0 else limit - 1].rstrip() + "…"  # WHAT: cut, with an ellipsis. WHY: shows it continues; the ellipsis is one character.


class _NoRedirects(urllib.request.HTTPRedirectHandler):  # WHAT: a redirect handler that follows nothing. WHY: urllib would carry our Authorization header to wherever a redirect points, even plain http.
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # WHAT: asked whether to follow. WHY: as above.
        return None  # WHAT: never. WHY: neither store redirects these calls; a redirect is answered as the error it then is.


_OPENER = urllib.request.build_opener(_NoRedirects)  # WHAT: urllib with that handler. WHY: every call goes through it.


class Http:  # WHAT: the one door to the network. WHY: tests swap it for a recorder, so nothing in a test can reach a store.
    def __init__(self, timeout=120):  # WHAT: a client with a default timeout. WHY: a stuck call must end.
        self.timeout = timeout  # WHAT: seconds. WHY: as above.

    def send(self, method, url, headers=None, body=None, timeout=None):  # WHAT: one request; returns (status, parsed JSON or None). WHY: both stores answer JSON.
        if not url.startswith("https://"):  # WHAT: HTTPS only. WHY: a token must never go out in the clear.
            raise StoreError("refusing a non-HTTPS address")  # WHAT: stop. WHY: as above.
        if body is None and method in ("POST", "PUT", "PATCH"):  # WHAT: a write with nothing to send. WHY: urllib then sends no Content-Length, which Google answers 411.
            body = b""  # WHAT: an empty body. WHY: Content-Length: 0 goes out.
        request = urllib.request.Request(url, data=body, method=method, headers=headers or {})  # WHAT: the request. WHY: urllib's.
        try:  # WHAT: send it. WHY: errors are turned into plain words below.
            with _OPENER.open(request, timeout=timeout or self.timeout) as answer:  # WHAT: the call. WHY: certificate checks are urllib's default and stay on.
                data = answer.read()  # WHAT: the body. WHY: parsed below.
                return answer.status, (json.loads(data) if data else None)  # WHAT: status and JSON. WHY: what callers use.
        except urllib.error.HTTPError as e:  # WHAT: the store said no. WHY: its reason is worth reading.
            try:  # WHAT: read the reason. WHY: the read itself can time out.
                reason = store_reason(e.read())  # WHAT: the store's words. WHY: says what to fix.
            except OSError:  # WHAT: it could not be read. WHY: still a plain message.
                reason = "(the answer could not be read)"  # WHAT: said so. WHY: as above.
            raise StoreError(f"{method} {redact_url(url)} was refused ({e.code}): {reason}") from None  # WHAT: the store's own words. WHY: says what to fix; from None keeps headers (the token) out of the trace.
        except urllib.error.URLError as e:  # WHAT: no answer at all. WHY: network trouble.
            raise StoreError(f"{method} {redact_url(url)} could not be reached: {e.reason}") from None  # WHAT: plain words. WHY: as above.
        except (OSError, ValueError) as e:  # WHAT: a timeout while reading, a dropped connection, or an answer that is not JSON. WHY: still a plain message, not a stack trace.
            raise StoreError(f"{method} {redact_url(url)} failed: {type(e).__name__}") from None  # WHAT: the kind of failure only. WHY: as above.


def redact_url(url):  # WHAT: an address without its query string. WHY: a query could carry something nobody needs in a log.
    return url.split("?", 1)[0]  # WHAT: the part before '?'. WHY: as above.


def store_reason(body):  # WHAT: the readable part of a store's error answer. WHY: the whole body is long JSON.
    try:  # WHAT: it is usually JSON. WHY: both stores answer errors in JSON.
        data = json.loads(body)  # WHAT: parse it. WHY: to pick the message out.
    except ValueError:  # WHAT: not JSON. WHY: a proxy or an outage page.
        return body[:300].decode("utf-8", "replace")  # WHAT: its start. WHY: enough to tell what happened.
    if not isinstance(data, dict):  # WHAT: JSON, but not an object. WHY: a list or a string has no fields to read.
        return json.dumps(data)[:600]  # WHAT: shortened. WHY: still something to read.
    if isinstance(data.get("error"), dict):  # WHAT: Google's shape. WHY: {"error": {"message": ...}}.
        return str(data["error"].get("message", data["error"]))[:600]  # WHAT: Google's message. WHY: says what is wrong.
    if isinstance(data.get("errors"), list):  # WHAT: Apple's shape. WHY: {"errors": [{"title", "detail"}]}.
        return "; ".join(f"{e.get('title', '')}: {e.get('detail', '')}" for e in data["errors"])[:900]  # WHAT: Apple's titles and details. WHY: as above.
    return json.dumps(data)[:600]  # WHAT: anything else, shortened. WHY: still something to read.


def fail(message):  # WHAT: the scripts' common exit. WHY: one plain line and exit code 1.
    print(f"error: {message}", file=sys.stderr, flush=True)  # WHAT: the reason. WHY: the job log shows it in red.
    return 1  # WHAT: the exit code. WHY: the job goes red.
