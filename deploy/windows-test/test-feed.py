"""A stand-in for the Office Server's release feed, for the Windows end-to-end test (office-e2e.ps1) only.

It answers the two requests hub::feed::Feed makes, shaped as GitHub's releases API answers them:

    GET /releases?per_page=20      every folder under ROOT as a release: its name is the tag, its files the assets
    GET /files/<tag>/<name>        one of those files

Releases are added while it runs by making a folder under ROOT (files first, release.json last). Only names it lists
itself are ever served, so no path a request carries reaches the disk. Every request is written to the log.

    python test-feed.py ROOT PORT LOG
"""

import http.server  # WHAT: the HTTP server. WHY: the standard library is enough for a test feed
import json  # WHAT: the releases list. WHY: JSON, as GitHub answers
import os  # WHAT: folders and files. WHY: each release is a folder
import sys  # WHAT: the arguments. WHY: root, port and log
import urllib.parse  # WHAT: the request's path, read. WHY: the query is ignored

ROOT, PORT, LOG = sys.argv[1], int(sys.argv[2]), sys.argv[3]  # WHAT: where the releases are, the port, the log. WHY: given by the script


def releases():
    """Every complete release under ROOT: {tag: {name: path}}."""
    found = {}  # WHAT: what is served. WHY: built fresh on every request, so new releases appear
    for tag in sorted(os.listdir(ROOT)):
        folder = os.path.join(ROOT, tag)  # WHAT: one release. WHY: a folder per tag
        if os.path.isdir(folder) and os.path.isfile(os.path.join(folder, "release.json")):  # WHAT: only one with its manifest. WHY: written last, so a half-made one is never seen
            found[tag] = {name: os.path.join(folder, name) for name in os.listdir(folder) if os.path.isfile(os.path.join(folder, name))}  # WHAT: its files. WHY: the assets
    return found


class Feed(http.server.BaseHTTPRequestHandler):
    def log_message(self, format, *args):  # noqa: A002 - the name http.server gives it
        with open(LOG, "a", encoding="utf-8") as f:
            f.write("%s %s\n" % (self.log_date_time_string(), format % args))  # WHAT: one line per request. WHY: the run's logs show what the server asked for

    def do_GET(self):  # noqa: N802 - the name http.server calls
        path = urllib.parse.urlsplit(self.path).path  # WHAT: the path, without the query. WHY: per_page is ignored
        listed = releases()  # WHAT: what there is now. WHY: below
        if path == "/releases":
            body = json.dumps([
                {"tag_name": tag, "draft": False, "prerelease": False,
                 "assets": [{"name": name, "browser_download_url": f"http://127.0.0.1:{PORT}/files/{tag}/{name}"} for name in sorted(files)]}
                for tag, files in listed.items()
            ]).encode("utf-8")  # WHAT: the list, GitHub's shape. WHY: hub::feed reads tag_name, draft, prerelease and assets
            return self.answer(200, "application/json", body)
        parts = path.split("/")  # WHAT: /files/<tag>/<name>. WHY: the only other request
        if len(parts) == 4 and parts[1] == "files" and parts[2] in listed and urllib.parse.unquote(parts[3]) in listed[parts[2]]:  # WHAT: a file this feed lists. WHY: nothing else is ever read
            with open(listed[parts[2]][urllib.parse.unquote(parts[3])], "rb") as f:
                return self.answer(200, "application/octet-stream", f.read())  # WHAT: the file. WHY: the server downloads it
        return self.answer(404, "text/plain", b"not here")  # WHAT: anything else. WHY: never guessed at

    def answer(self, code, kind, body):
        self.send_response(code)  # WHAT: the status. WHY: as above
        self.send_header("Content-Type", kind)  # WHAT: its type. WHY: as above
        self.send_header("Content-Length", str(len(body)))  # WHAT: its length. WHY: the client reads exactly that
        self.end_headers()
        self.wfile.write(body)  # WHAT: the body. WHY: the answer


if __name__ == "__main__":
    http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Feed).serve_forever()  # WHAT: this machine only. WHY: a test feed nobody else reaches
